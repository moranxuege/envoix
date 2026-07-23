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
import dev.envoix.app.ui.AppText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** Android projection of the canonical Manifest-v2 session. This service owns
 * foreground lifetime and platform save effects only; it does not select an
 * engine or reproduce the Rust reducer. */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val callbacks = ConcurrentHashMap<Long, ManifestCallback>()
    private val specs = ConcurrentHashMap<Long, ManifestSpec>()
    private val destinationWriter by lazy { ManifestV2DestinationWriter(this) }
    private val nextNativeAttemptId = AtomicLong(1)
    private val clock =
        DateTimeFormatter
            .ofPattern("HH:mm:ss")
            .withZone(ZoneId.systemDefault())
    private var foreground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        readSpecs().forEach { specs[it.id] = it }
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                uiText("Transfers", "传输"),
                NotificationManager.IMPORTANCE_LOW,
            ),
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
            ACTION_APPROVE_RECEIVE ->
                continueReceive(
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
        val broker = intent.getStringExtra(EXTRA_BROKER).orEmpty()
        val relay = intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val useRoom = intent.getBooleanExtra(EXTRA_USE_ROOM, true)
        val useMdns = intent.getBooleanExtra(EXTRA_USE_MDNS, true)
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
                destinationCopyApproved = intent.getBooleanExtra(EXTRA_COPY_APPROVED, false),
                useRoom = useRoom,
                useMdns = useMdns,
                holdState = null,
            )
        if (!useRoom && !useMdns) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText("Choose at least one available pairing route", "请至少选择一种可用的配对方式"),
                )
            }
            return
        }
        if (useRoom && broker.isBlank()) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText("Room pairing requires a rendezvous broker", "配对房间需要配置会合服务器"),
                )
            }
            return
        }
        if (direction == Direction.Send && spec.jobId.isNullOrBlank()) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText("Prepared transfer job is missing", "缺少已准备的传输任务"),
                )
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
                "finalizing_delivery" -> setState(id, Status.FinalizingDelivery, "saved; finalizing delivery proof")
                "completed" -> onCompleted(id, event, this)
                "failed" -> onFailed(id, event, this)
            }
        }

        override fun onSaveRequired(requestJson: String): String {
            check(callbacks[id] === this) { "Manifest v2 attempt is no longer active" }
            return destinationWriter.save(requestJson)
        }

        override fun onPlanRequired(requestJson: String): String {
            check(callbacks[id] === this) { "Manifest v2 attempt is no longer active" }
            return destinationWriter.plan(requestJson)
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
            inventoryPage
                ?.optJSONArray("entries")
                ?.let { entries ->
                    (0 until entries.length()).map { index ->
                        val entry = entries.getJSONObject(index)
                        TransferInventoryEntry(
                            entryId = entry.getInt("entry_id"),
                            parentEntryId =
                                entry
                                    .takeIf { it.has("parent_entry_id") && !it.isNull("parent_entry_id") }
                                    ?.getInt("parent_entry_id"),
                            name = entry.getString("name"),
                            directory = entry.getString("kind") == "directory",
                            size = entry.getLong("plaintext_size"),
                        )
                    }
                }.orEmpty()
        val exceptional = offer.optBoolean("exceptional")
        val hasDirectories = offer.optInt("directory_count") > 0
        val needsFolder =
            hasDirectories &&
                SettingsStore.settings.value.saveTreeUri
                    .isBlank()
        TransferRepository.update(id) {
            it.copy(
                status =
                    if (exceptional || needsFolder || !spec.destinationCopyApproved) {
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
                inventoryHasMore =
                    inventoryPage?.has("next_offset") == true &&
                        !inventoryPage.isNull("next_offset"),
                error =
                    when {
                        needsFolder ->
                            uiText(
                                "Choose a writable save folder before receiving directories.",
                                "接收文件夹前，请先选择可写入的保存位置。",
                            )
                        !spec.destinationCopyApproved ->
                            uiText(
                                "This Android destination requires private verification followed by an extra copy.",
                                "此 Android 目标位置需要先在私有目录验证，再额外复制一次。",
                            )
                        exceptional ->
                            uiText(
                                "Review this unusually large transfer before continuing.",
                                "此传输体积异常大，请确认后继续。",
                            )
                        else -> null
                    },
                log = addLog(it.log, "authenticated inventory received"),
            )
        }
        if (!exceptional && !needsFolder && spec.destinationCopyApproved) {
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
        if (transfer.directoryCount > 0 &&
            SettingsStore.settings.value.saveTreeUri
                .isBlank()
        ) {
            TransferRepository.update(id) {
                it.copy(
                    error =
                        uiText(
                            "Choose a writable save folder in Settings, then continue.",
                            "请先在设置中选择可写入的保存位置，然后继续。",
                        ),
                )
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
            TransferRepository.update(id) { it.copy(status = Status.AwaitingDecision, error = error) }
        } else {
            specs[id] = spec.copy(destinationCopyApproved = true)
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
                savedName = names.firstOrNull(),
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
        val jobId =
            spec?.jobId ?: TransferRepository.transfers.value
                .firstOrNull { it.id == id }
                ?.jobId
        // Only job-owned private/incomplete artifacts are discarded. Public
        // saved URIs returned by the result gate are never deleted here.
        receiveBase(id).deleteRecursively()
        jobId?.let {
            File(filesDir, "manifest-v2/source-staging/$it").deleteRecursively()
            File(filesDir, "manifest-v2/destination-save/$it.json").delete()
            File(filesDir, "manifest-v2/destination-save/$it.json.tmp").delete()
        }
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
                    status =
                        if (spec.holdState ==
                            "canceled"
                        ) {
                            Status.Cancelled
                        } else if (spec.holdState == "paused") {
                            Status.Paused
                        } else {
                            Status.Connecting
                        },
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
    ): List<String> = (current + "${clock.format(Instant.now())}  $message").takeLast(TransferRepository.LOG_CAP)

    private fun explainFailure(
        cause: String,
        detail: String,
    ): String =
        when (cause) {
            "sender_permission_lost" ->
                uiText(
                    "The sender lost permission to read a selected item. Reauthorize it and retry.",
                    "发送端已失去所选项目的读取权限。请重新授权后重试。",
                )
            "sender_source_changed" ->
                uiText(
                    "A selected item changed after preparation. Review and send it again.",
                    "所选项目在准备完成后发生了变化。请检查并重新发送。",
                )
            "receiver_space_insufficient" ->
                uiText(
                    "The receiver does not have enough space for this transfer.",
                    "接收端没有足够空间完成此次传输。",
                )
            "receiver_destination_decision_required" ->
                uiText(
                    "The receiver must choose or approve a save destination.",
                    "接收端必须选择或确认保存位置。",
                )
            "receiver_destination_unavailable" ->
                uiText(
                    "The selected receive destination is no longer available.",
                    "所选接收位置已不可用。",
                )
            "receiver_save_failed" ->
                uiText(
                    "The receiver could not save the verified files: $detail",
                    "接收端无法保存已验证的文件：$detail",
                )
            "receiver_reused_object_lost" ->
                uiText(
                    "A destination item selected for reuse changed or disappeared. Restore it and resume, or start a new transfer.",
                    "计划复用的目标项目已变化或消失。请恢复后继续，或重新开始传输。",
                )
            "receiver_finalization_outcome_unknown" ->
                uiText(
                    "The final save could not be confirmed after an interruption. Resume to reconcile the destination.",
                    "中断后无法确认最终保存结果。请继续任务以核对目标位置。",
                )
            "protocol_or_integrity_failure" ->
                uiText(
                    "Integrity verification failed; no unverified file was delivered.",
                    "完整性验证失败；未交付任何未经验证的文件。",
                )
            "transport" ->
                uiText(
                    "The connection was interrupted. Resume to continue from verified data.",
                    "连接已中断。继续任务即可从已验证的数据恢复。",
                )
            else -> detail
        }

    private fun uiText(
        english: String,
        simplifiedChinese: String,
    ): String = AppText.value(english, simplifiedChinese, SettingsStore.settings.value.language)

    private fun receiveBase(id: Long) = File(filesDir, "manifest-v2/receiver/$id")

    private fun receiveTarget(id: Long) = File(receiveBase(id), "final")

    private fun stateDirectory(id: Long) = File(receiveBase(id), "state")

    private fun ManifestSpec.paramsJson(context: Context): String =
        JSONObject()
            .put("direction", if (direction == Direction.Send) "send" else "receive")
            .put("room", room)
            .put("broker", broker)
            .put("relay", relay)
            .put("use_room", useRoom)
            .put("use_mdns", useMdns)
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
                notification(uiText("Preparing transfer…", "正在准备传输…")),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
            foreground = true
        }
    }

    private fun updateNotification() {
        if (!foreground) return
        val active = TransferRepository.transfers.value.filterNot { it.status.isTerminal }
        val text =
            active.lastOrNull()?.let { statusLabel(it.status) }
                ?: uiText("No active transfer", "当前没有进行中的传输")
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
        NotificationCompat
            .Builder(this, CHANNEL)
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
            Status.Preparing -> uiText("Preparing files…", "正在准备文件…")
            Status.Connecting -> uiText("Connecting…", "正在连接…")
            Status.AwaitingDecision -> uiText("Waiting for your save decision", "等待确认保存方式")
            Status.Transferring -> uiText("Transferring files…", "正在传输文件…")
            Status.Receiving -> uiText("Receiving files…", "正在接收文件…")
            Status.Verifying -> uiText("Verifying…", "正在验证…")
            Status.Saving -> uiText("Saving…", "正在保存…")
            Status.WaitingForReceiverSave -> uiText("Waiting for receiver to save…", "等待接收端保存…")
            Status.FinalizingDelivery -> uiText("Saved; finalizing delivery…", "已保存，正在确认送达…")
            Status.Paused -> uiText("Paused", "已暂停")
            Status.Completed -> uiText("Completed", "已完成")
            Status.Failed -> uiText("Failed", "失败")
            Status.Cancelled -> uiText("Cancelled", "已取消")
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
        private const val EXTRA_COPY_APPROVED = "destination_copy_approved"
        private const val EXTRA_USE_ROOM = "use_room"
        private const val EXTRA_USE_MDNS = "use_mdns"

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
            destinationCopyApproved: Boolean,
        ) = launch(
            context,
            ACTION_START_RECEIVE,
            room,
            broker,
            relay,
            qrPayload,
            jobId = null,
            copyApproved = destinationCopyApproved,
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
                    putExtra(EXTRA_USE_ROOM, SettingsStore.settings.value.useRoom)
                    putExtra(EXTRA_USE_MDNS, SettingsStore.settings.value.useMdns)
                },
            )
        }

        fun approveReceive(
            context: Context,
            id: Long,
        ) = command(context, ACTION_APPROVE_RECEIVE, id)

        fun pause(
            context: Context,
            id: Long,
        ) = command(context, ACTION_PAUSE, id)

        fun resume(
            context: Context,
            id: Long,
        ) = command(context, ACTION_RESUME, id)

        fun cancel(
            context: Context,
            id: Long,
        ) = command(context, ACTION_CANCEL, id)

        fun remove(
            context: Context,
            id: Long,
        ) = command(context, ACTION_REMOVE, id)

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

        fun jobStoreDirectory(context: Context): File = File(context.filesDir, "manifest-v2/jobs").apply { mkdirs() }

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
    val destinationCopyApproved: Boolean,
    val useRoom: Boolean,
    val useMdns: Boolean,
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
            .put("destination_copy_approved", destinationCopyApproved)
            .put("use_room", useRoom)
            .put("use_mdns", useMdns)
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
                destinationCopyApproved = value.optBoolean("destination_copy_approved"),
                useRoom = value.getBoolean("use_room"),
                useMdns = value.getBoolean("use_mdns"),
                holdState = value.optString("hold_state").takeIf(String::isNotEmpty),
            )
    }
}
