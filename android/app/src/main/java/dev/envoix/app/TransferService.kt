package dev.envoix.app

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
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** Android projection of the canonical Manifest-v2 session. This service owns
 * foreground lifetime and platform save effects only; it does not select an
 * engine or reproduce the Rust reducer. */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val callbacks = ConcurrentHashMap<Long, ManifestCallback>()
    private val specs = ConcurrentHashMap<Long, ManifestSpec>()
    private val publisher by lazy { ManifestV2Publisher(this) }
    private val nextNativeAttemptId = AtomicLong(1)
    private val clock = SimpleDateFormat("HH:mm:ss", Locale.US)
    private var foreground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        readSpecs().forEach { specs[it.id] = it }
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Transfers", NotificationManager.IMPORTANCE_LOW),
        )
    }

    override fun onDestroy() {
        val activeAttempts = callbacks.values.map(ManifestCallback::nativeId)
        callbacks.clear()
        activeAttempts.forEach(Native::cancelManifestV2Session)
        scope.cancel()
        super.onDestroy()
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_START_SEND -> startNew(intent, Direction.Send)
            ACTION_START_RECEIVE -> startNew(intent, Direction.Receive)
            ACTION_APPROVE_RECEIVE -> continueReceive(
                intent.getLongExtra(EXTRA_ID, -1),
                exceptionalApproved = true,
            )
            ACTION_PAUSE -> pause(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_RESUME -> resume(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_CANCEL -> cancelTransfer(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_REMOVE -> removeTransfer(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_RESTORE -> restoreSessions()
        }
        return START_NOT_STICKY
    }

    private fun startNew(
        intent: Intent,
        direction: Direction,
    ) {
        val room = intent.getStringExtra(EXTRA_ROOM)?.takeIf(String::isNotBlank) ?: return
        val broker = intent.getStringExtra(EXTRA_BROKER)?.takeIf(String::isNotBlank) ?: return
        val relay = intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val id = TransferRepository.create(direction, room)
        val spec =
            ManifestSpec(
                id = id,
                direction = direction,
                room = room,
                broker = broker,
                relay = relay,
                jobId = intent.getStringExtra(EXTRA_JOB_ID),
                qrPayload = intent.getStringExtra(EXTRA_QR),
                copyAfterVerifyApproved = intent.getBooleanExtra(EXTRA_COPY_APPROVED, false),
                holdState = null,
            )
        if (direction == Direction.Send && spec.jobId.isNullOrBlank()) {
            TransferRepository.update(id) {
                it.copy(status = Status.Failed, error = "Prepared transfer job is missing")
            }
            return
        }
        specs[id] = spec
        persistSpecs()
        TransferRepository.update(id) {
            it.copy(
                status = Status.Connecting,
                qrPayload = spec.qrPayload,
                jobId = spec.jobId,
                log = addLog(it.log, "canonical Manifest v2 session started"),
            )
        }
        startNative(spec)
    }

    private fun startNative(spec: ManifestSpec) {
        enterForeground()
        val callback = ManifestCallback(spec.id, nextNativeAttemptId.getAndIncrement())
        callbacks[spec.id] = callback
        Native.startManifestV2Session(callback.nativeId, spec.paramsJson(this), callback)
        updateNotification()
    }

    private inner class ManifestCallback(
        private val id: Long,
        val nativeId: Long,
    ) : ManifestV2Callback {
        override fun onEvent(json: String) {
            if (callbacks[id] !== this) return
            val event = runCatching { JSONObject(json) }.getOrNull() ?: return
            if (event.optString("notice") != "manifest_v2") return
            when (event.optString("kind")) {
                "progress" -> {
                    TransferRepository.update(id) {
                        it.copy(bytes = event.optLong("bytes"), total = event.optLong("total"))
                    }
                    updateNotification()
                    return
                }
                "diagnostic" -> {
                    val message = event.optString("message")
                    if (message.isNotBlank()) {
                        TransferRepository.update(id) { it.copy(log = addLog(it.log, message)) }
                    }
                    return
                }
                "path" -> {
                    TransferRepository.update(id) { it.copy(pathAddr = event.optString("path")) }
                    return
                }
            }
            when (event.optString("state")) {
                "connecting" -> setState(id, Status.Connecting, "connecting to peer")
                "offer" -> onOffer(id, event, this)
                "transferring" -> setState(id, Status.Transferring, "transferring files")
                "receiving" -> setState(id, Status.Receiving, "receiving files")
                "verifying" -> setState(id, Status.Verifying, "verifying received content")
                "saving" -> setState(id, Status.Saving, "saving to selected destination")
                "waiting_for_receiver_save" ->
                    setState(id, Status.WaitingForReceiverSave, "waiting for receiver to save files")
                "received" -> setState(id, Status.Received, "files saved; confirming delivery")
                "completed" -> onCompleted(id, event, this)
                "failed" -> onFailed(id, event, this)
            }
        }

        override fun onSaveRequired(requestJson: String): String {
            check(callbacks[id] === this) { "Manifest v2 attempt is no longer active" }
            return publisher.save(requestJson)
        }
    }

    private fun onOffer(
        id: Long,
        offer: JSONObject,
        callback: ManifestCallback,
    ) {
        val spec = specs[id] ?: return
        val inventoryPage =
            runCatching {
                JSONObject(Native.listManifestV2OfferEntries(callback.nativeId, 0, 128))
            }.getOrNull()
        val projectedEntries =
            inventoryPage?.optJSONArray("entries")?.let { entries ->
                (0 until entries.length()).map { index ->
                    val entry = entries.getJSONObject(index)
                    TransferInventoryEntry(
                        entryId = entry.getInt("entry_id"),
                        parentEntryId =
                            entry.takeIf { it.has("parent_entry_id") && !it.isNull("parent_entry_id") }
                                ?.getInt("parent_entry_id"),
                        name = entry.getString("name"),
                        directory = entry.getString("kind") == "directory",
                        size = entry.getLong("plaintext_size"),
                    )
                }
            }.orEmpty()
        val exceptional = offer.optBoolean("exceptional")
        val hasDirectories = offer.optInt("directory_count") > 0
        val needsFolder = hasDirectories && SettingsStore.settings.value.saveTreeUri.isBlank()
        TransferRepository.update(id) {
            it.copy(
                status =
                    if (exceptional || needsFolder || !spec.copyAfterVerifyApproved) {
                        Status.AwaitingDecision
                    } else {
                        Status.Receiving
                    },
                jobId = offer.optString("job_id"),
                rootCount = offer.optInt("root_count"),
                fileCount = offer.optInt("file_count"),
                directoryCount = offer.optInt("directory_count"),
                total = offer.optLong("total"),
                exceptionalOffer = exceptional,
                inventoryPreview = projectedEntries,
                inventoryHasMore = inventoryPage?.has("next_offset") == true &&
                    !inventoryPage.isNull("next_offset"),
                error =
                    when {
                        needsFolder -> "Choose a writable save folder before receiving directories."
                        !spec.copyAfterVerifyApproved ->
                            "This Android destination requires private verification followed by an extra copy."
                        exceptional -> "Review this unusually large transfer before continuing."
                        else -> null
                    },
                log = addLog(it.log, "authenticated inventory received"),
            )
        }
        if (!exceptional && !needsFolder && spec.copyAfterVerifyApproved) {
            continueReceive(id, exceptionalApproved = false)
        }
        updateNotification()
    }

    private fun continueReceive(
        id: Long,
        exceptionalApproved: Boolean,
    ) {
        val spec = specs[id] ?: return
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (transfer.directoryCount > 0 && SettingsStore.settings.value.saveTreeUri.isBlank()) {
            TransferRepository.update(id) {
                it.copy(error = "Choose a writable save folder in Settings, then continue.")
            }
            return
        }
        val target = receiveTarget(id).apply { mkdirs() }
        val response =
            Native.continueManifestV2Receive(
                callbacks[id]?.nativeId ?: return,
                JSONObject()
                    .put("target_directory", target.absolutePath)
                    .put("target_allocatable_bytes", target.usableSpace)
                    .put(
                        "exceptional_transfer_approved",
                        exceptionalApproved || !transfer.exceptionalOffer,
                    ).toString(),
            )
        val error = runCatching { JSONObject(response).optString("error") }.getOrDefault("")
        if (error.isNotEmpty()) {
            TransferRepository.update(id) { it.copy(status = Status.Failed, error = error) }
        } else {
            specs[id] = spec.copy(copyAfterVerifyApproved = true)
            persistSpecs()
            setState(id, Status.Receiving, "destination decision committed")
        }
    }

    private fun onCompleted(
        id: Long,
        event: JSONObject,
        callback: ManifestCallback,
    ) {
        val roots = event.optJSONArray("roots") ?: JSONArray()
        val uris = (0 until roots.length()).map { roots.getJSONObject(it).getString("uri") }
        val names = (0 until roots.length()).map { roots.getJSONObject(it).getString("final_name") }
        TransferRepository.update(id) {
            it.copy(
                status = Status.Completed,
                bytes = it.total,
                savedUri = uris.firstOrNull(),
                savedUris = uris,
                publishedName = names.firstOrNull(),
                error = null,
                log = addLog(it.log, "delivered · receiver save proof acknowledged"),
            )
        }
        callbacks.remove(id, callback)
        specs.remove(id)
        persistSpecs()
        leaveForegroundIfIdle()
    }

    private fun onFailed(
        id: Long,
        event: JSONObject,
        callback: ManifestCallback,
    ) {
        val cause = event.optString("cause", "transfer")
        val detail = event.optString("detail", "Transfer failed")
        TransferRepository.update(id) {
            it.copy(
                status =
                    if (cause == "user_canceled" || cause == "sender_canceled") {
                        Status.Cancelled
                    } else {
                        Status.Failed
                    },
                failureCause = cause,
                error = explainFailure(cause, detail),
                log = addLog(it.log, "$cause · $detail"),
            )
        }
        callbacks.remove(id, callback)
        leaveForegroundIfIdle()
    }

    private fun pause(id: Long) {
        if (id < 0) return
        callbacks.remove(id)?.let { Native.cancelManifestV2Session(it.nativeId) }
        specs[id]?.let { specs[id] = it.copy(holdState = "paused") }
        persistSpecs()
        TransferRepository.update(id) {
            if (it.status.isTerminal) it else it.copy(status = Status.Paused, error = null)
        }
        leaveForegroundIfIdle()
    }

    private fun resume(id: Long) {
        val spec = specs[id]?.copy(holdState = null) ?: return
        if (callbacks.containsKey(id)) return
        specs[id] = spec
        persistSpecs()
        TransferRepository.update(id) { it.copy(status = Status.Connecting, error = null) }
        startNative(spec)
    }

    private fun cancelTransfer(id: Long) {
        callbacks.remove(id)?.let { Native.cancelManifestV2Session(it.nativeId) }
        specs[id]?.let { specs[id] = it.copy(holdState = "canceled") }
        persistSpecs()
        TransferRepository.update(id) {
            if (it.status.isTerminal) it else it.copy(status = Status.Cancelled, error = null)
        }
        leaveForegroundIfIdle()
    }

    private fun removeTransfer(id: Long) {
        callbacks.remove(id)?.let { Native.cancelManifestV2Session(it.nativeId) }
        val spec = specs.remove(id)
        // Only job-owned private/incomplete artifacts are discarded. Public
        // saved URIs returned by the result gate are never deleted here.
        receiveBase(id).deleteRecursively()
        spec?.jobId?.let { File(filesDir, "manifest-v2/source-staging/$it").deleteRecursively() }
        persistSpecs()
        TransferRepository.remove(id)
        leaveForegroundIfIdle()
    }

    private fun restoreSessions() {
        specs.values.sortedBy(ManifestSpec::id).forEach { spec ->
            TransferRepository.restoreCard(
                spec.id,
                spec.direction,
                spec.room,
                qrPayload = spec.qrPayload,
            )
            TransferRepository.update(spec.id) {
                it.copy(
                    status = if (spec.holdState == "canceled") Status.Cancelled else if (spec.holdState == "paused") Status.Paused else Status.Connecting,
                    jobId = spec.jobId,
                )
            }
            if (spec.holdState == null && !callbacks.containsKey(spec.id)) startNative(spec)
        }
    }

    private fun setState(
        id: Long,
        status: Status,
        log: String,
    ) {
        TransferRepository.update(id) {
            it.copy(status = status, error = null, log = addLog(it.log, log))
        }
        updateNotification()
    }

    private fun addLog(
        current: List<String>,
        message: String,
    ): List<String> = (current + "${clock.format(Date())}  $message").takeLast(TransferRepository.LOG_CAP)

    private fun explainFailure(
        cause: String,
        detail: String,
    ): String =
        when (cause) {
            "sender_permission_lost" -> "The sender lost permission to read a selected item. Reauthorize it and retry."
            "sender_source_changed" -> "A selected item changed after preparation. Review and send it again."
            "receiver_space_insufficient" -> "The receiver does not have enough space for this transfer."
            "receiver_destination_decision_required" -> "The receiver must choose or approve a save destination."
            "receiver_destination_unavailable" -> "The selected receive destination is no longer available."
            "receiver_save_failed" -> "The receiver could not save the verified files: $detail"
            "protocol_or_integrity_failure" -> "Integrity verification failed; no unverified file was delivered."
            "transport" -> "The connection was interrupted. Resume to continue from verified data."
            else -> detail
        }

    private fun receiveBase(id: Long) = File(filesDir, "manifest-v2/receiver/$id")
    private fun receiveTarget(id: Long) = File(receiveBase(id), "final")
    private fun stateDirectory(id: Long) = File(receiveBase(id), "state")

    private fun ManifestSpec.paramsJson(context: Context): String =
        JSONObject()
            .put("direction", if (direction == Direction.Send) "send" else "receive")
            .put("room", room)
            .put("broker", broker)
            .put("relay", relay)
            .put("state_directory", stateDirectory(id).apply { mkdirs() }.absolutePath)
            .put("job_store_directory", jobStoreDirectory(context).absolutePath)
            .apply { jobId?.let { put("job_id", it) } }
            .toString()

    @Synchronized
    private fun persistSpecs() {
        val values = JSONArray()
        specs.values.sortedBy(ManifestSpec::id).forEach { values.put(it.toJson()) }
        val file = specFile(this)
        file.parentFile?.mkdirs()
        val temporary = File(file.parentFile, "${file.name}.tmp")
        temporary.writeText(values.toString())
        runCatching {
            Files.move(
                temporary.toPath(),
                file.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
                StandardCopyOption.ATOMIC_MOVE,
            )
        }.getOrElse {
            Files.move(
                temporary.toPath(),
                file.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    private fun readSpecs(): List<ManifestSpec> =
        runCatching {
            val values = JSONArray(specFile(this).readText())
            (0 until values.length()).map { ManifestSpec.fromJson(values.getJSONObject(it)) }
        }.getOrDefault(emptyList())

    private fun enterForeground() {
        if (!foreground) {
            startForeground(
                NOTIFICATION_ID,
                notification("Preparing transfer…"),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
            foreground = true
        }
    }

    private fun updateNotification() {
        if (!foreground) return
        val active = TransferRepository.transfers.value.filterNot { it.status.isTerminal }
        val text = active.lastOrNull()?.let { statusLabel(it.status) } ?: "No active transfer"
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(text))
    }

    private fun leaveForegroundIfIdle() {
        if (TransferRepository.activeCount() == 0 && foreground) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            foreground = false
            stopSelf()
        } else {
            updateNotification()
        }
    }

    private fun notification(text: String) =
        NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Envoix")
            .setContentText(text)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .setContentIntent(
                PendingIntent.getActivity(
                    this,
                    0,
                    Intent(this, MainActivity::class.java),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            ).build()

    private fun statusLabel(status: Status): String =
        when (status) {
            Status.Preparing -> "Preparing files…"
            Status.Connecting -> "Connecting…"
            Status.AwaitingDecision -> "Waiting for your save decision"
            Status.Transferring -> "Transferring files…"
            Status.Receiving -> "Receiving files…"
            Status.Verifying -> "Verifying…"
            Status.Saving -> "Saving…"
            Status.WaitingForReceiverSave -> "Waiting for receiver to save…"
            Status.Received -> "Received; confirming delivery…"
            Status.Paused -> "Paused"
            Status.Completed -> "Completed"
            Status.Failed -> "Failed"
            Status.Cancelled -> "Cancelled"
        }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIFICATION_ID = 1001
        private const val ACTION_START_SEND = "dev.envoix.app.manifest_v2.START_SEND"
        private const val ACTION_START_RECEIVE = "dev.envoix.app.manifest_v2.START_RECEIVE"
        private const val ACTION_APPROVE_RECEIVE = "dev.envoix.app.manifest_v2.APPROVE_RECEIVE"
        private const val ACTION_PAUSE = "dev.envoix.app.manifest_v2.PAUSE"
        private const val ACTION_RESUME = "dev.envoix.app.manifest_v2.RESUME"
        private const val ACTION_CANCEL = "dev.envoix.app.manifest_v2.CANCEL"
        private const val ACTION_REMOVE = "dev.envoix.app.manifest_v2.REMOVE"
        private const val ACTION_RESTORE = "dev.envoix.app.manifest_v2.RESTORE"
        private const val EXTRA_ID = "id"
        private const val EXTRA_ROOM = "room"
        private const val EXTRA_BROKER = "broker"
        private const val EXTRA_RELAY = "relay"
        private const val EXTRA_QR = "qr"
        private const val EXTRA_JOB_ID = "job_id"
        private const val EXTRA_COPY_APPROVED = "copy_after_verify_approved"

        fun startSend(
            context: Context,
            room: String,
            broker: String,
            relay: String,
            jobId: String,
            qrPayload: String?,
        ) = launch(
            context,
            ACTION_START_SEND,
            room,
            broker,
            relay,
            qrPayload,
            jobId,
            copyApproved = false,
        )

        fun startReceive(
            context: Context,
            room: String,
            broker: String,
            relay: String,
            qrPayload: String?,
            copyAfterVerifyApproved: Boolean,
        ) = launch(
            context,
            ACTION_START_RECEIVE,
            room,
            broker,
            relay,
            qrPayload,
            jobId = null,
            copyApproved = copyAfterVerifyApproved,
        )

        private fun launch(
            context: Context,
            action: String,
            room: String,
            broker: String,
            relay: String,
            qrPayload: String?,
            jobId: String?,
            copyApproved: Boolean,
        ) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    this.action = action
                    putExtra(EXTRA_ROOM, room)
                    putExtra(EXTRA_BROKER, broker)
                    putExtra(EXTRA_RELAY, relay)
                    putExtra(EXTRA_QR, qrPayload)
                    putExtra(EXTRA_JOB_ID, jobId)
                    putExtra(EXTRA_COPY_APPROVED, copyApproved)
                },
            )
        }

        fun approveReceive(context: Context, id: Long) = command(context, ACTION_APPROVE_RECEIVE, id)
        fun pause(context: Context, id: Long) = command(context, ACTION_PAUSE, id)
        fun resume(context: Context, id: Long) = command(context, ACTION_RESUME, id)
        fun cancel(context: Context, id: Long) = command(context, ACTION_CANCEL, id)
        fun remove(context: Context, id: Long) = command(context, ACTION_REMOVE, id)
        fun restoreAll(context: Context) = command(context, ACTION_RESTORE, -1)

        private fun command(
            context: Context,
            action: String,
            id: Long,
        ) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    this.action = action
                    putExtra(EXTRA_ID, id)
                },
            )
        }

        fun jobStoreDirectory(context: Context): File =
            File(context.filesDir, "manifest-v2/jobs").apply { mkdirs() }

        fun nextSessionIdFloor(context: Context): Long =
            runCatching {
                val values = JSONArray(specFile(context).readText())
                (0 until values.length()).maxOfOrNull { values.getJSONObject(it).getLong("id") }?.plus(1)
                    ?: 1L
            }.getOrDefault(1L)

        private fun specFile(context: Context) = File(context.filesDir, "manifest-v2/android-sessions.json")
    }
}

