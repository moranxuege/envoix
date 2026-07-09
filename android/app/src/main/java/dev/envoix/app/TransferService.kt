package dev.envoix.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.io.File

/** Params needed to (re)create a transfer session (also carries UI extras). */
private data class Spec(
    val direction: String,
    val room: String,
    val path: String,
    val broker: String,
    val relay: String,
    val chunkSize: String,
    val candidatesAllow: String,
    val candidatesDeny: String,
    /** Invite payload to advertise as a QR while waiting (initiated sessions only). */
    val qrPayload: String?,
    /** Rendezvous modes to attempt, in order Room → mDNS. */
    val useRoom: Boolean,
    val useMdns: Boolean,
) {
    fun dir(): Direction = if (direction == "send") Direction.Send else Direction.Receive

    /** The session params JSON for [Native.createSession]. `resume` false for a
     *  user-initiated NEW transfer (fresh semantics); true when re-creating a
     *  session that should honor partials/receipts. */
    fun paramsJson(resume: Boolean): String = JSONObject().apply {
        put("direction", direction)
        put("code", room)
        put("broker", broker)
        put("relay", relay)
        put("path", path)
        put("chunk_size", chunkSize)
        put("candidates_allow", candidatesAllow)
        put("candidates_deny", candidatesDeny)
        put("use_room", useRoom)
        put("use_mdns", useMdns)
        put("resume", resume)
    }.toString()
}

