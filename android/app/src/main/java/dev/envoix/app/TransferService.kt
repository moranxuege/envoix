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
    /** Receipt-mailbox endpoint frozen at creation; persisted in the record's
     *  context so confirmation survives later edits to the setting. */
    val receiptServer: String = "",
    /** Staging send only: the content:// source to copy into [path], and
     *  whether a durable read grant was taken (so restore knows if it can
     *  re-stage). Both ride platform_extras, so they survive restarts. */
    val sourceUri: String? = null,
    val sourceRecoverable: Boolean = false,
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
                put("receipt_server", receiptServer)
                val extras = org.json.JSONObject()
                qrPayload?.let { extras.put("qr", it) }
                sourceUri?.let {
                    extras.put("source_uri", it)
                    extras.put("source_recoverable", sourceRecoverable)
                }
                if (extras.length() > 0) put("platform_extras", extras)
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

    /** One scope per card: staging copy, session collector, and courier calls
     *  all run inside it, so Remove fences a card's ENTIRE async surface with
     *  a single cancel (no hidden session can start after, no receipt retry
     *  survives). Parented to the service scope - teardown cancels all. */
    private val transferScopes = HashMap<Long, CoroutineScope>()

    @Synchronized
    private fun transferScope(id: Long): CoroutineScope =
        transferScopes.getOrPut(id) {
            CoroutineScope(scope.coroutineContext + SupervisorJob(scope.coroutineContext[Job]))
        }

    @Synchronized
    private fun cancelTransferScope(id: Long) {
        transferScopes.remove(id)?.cancel()
    }

    /** Collector jobs per transfer id (live Rust session ⇔ live job). */
    private val jobs = java.util.concurrent.ConcurrentHashMap<Long, Job>()

    /** Last applied snapshot seq per id: out-of-order snapshots are dropped. */
    private val lastSeq = java.util.concurrent.ConcurrentHashMap<Long, Long>()

    /** Pump generation currently owning each card (stamped into every JNI
     *  notice). The active collector claims it on its first notice; anything
     *  else is a stale pump from a torn-down session for the same id and is
     *  dropped - the fence is explicit, not an artifact of flow mechanics. */
    private val generations = HashMap<Long, Long>()

    /** Ids whose staging copy has been launched, so the Preparing snapshot
     *  triggers it exactly once. */
    private val stagingStarted = HashSet<Long>()

    /** True when [notice] belongs to the card's current pump (claiming it if
     *  the card is unclaimed). */
    @Synchronized
    private fun ownsCard(
        id: Long,
        notice: JSONObject,
    ): Boolean {
        val gen = notice.optLong("gen", 0L)
        if (gen <= 0L) return true // pre-generation core: let it through
        val current = generations[id]
        if (current == null) {
            generations[id] = gen
            return true
        }
        return gen == current
    }

    /** Foreground is platform STATE, reconciled against the snapshot stream -
     *  not a history edge. (The old active->resting latch never fired for a
     *  card born terminal, e.g. a sync launch failure: the service stayed
     *  foreground with a stale ongoing notification.) */
    private var isForeground = false

    /** Cards currently holding a multicast ref (the lock is ref-counted). */
    private val multicastHolders = HashSet<Long>()

    /** The multicast lock derives from the OBSERVED state, not the launch
     *  path: held exactly while an mDNS-capable card is active (it disables
     *  radio multicast filtering - battery). Restore reconciles automatically:
     *  the first restored snapshot flows through here like any other. */
    @Synchronized
    private fun renderMulticast(
        id: Long,
        spec: Spec,
        status: Status,
    ) {
        val want = spec.useMdns && isActive(status)
        val holds = id in multicastHolders
        if (want && !holds) {
            runCatching { multicastLock.acquire() }
            multicastHolders.add(id)
        } else if (!want && holds) {
            multicastHolders.remove(id)
            runCatching { multicastLock.release() }
        }
    }

    /** Safety release when a collector dies without a resting snapshot. */
    @Synchronized
    private fun releaseMulticast(id: Long) {
        if (multicastHolders.remove(id)) runCatching { multicastLock.release() }
    }

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
        gcStaging()
    }

    /** The per-card receive staging dir (Phase 4): `filesDir/incoming/<id>/`. */
    private fun receiveStagingDir(id: Long) = File(File(filesDir, "incoming"), id.toString())

    /**
     * Reconcile staging with the record store at service start, before any
     * action can run (onCreate precedes onStartCommand, so no session exists
     * yet and nothing races the deletes):
     * - per-id staging dirs with no record are crash residue - delete;
     * - files directly in the incoming root are pre-Phase-4 shared-staging
     *   leftovers: publish finals (the old pre-receive sweep's recovery
     *   duty, one last time), drop sidecars. Runs async - new transfers
     *   never touch the root anymore.
     */
    private fun gcStaging() {
        var recordIds = emptySet<Long>()
        var legacyRootInUse = false
        val incoming = File(filesDir, "incoming")
        runCatching {
            val ctxs = org.json.JSONArray(Native.listRestoreContexts())
            recordIds =
                (0 until ctxs.length())
                    .mapNotNull { ctxs.optJSONObject(it)?.optLong("id", -1L)?.takeIf { id -> id >= 0 } }
                    .toSet()
            // Pre-Phase-4 records point straight at the shared root; their
            // artifacts live there and are NOT garbage while the record does.
            legacyRootInUse =
                (0 until ctxs.length()).any {
                    ctxs.optJSONObject(it)?.optString("path") == incoming.absolutePath
                }
        }
        incoming.listFiles { f -> f.isDirectory }?.forEach { dir ->
            if (dir.name.toLongOrNull() !in recordIds) {
                dir.deleteRecursively()
                OpLog.add("gc: dropped orphan staging ${dir.name}")
            }
        }
        val send = File(cacheDir, "send")
        send.listFiles { f -> f.isDirectory }?.forEach { dir ->
            if (dir.name.toLongOrNull() !in recordIds) dir.deleteRecursively()
        }
        if (!legacyRootInUse) {
            scope.launch(Dispatchers.IO) {
                sweepStaging(incoming.absolutePath, attributeTo = null)
                incoming.listFiles { f -> f.isFile }?.forEach { it.delete() }
            }
        }
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
                val spec0 =
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
                        SettingsStore.settings.value.logServer,
                    )
                enterForeground()
                val id = TransferRepository.create(spec0.dir(), room)
                // Receive staging is keyed by card id (Phase 4): a fresh dir
                // is empty by construction, so the core's existing-final path
                // cannot fire on another card's residue, and resume/receipt
                // artifacts flow only through the card that owns them.
                val spec =
                    if (spec0.dir() == Direction.Receive) {
                        spec0.copy(path = receiveStagingDir(id).absolutePath)
                    } else {
                        spec0
                    }
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
                    val uri = Uri.parse(sourceUri)
                    // The provider's DISPLAY_NAME is untrusted (can contain path
                    // separators): keep only the leaf, never a dot name.
                    val name =
                        (displayName(uri) ?: "upload.bin")
                            .let { File(it).name }
                            .takeUnless { it.isEmpty() || it == "." || it == ".." }
                            ?: "upload.bin"
                    // A durable read grant lets a restart re-stage the source.
                    val recoverable =
                        runCatching {
                            contentResolver.takePersistableUriPermission(
                                uri,
                                Intent.FLAG_GRANT_READ_URI_PERMISSION,
                            )
                        }.isSuccess
                    // Staging is keyed by card id so two same-named sends never
                    // share a source path (the sender hashes as it reads).
                    val stagingPath =
                        File(File(File(cacheDir, "send"), id.toString()), name).absolutePath
                    TransferRepository.update(id) {
                        it.copy(
                            fileName = name,
                            total = querySize(uri),
                            log = addLog(it.log, "preparing · staging $name…"),
                        )
                    }
                    val stagingSpec =
                        spec.copy(path = stagingPath, sourceUri = sourceUri, sourceRecoverable = recoverable)
                    specs[id] = stagingSpec
                    // Record commits in Preparing FIRST; the copy launches from
                    // the Preparing snapshot (durable intent before a byte moves).
                    startSession(id, stagingSpec, resume = false, staging = true)
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
                // Fence the card's async surface FIRST: staging copy, session
                // collector, courier retries all die here, so nothing can
                // start a hidden session or re-create files during cleanup.
                // (A blocking copy may outlive the signal briefly; its dir is
                // orphaned then and the startup GC reaps it.)
                cancelTransferScope(id)
                // D2, the one true abandon: discard partial + resume state +
                // receipt, then tear the session and card down. Both staging
                // dirs are keyed by card id, so they go too without consulting
                // the (possibly already gone) spec.
                File(File(cacheDir, "send"), id.toString()).deleteRecursively()
                receiveStagingDir(id).deleteRecursively()
                Native.destroySession(id, true)
                TransferLogs.delete(id)
                jobs.remove(id)
                specs.remove(id)
                lastSeq.remove(id)
                generations.remove(id)
                synchronized(stagingStarted) { stagingStarted.remove(id) }
                TransferRepository.remove(id)
                stopIfIdle()
            }
        }
        return START_NOT_STICKY
    }

    private fun enterForeground() {
        startForeground(NOTIF_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        isForeground = true
    }

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
        val ctxs = runCatching { org.json.JSONArray(Native.listRestoreContexts()) }.getOrNull() ?: return
        for (i in 0 until ctxs.length()) {
            val c = ctxs.optJSONObject(i) ?: continue
            val id = c.optLong("id", -1L)
            if (id < 0 || jobs.containsKey(id)) continue
            val direction = c.optString("direction")
            val code = c.optString("code")
            if (code.isEmpty()) continue
            if (!TransferRepository.restoreCard(
                    id,
                    if (direction == "send") Direction.Send else Direction.Receive,
                    code,
                    qrPayload = c.optString("qr").ifEmpty { null },
                    savedUri = c.optString("saved_uri").ifEmpty { null },
                )
            ) {
                continue
            }
            // Transport config (broker/relay/chunk/candidates) is unused for a
            // restored session - the core relaunches from the durable record's
            // own context - so the display Spec carries only what the card and
            // platform effects need.
            // A restored Preparing send re-stages only if its source grant was
            // durable; otherwise the copy path fails it with "needs re-picking".
            val recoverable = c.optBoolean("source_recoverable", false)
            val spec =
                Spec(
                    direction,
                    code,
                    c.optString("path"),
                    Endpoints.BROKER,
                    Endpoints.RELAY,
                    "",
                    "",
                    "",
                    null,
                    c.optBoolean("use_room"),
                    c.optBoolean("use_mdns"),
                    sourceUri = if (recoverable) c.optString("source_uri").ifEmpty { null } else null,
                    sourceRecoverable = recoverable,
                )
            specs[id] = spec
            lastSeq[id] = 0L
            generations.remove(id)
            val job =
                transferScope(id).launch {
                    try {
                        NativeSession.restore(id).collect { notice ->
                            if (!ownsCard(id, notice)) return@collect
                            when (notice.optString("notice")) {
                                "snapshot" -> onSnapshot(id, spec, notice)
                                "fetch_receipt" ->
                                    onFetchReceipt(id, notice.optString("key"), notice.optString("server"))
                                "post_receipt" -> onPostReceipt(id, notice)
                            }
                        }
                    } finally {
                        releaseMulticast(id)
                    }
                }
            jobs[id] = job
            OpLog.add("restored transfer id=$id")
        }
    }

    /** Copy a Preparing send's content:// source into its staging path, then
     *  report to the core (stage_complete / stage_failed). Launched from the
     *  Preparing snapshot, so the record is already durable. */
    private fun launchStaging(
        id: Long,
        spec: Spec,
    ) {
        transferScope(id).launch(Dispatchers.IO) {
            val uri = spec.sourceUri?.let { Uri.parse(it) }
            if (uri == null) {
                // A restored Preparing whose source cannot be reopened.
                Native.stageFailed(id, "source needs re-picking")
                return@launch
            }
            val out = File(spec.path)
            out.parentFile?.mkdirs()
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
                                    Native.stageProgress(id, copied)
                                }
                            }
                        }
                    }
                }.isSuccess
            if (ok) {
                Native.stageComplete(id)
            } else {
                out.delete()
                Native.stageFailed(id, "couldn't read the picked file")
            }
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
        staging: Boolean = false,
    ) {
        lastSeq[id] = 0L
        generations.remove(id)
        val notices =
            if (staging) {
                NativeSession.startStaging(id, spec.paramsJson(resume = false))
            } else {
                NativeSession.start(id, spec.paramsJson(resume))
            }
        val job =
            transferScope(id).launch {
                try {
                    notices.collect { notice ->
                        if (!ownsCard(id, notice)) return@collect
                        when (notice.optString("notice")) {
                            "snapshot" -> onSnapshot(id, spec, notice)
                            "fetch_receipt" ->
                                onFetchReceipt(id, notice.optString("key"), notice.optString("server"))
                            "post_receipt" -> onPostReceipt(id, notice)
                        }
                    }
                } finally {
                    releaseMulticast(id)
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
                "preparing" -> Status.Preparing
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

        // Platform effects derive from the observed snapshot (design rule):
        // the same code path serves fresh starts, restores, and cards born
        // terminal - there is no launch-path special case to fall out of sync.
        renderMulticast(id, spec, status)
        // The record is now committed (this snapshot proves it), so the copy
        // never runs ahead of the durable intent. Guarded to fire once.
        if (status == Status.Preparing && spec.dir() == Direction.Send) {
            synchronized(stagingStarted) {
                if (stagingStarted.add(id)) launchStaging(id, spec)
            }
        }
        when {
            entered != null -> updateNotification()
            else -> throttledNotification()
        }
        if (state == "completed" && spec.dir() == Direction.Receive) {
            sweepStaging(spec.path, attributeTo = id)
        }
        // Foreground reconciles against the whole card set: when nothing is
        // active, post the one final summary frame and detach - including for
        // a card whose FIRST snapshot is already terminal.
        val nowActive = TransferRepository.transfers.value.any { isActive(it.status) }
        if (isForeground && !nowActive) {
            updateNotification()
            stopForeground(STOP_FOREGROUND_DETACH)
            isForeground = false
        }
    }

    /** Human line for a state transition, for the card's log drawer. */
    private fun stateLogLine(
        state: String,
        s: JSONObject,
        bytes: Long,
    ): String =
        when (state) {
            "preparing" -> "preparing · staging the source…"
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
        durableServer: String,
    ) {
        if (key.isEmpty()) return
        // The driver's notice carries the endpoint the transfer was created
        // with; the mutable setting is only the fallback for pre-field records.
        val server =
            durableServer
                .ifEmpty { SettingsStore.settings.value.logServer }
                .trimEnd('/')
        if (server.isEmpty()) return
        transferScope(id).launch {
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
            notice
                .optString("server")
                .ifEmpty { SettingsStore.settings.value.logServer }
                .trimEnd('/')
        if (server.isEmpty()) return
        val bytes =
            runCatching {
                java.util.Base64
                    .getDecoder()
                    .decode(b64)
            }.getOrNull() ?: return
        transferScope(id).launch {
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
     * Publish every finalized file in one staging dir to Downloads, deleting
     * the staging copy on success (receipt sidecars stay - they re-confirm a
     * lost CompleteAck after the file is published away). With per-card
     * staging (Phase 4) the dir holds only [attributeTo]'s own artifacts; the
     * unattributed call in [gcStaging] is the one legacy exception, draining
     * pre-Phase-4 shared-staging residue.
     */
    private fun sweepStaging(
        outputDir: String,
        attributeTo: Long?,
    ) {
        val finals =
            File(outputDir)
                .listFiles { f -> f.isFile && !f.name.startsWith(".") } ?: return
        for (src in finals) publishOne(src, attributeTo)
    }

    /** The publish sidecar journal for one staged file: `.envoix-publish.<name>.json`
     *  beside it, holding the reserved target URI (written before the copy) and
     *  the committed URI (written after). Lets a crash mid-publish recover:
     *  drop a half-written candidate, or adopt an already-committed one. */
    private fun publishJournal(src: File) = File(src.parentFile, ".envoix-publish.${src.name}.json")

    /**
     * Publish one finalized staging file, journaled. Recovery first: a surviving
     * journal means a prior publish was interrupted — adopt its committed target
     * (if it still resolves) or delete the half-written candidate — then a fresh
     * reserve → copy → commit → delete-staging, recording each step first.
     */
    private fun publishOne(
        src: File,
        attributeTo: Long?,
    ) {
        val journal = publishJournal(src)
        // --- recovery: a journal survived a crash mid-publish ---
        runCatching { org.json.JSONObject(journal.readText()) }.getOrNull()?.let { prior ->
            val committed = prior.optString("committed_uri").ifEmpty { null }
            if (committed != null && MediaStoreSaver.resolves(this, Uri.parse(committed))) {
                // Commit had landed; the crash was before staging was cleared.
                adopt(attributeTo, src.name, committed)
                src.delete()
                journal.delete()
                LogStore.append("app: adopted already-published ${src.name}")
                return
            }
            // Reserved but never committed (or the user deleted it): drop the
            // half-written candidate so we do not leave a truncated file, then
            // fall through to a fresh publish.
            prior.optString("target").ifEmpty { null }?.let { MediaStoreSaver.delete(this, Uri.parse(it)) }
            journal.delete()
        }

        // --- fresh publish ---
        val s = SettingsStore.settings.value
        val target = MediaStoreSaver.reserve(this, src.name, s.saveTreeUri, s.saveFolder) ?: return
        // Record the reservation BEFORE any byte is copied.
        writePublishJournal(journal, target.uri.toString(), target.mediaStorePending, committed = null)
        if (!MediaStoreSaver.copyInto(this, src, target)) {
            MediaStoreSaver.delete(this, target.uri)
            journal.delete()
            return
        }
        MediaStoreSaver.commit(this, target)
        // Record the commit BEFORE clearing staging, so a crash here recovers
        // by adopting (never re-publishing = duplicate).
        writePublishJournal(journal, target.uri.toString(), target.mediaStorePending, committed = target.uri.toString())
        adopt(attributeTo, src.name, target.uri.toString())
        src.delete()
        journal.delete()
        LogStore.append("app: saved ${src.name} to Downloads")
    }

    private fun writePublishJournal(
        journal: File,
        target: String,
        pending: Boolean,
        committed: String?,
    ) {
        val obj =
            org.json
                .JSONObject()
                .put("target", target)
                .put("pending", pending)
        committed?.let { obj.put("committed_uri", it) }
        runCatching { journal.writeText(obj.toString()) }
    }

    /** Attribute a published URI to its card (savedUri), and durable extras. */
    private fun adopt(
        attributeTo: Long?,
        name: String,
        uri: String,
    ) {
        if (attributeTo == null) return
        TransferRepository.update(attributeTo) {
            if (it.fileName == null || it.fileName == name) {
                it.copy(fileName = it.fileName ?: name, savedUri = uri)
            } else {
                it
            }
        }
        syncExtras(attributeTo)
    }

    /** Push the card's platform context (QR payload, saved URI) into the
     *  transfer's durable record, so it survives restarts. */
    private fun syncExtras(id: Long) {
        val t = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        val extras = org.json.JSONObject()
        t.qrPayload?.let { extras.put("qr", it) }
        t.savedUri?.let { extras.put("saved_uri", it) }
        Native.setSessionExtras(id, extras.toString())
    }

    /** Active machine states pin the tray; everything else rests. */
    private fun isActive(st: Status) =
        st == Status.Preparing ||
            st == Status.Waiting ||
            st == Status.Connecting ||
            st == Status.Verifying ||
            st == Status.Transferring ||
            st == Status.Confirming

    private fun arrow(t: Transfer) = if (t.direction == Direction.Send) "↑" else "↓"

    private fun trayWord(t: Transfer): String =
        when (t.status) {
            Status.Preparing -> "Preparing…"
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
        // Direction-specific status-bar icon: up-only for sends, down-only for
        // receives, both arrows only when genuinely sending AND receiving, a
        // checkmark once everything is done.
        val icon =
            when {
                active.isEmpty() -> R.drawable.ic_stat_done
                active.any { it.direction == Direction.Send } &&
                    active.any { it.direction == Direction.Receive } -> R.drawable.ic_stat_transfer
                active.first().direction == Direction.Send -> R.drawable.ic_stat_upload
                else -> R.drawable.ic_stat_download
            }
        val b =
            NotificationCompat
                .Builder(this, CHANNEL)
                .setSmallIcon(icon)
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
        isForeground = false
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
