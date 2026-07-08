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
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File

/** Params needed to (re)launch a transfer, kept so a paused/failed one can resume. */
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
}

/**
 * Foreground service that owns running transfers (decoupled from the Activity so
 * they survive backgrounding), updates [TransferRepository], publishes received
 * files to Downloads, and shows an ongoing progress notification.
 */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val active = java.util.concurrent.atomic.AtomicInteger(0)

    /** Held while an mDNS-enabled transfer runs; Android gates multicast behind it. */
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
                if (direction == null || room == null || path == null) return stopIfIdle()
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
                LogStore.append("app: start $direction room=${room.substringBefore('-')} id=$id")
                runLoop(id, spec)
            }
            ACTION_RESUME -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                val spec = specs[id] ?: return stopIfIdle()
                enterForeground()
                TransferRepository.update(id) { it.copy(status = Status.Connecting, error = null) }
                LogStore.append("app: resume id=$id")
                runLoop(id, spec)
            }
            ACTION_PAUSE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                Native.cancel(id)
                TransferRepository.update(id) {
                    if (it.status.isTerminal) it else it.copy(status = Status.Paused, speedBps = 0.0)
                }
            }
            ACTION_CANCEL -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                Native.cancel(id)
                specs.remove(id)
                TransferRepository.update(id) {
                    if (it.status.isTerminal) it else it.copy(status = Status.Cancelled)
                }
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

    private fun runLoop(id: Long, spec: Spec) {
        active.incrementAndGet()
        updateNotification()
        scope.launch {
            var lastTs = 0L
            var lastBytes = 0L
            var lastNotif = 0L
            var startTs = 0L
            if (spec.useMdns) runCatching { multicastLock.acquire() }
            NativeTransfer.run(id, spec.direction, spec.room, spec.broker, spec.relay, spec.path, spec.chunkSize, spec.candidatesAllow, spec.candidatesDeny, spec.useRoom, spec.useMdns)
                .collect { ev ->
                    TransferRepository.update(id) { t ->
                        when (ev) {
                            CliEvent.Binding, CliEvent.Connecting ->
                                t.copy(
                                    status = Status.Connecting,
                                    log = if (t.log.isEmpty())
                                        addLog(t.log, "pairing in room ${spec.room.substringBefore('-')}…")
                                    else t.log,
                                )
                            is CliEvent.Connected ->
                                t.copy(
                                    pathType = ev.pathType, pathAddr = ev.addr,
                                    log = addLog(
                                        t.log,
                                        "connected · ${ev.pathType}" + if (ev.addr.isNotBlank()) " (${ev.addr})" else "",
                                    ),
                                )
                            is CliEvent.Started ->
                                t.copy(
                                    fileName = ev.fileName, total = ev.totalBytes, status = Status.Transferring,
                                    log = addLog(t.log, "started · ${ev.fileName}"),
                                )
                            is CliEvent.Progress -> {
                                val now = System.nanoTime()
                                if (startTs == 0L) {
                                    startTs = now; lastTs = now; lastBytes = ev.bytesTransferred
                                }
                                // True average = total bytes / elapsed (matches the CLI's avg_bps),
                                // not the mean of per-event samples (which over-weights fast bursts).
                                val avg = if (now > startTs)
                                    ev.bytesTransferred * 1e9 / (now - startTs) else 0.0
                                val dt = now - lastTs
                                // Sample the instantaneous rate over >=250ms windows, so the chart
                                // and peak reflect real bursts, not sub-ms measurement noise.
                                if (dt >= 250_000_000L) {
                                    val bps = (ev.bytesTransferred - lastBytes) * 1e9 / dt
                                    lastTs = now; lastBytes = ev.bytesTransferred
                                    t.copy(
                                        bytes = ev.bytesTransferred, total = ev.totalBytes,
                                        speedBps = bps, avgBps = avg, status = Status.Transferring,
                                        speedHistory = (t.speedHistory + bps).takeLast(90),
                                    )
                                } else {
                                    t.copy(
                                        bytes = ev.bytesTransferred, total = ev.totalBytes,
                                        avgBps = avg, status = Status.Transferring,
                                    )
                                }
                            }
                            is CliEvent.Completed -> {
                                val now = System.nanoTime()
                                val avg = if (startTs > 0 && now > startTs)
                                    ev.bytesTransferred * 1e9 / (now - startTs) else t.avgBps
                                t.copy(
                                    bytes = ev.bytesTransferred, speedBps = 0.0, avgBps = avg,
                                    status = Status.Completed, log = addLog(t.log, "complete"),
                                )
                            }
                            is CliEvent.Failed ->
                                // A pause/cancel surfaces as a Failed event; keep the
                                // Paused/Cancelled status the action already set.
                                if (t.status == Status.Cancelled || t.status == Status.Paused) t
                                else t.copy(status = Status.Failed, error = ev.error, log = addLog(t.log, "failed · ${ev.error}"))
                            is CliEvent.Exit ->
                                if (t.status.isTerminal || t.status == Status.Paused) t
                                else t.copy(status = if (ev.code == 0) Status.Completed else Status.Failed)
                        }
                    }
                    // Keep the foreground notification's progress bar live while
                    // backgrounded; throttle the frequent Progress events.
                    when (ev) {
                        is CliEvent.Progress -> {
                            val now = System.currentTimeMillis()
                            if (now - lastNotif > 700) { lastNotif = now; updateNotification() }
                        }
                        is CliEvent.Started, is CliEvent.Connected -> updateNotification()
                        else -> {}
                    }
                }
            if (spec.useMdns) runCatching { multicastLock.release() }
            if (spec.dir() == Direction.Receive) publishReceived(id, spec.path)
            active.decrementAndGet()
            // Refresh only while transfers remain; when idle, let stopIfIdle's
            // stopForeground(REMOVE) clear the notification — reposting here would
            // detach it and leave a stale status-bar icon.
            if (active.get() > 0) updateNotification()
            stopIfIdle()
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

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification())
    }

    private fun stopIfIdle(): Int {
        if (active.get() <= 0) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIF_ID = 1
        private const val ACTION_START = "dev.envoix.app.START"
        private const val ACTION_CANCEL = "dev.envoix.app.CANCEL"
        private const val ACTION_PAUSE = "dev.envoix.app.PAUSE"
        private const val ACTION_RESUME = "dev.envoix.app.RESUME"

        /** Launch specs by id, so a paused/failed transfer can be resumed. */
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

        /** Relaunch a paused/failed transfer from its stored spec (resumes the partial). */
        fun resume(context: Context, id: Long) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_RESUME
                    putExtra(EXTRA_ID, id)
                }
            )
        }
    }
}