private data class ManifestSpec(
    val id: Long,
    val direction: Direction,
    val room: String,
    val broker: String,
    val relay: String,
    val jobId: String?,
    val qrPayload: String?,
    val copyAfterVerifyApproved: Boolean,
    val holdState: String?,
) {
    fun toJson(): JSONObject =
        JSONObject()
            .put("id", id)
            .put("direction", direction.name.lowercase())
            .put("room", room)
            .put("broker", broker)
            .put("relay", relay)
            .put("job_id", jobId)
            .put("qr", qrPayload)
            .put("copy_after_verify_approved", copyAfterVerifyApproved)
            .put("hold_state", holdState)

    companion object {
        fun fromJson(value: JSONObject) =
            ManifestSpec(
                id = value.getLong("id"),
                direction = if (value.getString("direction") == "send") Direction.Send else Direction.Receive,
                room = value.getString("room"),
                broker = value.getString("broker"),
                relay = value.optString("relay"),
                jobId = value.optString("job_id").takeIf(String::isNotEmpty),
                qrPayload = value.optString("qr").takeIf(String::isNotEmpty),
                copyAfterVerifyApproved = value.optBoolean("copy_after_verify_approved"),
                holdState = value.optString("hold_state").takeIf(String::isNotEmpty),
            )
    }
}
