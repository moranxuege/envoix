package dev.envoix.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.IBinder
import android.provider.OpenableColumns
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
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
    fun paramsJson(resume: Boolean): String =
        JSONObject()
            .apply {
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

    /** Latch for the active→resting tray transition. */
    private var wasActive = false

    /** Held while an mDNS-enabled session runs; Android gates multicast behind it. */
    private val multicastLock by lazy {
        (getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager)
            .createMulticastLock("envoix-mdns")
            .apply { setReferenceCounted(true) }
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
            NotificationChannel(CHANNEL, "Transfers", NotificationManager.IMPORTANCE_LOW),
        )
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_START -> {
                val direction = intent.getStringExtra(EXTRA_DIRECTION)
                val room = intent.getStringExtra(EXTRA_ROOM)
                val path = intent.getStringExtra(EXTRA_PATH)
                if (direction == null || room == null || path == null) return START_NOT_STICKY
                val spec =
                    Spec(
                        direction,
                        room,
                        path,
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
                        fileName =
                            if (spec.dir() == Direction.Send && spec.path.isNotEmpty()) {
                                File(spec.path).name
                            } else {
                                it.fileName
                            },
                    )
                }
                specs[id] = spec
                OpLog.add("start $direction room=${room.substringBefore('-')} id=$id")
                val sourceUri = intent.getStringExtra(EXTRA_SOURCE_URI)
                if (spec.dir() == Direction.Send && !sourceUri.isNullOrEmpty()) {
                    stageAndStart(id, spec, Uri.parse(sourceUri))
                } else {
                    startSession(id, spec, resume = false)
                }
            }
            ACTION_RESUME -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                enterForeground()
                OpLog.add("resume transfer id=$id", id)
                // Restore-then-intent: rehydrate any dead sessions from their
                // records FIRST, then let the machine's legality table decide.
                // Never reconstruct from a shadow copy (the Q3 bypass bug).
                if (!jobs.containsKey(id)) restoreAllRecords()
                Native.sessionIntent(id, "resume")
            }
            ACTION_PAUSE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("pause transfer id=$id", id)
                Native.sessionIntent(id, "pause")
            }
            ACTION_CANCEL -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("cancel transfer id=$id", id)
                Native.sessionIntent(id, "cancel")
            }
            ACTION_RESTORE_ALL -> restoreAllRecords()
            ACTION_REVERIFY -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("serve re-verify id=$id", id)
                if (!jobs.containsKey(id)) restoreAllRecords()
                Native.sessionIntent(id, "reverify")
            }
            ACTION_REMOVE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                OpLog.add("remove transfer id=$id", id)
                // D2, the one true abandon: discard partial + resume state +
                // receipt, then tear the session and card down. The send
                // staging dir is keyed by card id, so it goes too without
                // consulting the (possibly already gone) spec.
                File(File(cacheDir, "send"), id.toString()).deleteRecursively()
                Native.destroySession(id, true)
                TransferLogs.delete(id)
                jobs.remove(id)?.cancel()
                specs.remove(id)
                lastSeq.remove(id)
                TransferRepository.remove(id)
                stopIfIdle()
            }
        }
        return START_NOT_STICKY
    }

    private fun enterForeground() = startForeground(NOTIF_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)

    private val logTime = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US)

    /** Append a timestamped line to a transfer's log, keeping the last 60. */
    private fun addLog(
        cur: List<String>,
        line: String,
    ): List<String> = (cur + "${logTime.format(java.util.Date())}  $line").takeLast(TransferRepository.LOG_CAP)

    /**
     * Restore persisted transfer records (roadmap #5): recreate each card with
     * its durable id and rehydrate its Rust session — the session's initial
     * snapshot repopulates the card through the normal rendering path. Resting
     * cards idle; a restored Unconfirmed resumes its mailbox poll in Rust.
     */
    private fun restoreAllRecords() {
        val records = runCatching { org.json.JSONArray(Native.listRecords()) }.getOrNull() ?: return
        for (i in 0 until records.length()) {
            val rec = records.optJSONObject(i) ?: continue
            val id = rec.optLong("id", -1L)
            if (id < 0 || jobs.containsKey(id)) continue
            val context = rec.optJSONObject("context") ?: rec
            val params = context.optJSONObject("params") ?: rec.optJSONObject("params") ?: continue
            val client = context.optJSONObject("client")
            val direction = if (params.optString("direction") == "Send") "send" else "receive"
            val sources = params.optJSONArray("sources")
            var code = ""
            var broker = ""
            var useRoom = false
            var useMdns = false
            for (j in 0 until (sources?.length() ?: 0)) {
                val src = sources!!.optJSONObject(j) ?: continue
                src.optJSONObject("Room")?.let {
                    useRoom = true
                    code = it.optString("code")
                    broker = it.optString("broker")
                }
                src.optJSONObject("Mdns")?.let {
                    useMdns = true
                    if (code.isEmpty()) code = it.optString("token")
                }
            }
            if (code.isEmpty()) continue
            if (!TransferRepository.restoreCard(id, if (direction == "send") Direction.Send else Direction.Receive, code)) continue
            val spec =
                Spec(
                    direction,
                    code,
                    params.optString("path"),
                    broker.ifEmpty { Endpoints.BROKER },
                    params
                        .optJSONObject("options")
                        ?.optString("relay")
                        .orEmpty()
                        .ifEmpty { Endpoints.RELAY },
                    client?.optString("chunk_size").orEmpty(),
                    jsonStringArrayCsv(client?.optJSONArray("candidates_allow")),
                    jsonStringArrayCsv(client?.optJSONArray("candidates_deny")),
                    null,
                    useRoom,
                    useMdns,
                )
            specs[id] = spec
            lastSeq[id] = 0L
            val job =
                scope.launch {
                    NativeSession.restore(id).collect { notice ->
                        when (notice.optString("notice")) {
                            "snapshot" -> onSnapshot(id, spec, notice)
                            "fetch_receipt" -> onFetchReceipt(id, notice.optString("key"))
                            "post_receipt" -> onPostReceipt(id, notice)
                        }
                    }
                }
            jobs[id] = job
            OpLog.add("restored transfer id=$id")
        }
    }

    private fun jsonStringArrayCsv(array: org.json.JSONArray?): String =
        (0 until (array?.length() ?: 0))
            .mapNotNull { array?.optString(it)?.takeIf(String::isNotEmpty) }
            .joinToString(",")

    /**
     * Stage a picked content:// into a real path the core can (re)open across
     * attempts, VISIBLY: the card exists from the moment of the tap, and the
     * copy shows as "preparing" with a live bar (for a large file this phase
     * is seconds - hiding it made Send look dead). Runs in the service scope,
     * so a rotation mid-copy no longer kills the send.
     */
    private fun stageAndStart(
        id: Long,
        spec0: Spec,
        uri: Uri,
    ) {
        scope.launch(Dispatchers.IO) {
            // The provider's DISPLAY_NAME is untrusted input (it can contain
            // path separators): keep only the leaf, never a dot name.
            val name =
                (displayName(uri) ?: "upload.bin")
                    .let { File(it).name }
                    .takeUnless { it.isEmpty() || it == "." || it == ".." }
                    ?: "upload.bin"
            val size = querySize(uri)
            TransferRepository.update(id) {
                it.copy(fileName = name, total = size, log = addLog(it.log, "preparing · staging $name…"))
            }
            // Staging is keyed by card id: two same-named sends must never
            // share a source path (the sender hashes as it reads, so a
            // mid-send overwrite can pass verification with mixed bytes).
            val out = File(File(File(cacheDir, "send"), id.toString()).apply { mkdirs() }, name)
            val ok =
                runCatching {
                    contentResolver.openInputStream(uri)!!.use { input ->
                        out.outputStream().use { o ->
                            val buf = ByteArray(1 shl 20)
                            var copied = 0L
                            var last = 0L
                            while (true) {
                                val n = input.read(buf)
                                if (n < 0) break
                                o.write(buf, 0, n)
                                copied += n
                                val now = System.currentTimeMillis()
                                if (now - last > 150) {
                                    last = now
                                    TransferRepository.update(id) { it.copy(bytes = copied) }
                                }
                            }
                        }
                    }
                }.isSuccess
            if (!ok) {
                out.delete()
                TransferRepository.update(id) {
                    it.copy(
                        status = Status.Failed,
                        error = "couldn't read the picked file",
                        log = addLog(it.log, "failed · staging the picked file"),
                    )
                }
                return@launch
            }
            // Remove may have raced the staging copy: never start a session
            // for a card that no longer exists (it would run invisibly and
            // re-create the record Remove just deleted).
            if (TransferRepository.transfers.value.none { it.id == id }) {
                out.parentFile?.deleteRecursively()
                return@launch
            }
            // Reset the bar for the real transfer; the machine owns it from here.
            TransferRepository.update(id) { it.copy(bytes = 0) }
            val spec = spec0.copy(path = out.absolutePath)
            specs[id] = spec
            startSession(id, spec, resume = false)
        }
    }

    private fun displayName(uri: Uri): String? =
        contentResolver
            .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }

    private fun querySize(uri: Uri): Long =
        contentResolver
            .query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getLong(0) else 0L } ?: 0L

    /** Create the Rust session and render its notice stream. */
    private fun startSession(
        id: Long,
        spec: Spec,
        resume: Boolean,
    ) {
        lastSeq[id] = 0L
        val job =
            scope.launch {
                // Drain any residue BEFORE the transfer: a finalized file left
                // in staging (e.g. by a failed publish) makes the core's
                // existing-final path answer a resume-enabled send of that
                // file instantly - and invisibly, since the user only sees
                // Downloads. Field bug: room 104519. Must complete before the
                // session starts: a fire-and-forget sweep can lose the race.
                if (spec.dir() == Direction.Receive) {
                    withContext(Dispatchers.IO) { sweepStaging(spec.path, attributeTo = null) }
                }
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
    private fun onSnapshot(
        id: Long,
        spec: Spec,
        s: JSONObject,
    ) {
        val seq = s.optLong("seq")
        val prev = lastSeq[id] ?: 0L
        if (seq <= prev) return
        lastSeq[id] = seq

        val state = s.optString("state")
        val status =
            when (state) {
                "waiting" -> Status.Waiting
                "connecting" -> Status.Connecting
                "verifying" -> Status.Verifying
                "transferring" -> Status.Transferring
                "confirming" -> Status.Confirming
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
                attempt = s.optInt("attempt", t.attempt),
                proofDelivered =
                    s.optJSONObject("facts")?.optBoolean("proof_delivered")
                        ?: t.proofDelivered,
                transferId = s.optString("transfer_id").ifEmpty { t.transferId },
                fileName = s.optString("file_name").ifEmpty { t.fileName },
                bytes = bytes,
                total = if (total > 0) total else t.total,
                speedBps = if (status == Status.Transferring) speed else 0.0,
                avgBps = avg,
                speedHistory =
                    if (status == Status.Transferring && speed > 0) {
                        (t.speedHistory + speed).takeLast(90)
                    } else {
                        t.speedHistory
                    },
                pathType = path?.substringBefore(' ') ?: t.pathType,
                pathAddr =
                    path?.substringAfter('(', "")?.removeSuffix(")")?.ifEmpty { null }
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
            sweepStaging(spec.path, attributeTo = id)
        }
        // Active→resting transition: post the one final summary frame, then
        // detach so a stale ongoing notification can never linger.
        val nowActive = TransferRepository.transfers.value.any { isActive(it.status) }
        if (wasActive && !nowActive) {
            updateNotification()
            stopForeground(STOP_FOREGROUND_DETACH)
        }
        wasActive = nowActive
    }

    /** Human line for a state transition, for the card's log drawer. */
    private fun stateLogLine(
        state: String,
        s: JSONObject,
        bytes: Long,
    ): String =
        when (state) {
            "waiting" -> "waiting for peer…"
            "connecting" -> "pairing in room…"
            "verifying" -> "verifying…"
            "transferring" -> "started · ${s.optString("file_name")}"
            "confirming" -> "confirming delivery…"
            "paused" ->
                when (s.optString("origin")) {
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
    private fun onFetchReceipt(
        id: Long,
        key: String,
    ) {
        if (key.isEmpty()) return
        val server =
            SettingsStore.settings.value.logServer
                .trimEnd('/')
        if (server.isEmpty()) return
        scope.launch {
            val blob = LogUpload.getBytes("$server/receipts/$key")
            val b64 =
                blob?.let {
                    java.util.Base64
                        .getEncoder()
                        .encodeToString(it)
                } ?: ""
            Native.receiptResponse(id, key, b64)
        }
    }

    /** Courier: POST the sealed receipt blob (with backoff — the whole point
     *  of the mailbox is that this can retry long after the ack could not). */
    private fun onPostReceipt(
        id: Long,
        notice: JSONObject,
    ) {
        val key = notice.optString("key")
        val b64 = notice.optString("blob")
        if (key.isEmpty() || b64.isEmpty()) return
        val server =
            SettingsStore.settings.value.logServer
                .trimEnd('/')
        if (server.isEmpty()) return
        val bytes =
            runCatching {
                java.util.Base64
                    .getDecoder()
                    .decode(b64)
            }.getOrNull() ?: return
        scope.launch {
            for (backoff in listOf(0L, 5_000L, 30_000L)) {
                if (backoff > 0) delay(backoff)
                if (LogUpload.postBytes("$server/receipts/$key", bytes)) {
                    OpLog.add("receipt posted id=$id", id)
                    Native.sessionIntent(id, "receipt_posted")
                    return@launch
                }
            }
            OpLog.add("receipt post failed id=$id")
        }
    }

    /**
     * Publish EVERY finalized file in the staging dir to Downloads (the card's
     * own file plus any residue from an earlier failed publish), deleting the
     * staging copy on success. The staging dir must never retain finals: the
     * core treats an existing final as "already have it" and answers future
     * sends of that file instantly - correct for the CLI (a real output dir),
     * wrong for the app (staging is invisible; the user only sees Downloads).
     * [attributeTo] names the card that gets fileName/savedUri (completion
     * sweeps only; a start-of-session sweep publishes without attribution -
     * the residue belongs to some older card).
     *
     * DELETE WITH PHASE 4: this sweep only compensates SHARED receive staging.
     * Per-transfer incoming/<id>/ staging is empty by construction - nothing
     * to sweep, and the existing-final path cannot fire on residue.
     */
    private fun sweepStaging(
        outputDir: String,
        attributeTo: Long?,
    ) {
        val finals =
            File(outputDir)
                .listFiles { f -> f.isFile && !f.name.startsWith(".") } ?: return
        val s = SettingsStore.settings.value
        for (src in finals) {
            val uri =
                MediaStoreSaver.saveReceived(this, src, src.name, s.saveTreeUri, s.saveFolder)
                    ?: continue
            src.delete()
            if (attributeTo != null) {
                TransferRepository.update(attributeTo) {
                    if (it.fileName == null || it.fileName == src.name) {
                        it.copy(fileName = it.fileName ?: src.name, savedUri = uri.toString())
                    } else {
                        it
                    }
                }
            }
            LogStore.append("app: saved ${src.name} to Downloads")
        }
    }

    /** Active machine states pin the tray; everything else rests. */
    private fun isActive(st: Status) =
        st == Status.Waiting ||
            st == Status.Connecting ||
            st == Status.Verifying ||
            st == Status.Transferring ||
            st == Status.Confirming

    private fun arrow(t: Transfer) = if (t.direction == Direction.Send) "↑" else "↓"

    private fun trayWord(t: Transfer): String =
        when (t.status) {
            Status.Waiting -> "Waiting for peer"
            Status.Connecting -> "Pairing…"
            Status.Verifying -> "Verifying"
            Status.Confirming -> "Confirming"
            Status.Transferring ->
                if (t.total > 0) "${((t.bytes * 100) / t.total).toInt().coerceIn(0, 100)}%" else "…"
            Status.Paused -> "Paused"
            Status.Unconfirmed -> "Unconfirmed"
            Status.Completed -> "Done"
            Status.Failed -> "Failed"
            Status.Cancelled -> "Cancelled"
        }

    /**
     * The tray is a THIRD renderer of the repository cards (after the list and
     * the log): everything derived, zero tray-side state — so it can never
     * disagree with the cards (the old tray said "transferring" forever).
     */
    private fun notification(): Notification {
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val cards = TransferRepository.transfers.value
        val active = cards.filter { isActive(it.status) }
        val b =
            NotificationCompat
                .Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_upload)
                .setOngoing(active.isNotEmpty())
                .setOnlyAlertOnce(true)
                .setContentIntent(open)
        when {
            active.isEmpty() -> {
                // Final summary: outcomes, never a stale "transferring".
                val done = cards.count { it.status == Status.Completed }
                val paused = cards.count { it.status == Status.Paused || it.status == Status.Unconfirmed }
                val failed = cards.count { it.status == Status.Failed }
                val parts =
                    buildList {
                        if (done > 0) add("$done done")
                        if (paused > 0) add("$paused paused")
                        if (failed > 0) add("$failed failed")
                    }
                b.setContentTitle("Envoix").setContentText(
                    if (parts.isEmpty()) {
                        "No transfers"
                    } else {
                        "All transfers finished · ${parts.joinToString(", ")}"
                    },
                )
            }
            active.size == 1 -> {
                val t = active.single()
                b.setSmallIcon(
                    if (t.direction == Direction.Send) {
                        android.R.drawable.stat_sys_upload
                    } else {
                        android.R.drawable.stat_sys_download
                    },
                )
                val speed =
                    if (t.status == Status.Transferring && t.speedBps > 0) {
                        " · ${humanBytes(t.speedBps.toLong())}/s"
                    } else {
                        ""
                    }
                b.setContentTitle("${arrow(t)} ${t.fileName ?: "…"}")
                b.setContentText(trayWord(t) + speed)
                if (t.status == Status.Transferring && t.total > 0) {
                    b.setProgress(100, ((t.bytes * 100) / t.total).toInt().coerceIn(0, 100), false)
                } else {
                    b.setProgress(0, 0, true)
                }
            }
            else -> {
                val up = active.count { it.direction == Direction.Send }
                b.setContentTitle("${active.size} transfers · $up↑ ${active.size - up}↓")
                val style = NotificationCompat.InboxStyle()
                for (t in active.take(5)) {
                    val speed =
                        if (t.status == Status.Transferring && t.speedBps > 0) {
                            " · ${humanBytes(t.speedBps.toLong())}/s"
                        } else {
                            ""
                        }
                    style.addLine("${arrow(t)} ${t.fileName ?: "…"} · ${trayWord(t)}$speed")
                }
                b.setStyle(style)
            }
        }
        return b.build()
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
        private const val ACTION_RESTORE_ALL = "dev.envoix.app.RESTORE_ALL"
        private const val ACTION_REVERIFY = "dev.envoix.app.REVERIFY"

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
        private const val EXTRA_SOURCE_URI = "source_uri"
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
            sourceUri: String? = null,
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
                    putExtra(EXTRA_SOURCE_URI, sourceUri)
                },
            )
        }

        fun cancel(
            context: Context,
            id: Long,
        ) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_CANCEL
                    putExtra(EXTRA_ID, id)
                },
            )
        }

        /** Stop a transfer but keep its partial + spec, so it can be resumed. */
        fun pause(
            context: Context,
            id: Long,
        ) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_PAUSE
                    putExtra(EXTRA_ID, id)
                },
            )
        }

        /** Resume/retry: a live session bumps its attempt; a dead one is
         *  re-created from its spec with resume semantics. */
        fun resume(
            context: Context,
            id: Long,
        ) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_RESUME
                    putExtra(EXTRA_ID, id)
                },
            )
        }

        /** Serve a peer's re-verify from a Completed card (service, not resume). */
        fun reverify(
            context: Context,
            id: Long,
        ) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_REVERIFY
                    putExtra(EXTRA_ID, id)
                },
            )
        }

        /** Restore persisted transfer records into cards + idle sessions. */
        fun restoreAll(context: Context) {
            context.startService(
                Intent(context, TransferService::class.java).apply { action = ACTION_RESTORE_ALL },
            )
        }

        /** Remove the card AND its on-disk leftovers (D2: the one true abandon). */
        fun remove(
            context: Context,
            id: Long,
        ) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_REMOVE
                    putExtra(EXTRA_ID, id)
                },
            )
        }
    }
}