/**
 * Foreground service that owns transfer sessions. Since the state machine
 * moved into the Rust core (envoix-client machine + driver), this service no
 * longer interprets events: it forwards user intents to the session, renders
 * the snapshot stream into [TransferRepository], acts as the mailbox courier
 * (dumb HTTP GET/POST — the driver seals, verifies, and decides), and keeps
 * the Android-only side effects: notifications, MediaStore publish, multicast.
 */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** Collector jobs per transfer id (live Rust session ⇔ live job). */
    private val jobs = java.util.concurrent.ConcurrentHashMap<Long, Job>()

    /** Last applied snapshot seq per id: out-of-order snapshots are dropped. */
    private val lastSeq = java.util.concurrent.ConcurrentHashMap<Long, Long>()

    /** Held while an mDNS-enabled session runs; Android gates multicast behind it. */
    private val multicastLock by lazy {
        (getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager)
            .createMulticastLock("envoix-mdns").apply { setReferenceCounted(true) }
    }

    /** True when the active network actually reaches the internet (not just a
     *  captive portal). Room pairing needs the broker, so skip it when this is
     *  false — otherwise Room just retries an unreachable broker forever, and the
     *  mDNS fallback never gets a turn. */
    private fun hasInternet(): Boolean {
        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) } ?: return false
        return caps.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
            caps.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_VALIDATED)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val mgr = getSystemService(NotificationManager::class.java)
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL, "Transfers", NotificationManager.IMPORTANCE_LOW)
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                val direction = intent.getStringExtra(EXTRA_DIRECTION)
                val room = intent.getStringExtra(EXTRA_ROOM)
                val path = intent.getStringExtra(EXTRA_PATH)
                if (direction == null || room == null || path == null) return START_NOT_STICKY
                val spec = Spec(
                    direction, room, path,
                    intent.getStringExtra(EXTRA_BROKER) ?: Endpoints.BROKER,
                    intent.getStringExtra(EXTRA_RELAY) ?: Endpoints.RELAY,
                    intent.getStringExtra(EXTRA_CHUNK_SIZE) ?: "",
                    intent.getStringExtra(EXTRA_CAND_ALLOW) ?: "",
                    intent.getStringExtra(EXTRA_CAND_DENY) ?: "",
                    intent.getStringExtra(EXTRA_QR),
                    SettingsStore.settings.value.useRoom && hasInternet(),
                    SettingsStore.settings.value.useMdns,
                )
                enterForeground()
                val id = TransferRepository.create(spec.dir(), room)
                TransferRepository.update(id) {
                    it.copy(
                        qrPayload = spec.qrPayload,
                        // Show the outgoing file name right away; receives learn it on Started.
                        fileName = if (spec.dir() == Direction.Send) File(spec.path).name else it.fileName,
                    )
                }
                specs[id] = spec
                OpLog.add("start $direction room=${room.substringBefore('-')} id=$id")
                startSession(id, spec, resume = false)
            }
            ACTION_RESUME -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                enterForeground()
                OpLog.add("resume transfer id=$id")
                if (jobs.containsKey(id)) {
                    // Live session: the machine handles the attempt bump.
                    Native.sessionIntent(id, "resume")
                } else {
                    // Session died with the service; re-create it with resume
                    // semantics (partials/receipts honored).
                    val spec = specs[id] ?: return START_NOT_STICKY
                    startSession(id, spec, resume = true)
                }
            }
            ACTION_PAUSE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("pause transfer id=$id")
                Native.sessionIntent(id, "pause")
            }
            ACTION_CANCEL -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("cancel transfer id=$id")
                Native.sessionIntent(id, "cancel")
            }
            ACTION_REMOVE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("remove transfer id=$id")
                // D2, the one true abandon: discard partial + resume state +
                // receipt, then tear the session and card down.
                Native.destroySession(id, true)
                jobs.remove(id)?.cancel()
                specs.remove(id)
                lastSeq.remove(id)
                TransferRepository.remove(id)
                stopIfIdle()
            }
        }
        return START_NOT_STICKY
    }

    private fun enterForeground() =
        startForeground(NOTIF_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)

    private val logTime = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US)

    /** Append a timestamped line to a transfer's log, keeping the last 60. */
    private fun addLog(cur: List<String>, line: String): List<String> =
        (cur + "${logTime.format(java.util.Date())}  $line").takeLast(TransferRepository.LOG_CAP)

    /** Create the Rust session and render its notice stream. */
    private fun startSession(id: Long, spec: Spec, resume: Boolean) {
        lastSeq[id] = 0L
        val job = scope.launch {
            if (spec.useMdns) runCatching { multicastLock.acquire() }
            try {
                NativeSession.start(id, spec.paramsJson(resume)).collect { notice ->
                    when (notice.optString("notice")) {
                        "snapshot" -> onSnapshot(id, spec, notice)
                        "fetch_receipt" -> onFetchReceipt(id, notice.optString("key"))
                        "post_receipt" -> onPostReceipt(id, notice)
                    }
                }
            } finally {
                if (spec.useMdns) runCatching { multicastLock.release() }
            }
        }
        jobs[id] = job
        updateNotification()
    }

    /** Map one machine snapshot onto the card. The machine is authoritative:
     *  no interpretation, no guards — just rendering. */
    private fun onSnapshot(id: Long, spec: Spec, s: JSONObject) {
        val seq = s.optLong("seq")
        val prev = lastSeq[id] ?: 0L
        if (seq <= prev) return
        lastSeq[id] = seq

        val state = s.optString("state")
        val status = when (state) {
            "waiting", "connecting", "verifying" -> Status.Connecting
            "transferring", "confirming" -> Status.Transferring
            "paused" -> Status.Paused
            "unconfirmed" -> Status.Unconfirmed
            "completed" -> Status.Completed
            "failed" -> Status.Failed
            "cancelled" -> Status.Cancelled
            else -> return
        }
        val reason = s.optString("reason").ifEmpty { null }
        val bytes = s.optLong("bytes")
        val total = s.optLong("total")
        val speed = s.optDouble("speed_bps", 0.0)
        val avg = s.optDouble("avg_bps", 0.0)
        val path = s.optString("path").ifEmpty { null }
        var entered: String? = null

        TransferRepository.update(id) { t ->
            entered = if (t.status != status) stateLogLine(state, s, bytes) else null
            t.copy(
                status = status,
                transferId = s.optString("transfer_id").ifEmpty { t.transferId },
                fileName = s.optString("file_name").ifEmpty { t.fileName },
                bytes = bytes,
                total = if (total > 0) total else t.total,
                speedBps = if (status == Status.Transferring) speed else 0.0,
                avgBps = avg,
                speedHistory = if (status == Status.Transferring && speed > 0)
                    (t.speedHistory + speed).takeLast(90) else t.speedHistory,
                pathType = path?.substringBefore(' ') ?: t.pathType,
                pathAddr = path?.substringAfter('(', "")?.removeSuffix(")")?.ifEmpty { null }
                    ?: t.pathAddr,
                error = if (status == Status.Failed || status == Status.Unconfirmed) reason else null,
                log = entered?.let { addLog(t.log, it) } ?: t.log,
            )
        }

        when {
            entered != null -> updateNotification()
            else -> throttledNotification()
        }
        if (state == "completed" && spec.dir() == Direction.Receive) {
            publishReceived(id, spec.path)
        }
        if (!jobsActive()) stopForegroundKeepCards()
    }

    /** Human line for a state transition, for the card's log drawer. */
    private fun stateLogLine(state: String, s: JSONObject, bytes: Long): String = when (state) {
        "waiting" -> "waiting for peer…"
        "connecting" -> "pairing in room…"
        "verifying" -> "verifying…"
        "transferring" -> "started · ${s.optString("file_name")}"
        "confirming" -> "confirming delivery…"
        "paused" -> when (s.optString("origin")) {
            "peer" -> "paused by peer (resumable)"
            "lost" -> "paused · interrupted, $bytes B kept (resumable)"
            else -> "paused"
        }
        "unconfirmed" -> "sent · unconfirmed — awaiting proof (mailbox)"
        "completed" -> "complete"
        "failed" -> "failed · ${s.optString("reason")}"
        "cancelled" -> "cancelled"
        else -> state
    }

    /** Courier: GET the mailbox slot and hand the blob back to the driver. */
    private fun onFetchReceipt(id: Long, key: String) {
        if (key.isEmpty()) return
        val server = SettingsStore.settings.value.logServer.trimEnd('/')
        if (server.isEmpty()) return
        scope.launch {
            val blob = LogUpload.getBytes("$server/receipts/$key")
            val b64 = blob?.let { java.util.Base64.getEncoder().encodeToString(it) } ?: ""
            Native.receiptResponse(id, b64)
        }
    }

    /** Courier: POST the sealed receipt blob (with backoff — the whole point
     *  of the mailbox is that this can retry long after the ack could not). */
    private fun onPostReceipt(id: Long, notice: JSONObject) {
        val key = notice.optString("key")
        val b64 = notice.optString("blob")
        if (key.isEmpty() || b64.isEmpty()) return
        val server = SettingsStore.settings.value.logServer.trimEnd('/')
        if (server.isEmpty()) return
        val bytes = runCatching { java.util.Base64.getDecoder().decode(b64) }.getOrNull() ?: return
        scope.launch {
            for (backoff in listOf(0L, 5_000L, 30_000L)) {
                if (backoff > 0) delay(backoff)
                if (LogUpload.postBytes("$server/receipts/$key", bytes)) {
                    OpLog.add("receipt posted id=$id")
                    return@launch
                }
            }
            OpLog.add("receipt post failed id=$id")
        }
    }

    /** Move a completed received file from the private output dir into Downloads. */
    private fun publishReceived(id: Long, outputDir: String) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status != Status.Completed) return
        val name = t.fileName ?: return
        val src = File(outputDir, name)
        if (!src.exists()) return
        val s = SettingsStore.settings.value
        val uri = MediaStoreSaver.saveReceived(this, src, name, s.saveTreeUri, s.saveFolder)
        if (uri != null) {
            src.delete()
            TransferRepository.update(id) { it.copy(savedUri = uri.toString()) }
            LogStore.append("app: saved $name to Downloads")
        }
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val active = TransferRepository.transfers.value.filter {
            it.status == Status.Transferring || it.status == Status.Connecting
        }
        val b = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setOngoing(active.isNotEmpty())
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
        val t = active.singleOrNull()
        when {
            active.isEmpty() -> b.setContentTitle("Envoix").setContentText("Done")
            t == null -> b.setContentTitle("Envoix").setContentText("${active.size} transfers in progress")
            else -> {
                b.setSmallIcon(
                    if (t.direction == Direction.Send) android.R.drawable.stat_sys_upload
                    else android.R.drawable.stat_sys_download,
                )
                val verb = if (t.direction == Direction.Send) "Sending" else "Receiving"
                b.setContentTitle("$verb ${t.fileName ?: "…"}")
                if (t.status == Status.Transferring && t.total > 0) {
                    val pct = ((t.bytes * 100) / t.total).toInt().coerceIn(0, 100)
                    b.setContentText("$pct%  ·  ${humanBytes(t.bytes)} / ${humanBytes(t.total)}")
                    b.setProgress(100, pct, false)
                } else {
                    b.setContentText("Connecting…")
                    b.setProgress(0, 0, true)
                }
            }
        }
        return b.build()
    }

    private fun humanBytes(n: Long): String = when {
        n < 1024 -> "$n B"
        n < 1024 * 1024 -> "%.0f KB".format(n / 1024.0)
        n < 1024L * 1024 * 1024 -> "%.1f MB".format(n / 1048576.0)
        else -> "%.2f GB".format(n / 1073741824.0)
    }

    private var lastNotif = 0L

    private fun throttledNotification() {
        val now = System.currentTimeMillis()
        if (now - lastNotif > 700) {
            lastNotif = now
            updateNotification()
        }
    }

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification())
    }

    /** Whether any card still has a live attempt or a pending proof. */
    private fun jobsActive(): Boolean = TransferRepository.transfers.value.any {
        it.status == Status.Connecting || it.status == Status.Transferring ||
            it.status == Status.Unconfirmed
    }

    /** All cards at rest: drop the foreground state (notification dismissible)
     *  but keep the service — sessions idle cheaply and stay resumable. */
    private fun stopForegroundKeepCards() {
        stopForeground(STOP_FOREGROUND_DETACH)
        updateNotification()
    }

    private fun stopIfIdle(): Int {
        if (TransferRepository.transfers.value.isEmpty()) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel() // collectors' awaitClose destroys the Rust sessions
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIF_ID = 1
        private const val ACTION_START = "dev.envoix.app.START"
        private const val ACTION_CANCEL = "dev.envoix.app.CANCEL"
        private const val ACTION_PAUSE = "dev.envoix.app.PAUSE"
        private const val ACTION_RESUME = "dev.envoix.app.RESUME"
        private const val ACTION_REMOVE = "dev.envoix.app.REMOVE"

        /** Launch specs by id, so a session can be re-created after the service
         *  (or process) restarted. */
        private val specs = java.util.concurrent.ConcurrentHashMap<Long, Spec>()
        private const val EXTRA_DIRECTION = "direction"
        private const val EXTRA_ROOM = "room"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_BROKER = "broker"
        private const val EXTRA_RELAY = "relay"
        private const val EXTRA_CHUNK_SIZE = "chunk_size"
        private const val EXTRA_CAND_ALLOW = "candidates_allow"
        private const val EXTRA_CAND_DENY = "candidates_deny"
        private const val EXTRA_QR = "qr"
        private const val EXTRA_ID = "id"

        /** `direction` is "send"/"receive"; `path` is the file to send or the
         *  output directory to receive into; the config fields carry chunk_size +
         *  the candidate CIDR allow/deny lists (comma-joined, "" when unset);
         *  `qrPayload` is the invite to show while waiting (null when joining). */
        fun start(
            context: Context,
            direction: String,
            room: String,
            path: String,
            broker: String,
            relay: String,
            chunkSize: String,
            candidatesAllow: String,
            candidatesDeny: String,
            qrPayload: String?,
        ) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_START
                    putExtra(EXTRA_DIRECTION, direction)
                    putExtra(EXTRA_ROOM, room)
                    putExtra(EXTRA_PATH, path)
                    putExtra(EXTRA_BROKER, broker)
                    putExtra(EXTRA_RELAY, relay)
                    putExtra(EXTRA_CHUNK_SIZE, chunkSize)
                    putExtra(EXTRA_CAND_ALLOW, candidatesAllow)
                    putExtra(EXTRA_CAND_DENY, candidatesDeny)
                    putExtra(EXTRA_QR, qrPayload)
                }
            )
        }

        fun cancel(context: Context, id: Long) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_CANCEL
                    putExtra(EXTRA_ID, id)
                }
            )
        }

        /** Stop a transfer but keep its partial + spec, so it can be resumed. */
        fun pause(context: Context, id: Long) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_PAUSE
                    putExtra(EXTRA_ID, id)
                }
            )
        }

        /** Resume/retry: a live session bumps its attempt; a dead one is
         *  re-created from its spec with resume semantics. */
        fun resume(context: Context, id: Long) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_RESUME
                    putExtra(EXTRA_ID, id)
                }
            )
        }

        /** Remove the card AND its on-disk leftovers (D2: the one true abandon). */
        fun remove(context: Context, id: Long) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_REMOVE
                    putExtra(EXTRA_ID, id)
                }
            )
        }
    }
}
