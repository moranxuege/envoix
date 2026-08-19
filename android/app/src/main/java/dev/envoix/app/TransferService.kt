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
import dev.envoix.app.ffi.registerProtectedRememberedCredential
import dev.envoix.app.ui.AppText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
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
import java.util.concurrent.atomic.AtomicReference

/** Android projection of the canonical Manifest-v2 session. This service owns
 * foreground lifetime and platform save effects only; it does not select an
 * engine or reproduce the Rust reducer. */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val callbacks = ConcurrentHashMap<Long, ManifestCallback>()
    private val specs = ConcurrentHashMap<Long, ManifestSpec>()
    private val progressTrackers = ConcurrentHashMap<Long, TransferProgressTracker>()
    private val destinationWriter by lazy { ManifestV2DestinationWriter(this) }
    private val sendGateway = ManifestV2SendGateway.shared
    private val clock =
        DateTimeFormatter
            .ofPattern("HH:mm:ss")
            .withZone(ZoneId.systemDefault())
    private var foreground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val storedSpecs = readSpecs()
        storedSpecs.filter(ManifestSpec::restorable).forEach { specs[it.id] = it }
        writeSpecs(storedSpecs)
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                uiText("Transfers", "传输"),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    override fun onDestroy() {
        val activeAttempts = callbacks.values.toList()
        callbacks.clear()
        specs.values.forEach(::releaseRememberedSession)
        progressTrackers.clear()
        activeAttempts.forEach(ManifestCallback::cancelAttempt)
        scope.cancel()
        super.onDestroy()
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_START_SEND -> startForegroundTransfer(intent, Direction.Send)
            ACTION_START_RECEIVE -> startForegroundTransfer(intent, Direction.Receive)
            ACTION_APPROVE_RECEIVE -> approveReceive(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_PAUSE -> pause(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_RESUME -> resume(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_CANCEL -> cancelTransfer(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_REMOVE -> removeTransfer(intent.getLongExtra(EXTRA_ID, -1))
            ACTION_RESTORE -> restoreSessions()
        }
        return START_NOT_STICKY
    }

    private fun startForegroundTransfer(
        intent: Intent,
        direction: Direction,
    ) {
        // startForegroundService() starts a strict platform deadline before
        // any credential or source validation runs. Satisfy it immediately,
        // then tear the notification down if validation rejects the request.
        enterForeground()
        try {
            startNew(intent, direction)
        } catch (error: Throwable) {
            failReservedStart(
                intent.getLongExtra(EXTRA_ID, -1L),
                uiText(
                    "Could not start this transfer. Try again.",
                    "无法开始此次传输，请重试。",
                ),
                "start_failed",
                RecoveryAction.Retry,
            )
            OpLog.add("manifest-v2 start rejected: ${error.message ?: error::class.java.simpleName}")
        } finally {
            leaveForegroundIfIdle()
        }
    }

    private fun failReservedStart(
        reservedId: Long,
        message: String,
        cause: String,
        recoveryAction: RecoveryAction,
    ) {
        if (reservedId < 0L) return
        TransferRepository.update(reservedId) {
            it.copy(
                status = Status.Failed,
                error = message,
                failureCause = cause,
                retryable = true,
                recoveryAction = recoveryAction,
            )
        }
    }

    private fun startNew(
        intent: Intent,
        direction: Direction,
    ) {
        val reservedId = intent.getLongExtra(EXTRA_ID, -1L)
        val rememberedRelationshipId =
            intent.getStringExtra(EXTRA_REMEMBERED_RELATIONSHIP_ID)?.takeIf(String::isNotBlank)
        val remembered =
            rememberedRelationshipId?.let { RememberedPeerStore.get(this).load(it) }
        if (rememberedRelationshipId != null && remembered == null) {
            failReservedStart(
                reservedId,
                uiText("This remembered device is no longer available", "此已记住设备已不可用"),
                "remembered_device_missing",
                RecoveryAction.RePair,
            )
            return
        }
        val room =
            remembered?.summary?.label
                ?: intent.getStringExtra(EXTRA_ROOM)?.takeIf(String::isNotBlank)
                ?: run {
                    failReservedStart(
                        reservedId,
                        uiText("The transfer room is unavailable", "传输房间不可用"),
                        "room_unavailable",
                        RecoveryAction.RePair,
                    )
                    return
                }
        val broker = remembered?.summary?.broker ?: intent.getStringExtra(EXTRA_BROKER).orEmpty()
        val relay = remembered?.summary?.relay ?: intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val useRoom = intent.getBooleanExtra(EXTRA_USE_ROOM, true)
        val useMdns = intent.getBooleanExtra(EXTRA_USE_MDNS, true)
        val protectedReference =
            remembered?.let {
                val reference =
                    runCatching {
                        registerProtectedRememberedCredential(it.opaqueCredential)
                    }.getOrNull()
                if (reference.isNullOrBlank()) {
                    failReservedStart(
                        reservedId,
                        uiText(
                            "This remembered device could not be unlocked. Try again.",
                            "暂时无法解锁此已记住设备，请重试。",
                        ),
                        "remembered_credential_unavailable",
                        RecoveryAction.Retry,
                    )
                    return
                }
                reference
            }
        val pendingRemember =
            intent
                .getStringExtra(EXTRA_REMEMBER_LABEL)
                ?.trim()
                ?.takeIf(String::isNotEmpty)
                ?.let { RememberedPeerStore.get(this).prepare(it, broker, relay) }
        val sessionRelationshipId =
            rememberedRelationshipId ?: pendingRemember?.relationshipId
        val id =
            if (reservedId >= 0L &&
                TransferRepository.transfers.value.any {
                    it.id == reservedId && it.direction == direction
                }
            ) {
                reservedId
            } else {
                TransferRepository.create(direction, room)
            }
        sessionRelationshipId?.let { relationshipId ->
            TransferRepository.assignActivityGroup(
                id = id,
                groupId = TransferActivityGroup.remembered(relationshipId),
                groupLabel = remembered?.summary?.label ?: pendingRemember?.label,
            )
        }
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
                mode = if (remembered == null) "invitation" else "remembered",
                rememberConsent = pendingRemember != null,
                pendingRemember = pendingRemember,
                rememberedRelationshipId = sessionRelationshipId,
                rememberedCredentialReference = protectedReference,
                rememberedGeneration = remembered?.summary?.generation ?: 0,
                rememberedPreviousGeneration = remembered?.summary?.previousGeneration,
                restorable = false,
            )
        TransferRepository.update(id) {
            it.copy(
                room = room,
                jobId = spec.jobId,
            )
        }
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
        if (
            sessionRelationshipId != null &&
            !RememberedPeerStore.get(this).acquireSession(sessionRelationshipId)
        ) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error =
                        uiText(
                            "This remembered device already has an active transfer",
                            "此已记住设备已有进行中的传输",
                        ),
                )
            }
            return
        }
        specs[id] = spec
        persistSpecs()
        startNative(spec)
        // Keep launch metadata visible without publishing synthetic readiness.
        // The native Joining event is the authoritative waiting-for-peer
        // barrier used before room control acknowledges an incoming offer.
        TransferRepository.update(id) {
            it.copy(
                qrPayload = spec.qrPayload,
                jobId = spec.jobId,
                log = addLog(it.log, "canonical Manifest v2 session started"),
            )
        }
    }

    private fun startNative(spec: ManifestSpec) {
        enterForeground()
        val initialBytes =
            TransferRepository.transfers.value
                .firstOrNull { it.id == spec.id }
                ?.bytes ?: 0
        progressTrackers[spec.id] = TransferProgressTracker(initialBytes)
        val callback = ManifestCallback(spec, nextNativeAttemptIds.getAndIncrement())
        callbacks[spec.id] = callback
        if (spec.direction == Direction.Send && spec.mode == "remembered") {
            val cancellation = sendGateway.newCancellation()
            callback.bindTypedCancellation(cancellation)
            scope.launch { runTypedRememberedSend(spec, callback, cancellation) }
        } else {
            Native.startManifestV2Session(callback.nativeId, spec.paramsJson(this), callback)
        }
        updateNotification()
    }

    private suspend fun runTypedRememberedSend(
        spec: ManifestSpec,
        callback: ManifestCallback,
        cancellation: ManifestV2SendCancellation,
    ) {
        try {
            val completion =
                sendGateway.sendRemembered(
                    request =
                        RememberedManifestV2SendRequest(
                            jobStoreDirectory = jobStoreDirectory(this).absolutePath,
                            jobId = requireNotNull(spec.jobId),
                            stateDirectory = stateDirectory(spec.id).apply { mkdirs() }.absolutePath,
                            language = SettingsStore.settings.value.language,
                            broker = spec.broker,
                            relay = spec.relay,
                            credentialReference = requireNotNull(spec.rememberedCredentialReference),
                            generation = spec.rememberedGeneration,
                            previousGeneration = spec.rememberedPreviousGeneration,
                        ),
                    cancellation = cancellation,
                    observer = callback,
                )
            if (callbacks[spec.id] === callback) {
                finishCompleted(
                    id = spec.id,
                    callback = callback,
                    totalBytes = completion.totalBytes,
                )
            }
        } catch (error: Throwable) {
            if (callbacks[spec.id] === callback) {
                finishFailed(
                    id = spec.id,
                    callback = callback,
                    cause = "start_failed",
                    detail = error.message ?: error::class.java.simpleName,
                    retryable = true,
                    recoveryAction = RecoveryAction.Retry,
                    outcome = FailureOutcome.Failed,
                    disposition = FailureSessionDisposition.Release,
                )
            }
        } finally {
            callback.closeTypedCancellation(cancellation)
        }
    }

    private inner class ManifestCallback(
        private val spec: ManifestSpec,
        val nativeId: Long,
    ) : ManifestV2Callback,
        ManifestV2SendObserver {
        private val id = spec.id
        private val typedCancellation = AtomicReference<ManifestV2SendCancellation?>()
        private val rememberedPersistence =
            RememberedPersistenceState(spec.pendingRemember, spec.rememberedRelationshipId)

        fun bindTypedCancellation(cancellation: ManifestV2SendCancellation) {
            check(typedCancellation.compareAndSet(null, cancellation)) {
                "Manifest v2 cancellation is already bound"
            }
        }

        fun cancelAttempt() {
            typedCancellation.get()?.cancel() ?: Native.cancelManifestV2Session(nativeId)
        }

        fun closeTypedCancellation(cancellation: ManifestV2SendCancellation) {
            if (typedCancellation.compareAndSet(cancellation, null)) cancellation.close()
        }

        override fun onEvent(json: String) {
            val event = runCatching { JSONObject(json) }.getOrNull() ?: return
            if (event.optString("notice") != "manifest_v2") return
            val kind = event.optString("kind")
            if (kind == "stage_timing") {
                onStageTiming(id, spec.direction, event)
                return
            }
            if (callbacks[id] !== this) return
            when (kind) {
                "progress" -> {
                    updateProgress(id, event.optLong("bytes"), event.optLong("total"))
                    return
                }
                "diagnostic" -> {
                    appendDiagnostic(id, event.optString("message"))
                    return
                }
                "path" -> {
                    val kind =
                        ConnectionPathKind.fromWireOrLegacy(
                            event.optString("path_kind").ifBlank { event.optString("path") },
                        )
                    kind?.let { updateConnectionPath(id, it) }
                    return
                }
            }
            when (event.optString("state")) {
                "waiting_for_peer" -> setState(id, Status.WaitingForPeer, "waiting for peer")
                "pairing" -> setState(id, Status.Pairing, "pairing with peer")
                "connecting" -> setState(id, Status.Connecting, "connecting to peer")
                "offer" -> onOffer(id, event, this)
                "transferring" -> setState(id, Status.Transferring, "transferring files")
                "verifying" -> setState(id, Status.Verifying, "verifying received content")
                "saving" -> setState(id, Status.Saving, "saving to selected destination")
                "waiting_for_receiver_save" ->
                    setState(id, Status.WaitingForReceiverSave, "waiting for receiver to save files")
                "finalizing_delivery" -> setState(id, Status.FinalizingDelivery, "saved; finalizing delivery proof")
                "completed" -> onCompleted(id, event, this)
                "failed" -> onFailed(id, event, this)
            }
        }

        override fun onStarted(
            itemCount: Long,
            totalBytes: Long,
        ) {
            if (callbacks[id] !== this) return
            TransferRepository.update(id) {
                it.copy(
                    total = maxOf(it.total, totalBytes),
                    log = addLog(it.log, "authenticated transfer started · $itemCount items"),
                )
            }
        }

        override fun onPhase(status: Status) {
            if (callbacks[id] !== this) return
            setState(id, status, status.wire.replace('_', ' '))
        }

        override fun onProgress(
            transferred: Long,
            total: Long,
        ) {
            if (callbacks[id] !== this) return
            updateProgress(id, transferred, total)
        }

        override fun onFailure(failure: ManifestV2SendFailure) {
            if (callbacks[id] !== this) return
            finishFailed(
                id = id,
                callback = this,
                cause = failure.cause,
                detail = failure.diagnosticMessage,
                retryable = failure.retryable,
                recoveryAction = failure.recoveryAction,
                outcome = failure.outcome,
                disposition = failure.sessionDisposition,
            )
        }

        override fun onConnectionPath(path: ConnectionPathKind) {
            if (callbacks[id] !== this) return
            updateConnectionPath(id, path)
        }

        override fun onStageTiming(timing: TransferStageTiming) {
            onStageTiming(id, spec.direction, timing)
        }

        override fun onDiagnostic(message: String) {
            if (callbacks[id] !== this) return
            appendDiagnostic(id, message)
        }

        override fun onSaveRequired(requestJson: String): String {
            check(callbacks[id] === this) { "Manifest v2 attempt is no longer active" }
            val result = destinationWriter.saveWithDestination(requestJson)
            TransferRepository.update(id) {
                it.copy(savedDestinationLabel = result.destinationLabel)
            }
            return result.responseJson
        }

        override fun onPlanRequired(requestJson: String): String {
            check(callbacks[id] === this) { "Manifest v2 attempt is no longer active" }
            return destinationWriter.plan(requestJson)
        }

        override fun onRememberedCredential(
            opaqueCredential: ByteArray,
            generation: Long,
        ): Boolean =
            rememberedPersistence.persist(
                create = { pending ->
                    val store = RememberedPeerStore.get(this@TransferService)
                    val persisted =
                        if (store.load(pending.relationshipId) == null) {
                            store.create(pending, opaqueCredential, generation)
                        } else {
                            // A retry may renegotiate after the initial
                            // credential was already committed.
                            store.rotate(pending.relationshipId, opaqueCredential, generation)
                        }
                    if (persisted) {
                        specs.computeIfPresent(id) { _, current ->
                            if (current.pendingRemember?.relationshipId == pending.relationshipId) {
                                current.copy(pendingRemember = null)
                            } else {
                                current
                            }
                        }
                    }
                    persisted
                },
                rotate = { relationshipId ->
                    RememberedPeerStore
                        .get(this@TransferService)
                        .rotate(relationshipId, opaqueCredential, generation)
                },
            )
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
                        Status.Transferring
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
            setState(id, Status.Transferring, "destination decision committed")
        }
    }

    private fun approveReceive(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canApprove) return
        continueReceive(id, exceptionalApproved = true)
    }

    private fun onCompleted(
        id: Long,
        event: JSONObject,
        callback: ManifestCallback,
    ) {
        val roots = event.optJSONArray("roots") ?: JSONArray()
        val uris = (0 until roots.length()).map { roots.getJSONObject(it).getString("uri") }
        val names = (0 until roots.length()).map { roots.getJSONObject(it).getString("final_name") }
        finishCompleted(id, callback, uris, names)
    }

    private fun finishCompleted(
        id: Long,
        callback: ManifestCallback,
        uris: List<String> = emptyList(),
        names: List<String> = emptyList(),
        totalBytes: Long? = null,
    ) {
        TransferRepository.update(id) {
            val deliveredBytes = maxOf(it.total, totalBytes ?: 0L)
            it.copy(
                status = Status.Delivered,
                bytes = deliveredBytes,
                total = deliveredBytes,
                savedUri = uris.firstOrNull(),
                savedUris = uris,
                savedName = names.firstOrNull(),
                error = null,
                log = addLog(it.log, "delivered · receiver save proof acknowledged"),
            )
        }
        callbacks.remove(id, callback)
        progressTrackers.remove(id)
        releaseRememberedSession(specs.remove(id))
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
        val outcome =
            FailureOutcome.fromWire(event.optString("outcome")) ?: FailureOutcome.Failed
        val disposition =
            FailureSessionDisposition.fromWire(event.optString("session_disposition"))
                ?: FailureSessionDisposition.Release
        val retryable = event.optBoolean("retryable", false)
        val recoveryAction = RecoveryAction.fromWire(event.optString("recovery_action"))
        finishFailed(
            id = id,
            callback = callback,
            cause = cause,
            detail = detail,
            retryable = retryable,
            recoveryAction = recoveryAction,
            outcome = outcome,
            disposition = disposition,
        )
    }

    private fun finishFailed(
        id: Long,
        callback: ManifestCallback,
        cause: String,
        detail: String,
        retryable: Boolean,
        recoveryAction: RecoveryAction,
        outcome: FailureOutcome,
        disposition: FailureSessionDisposition,
    ) {
        TransferRepository.update(id) {
            it.copy(
                status = outcome.status,
                failureCause = cause,
                retryable = retryable,
                recoveryAction = recoveryAction,
                error = explainFailure(cause),
                log = addLog(it.log, "$cause · $detail"),
            )
        }
        callbacks.remove(id, callback)
        progressTrackers.remove(id)
        if (disposition == FailureSessionDisposition.Release) {
            releaseRememberedSession(specs.remove(id))
            persistSpecs()
        }
        leaveForegroundIfIdle()
    }

    private fun pause(id: Long) {
        if (id < 0) return
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canPause) return
        callbacks.remove(id)?.cancelAttempt()
        progressTrackers.remove(id)
        specs[id]?.let { specs[id] = it.copy(holdState = "paused") }
        persistSpecs()
        TransferRepository.update(id) {
            if (it.status.isTerminal) it else it.copy(status = Status.Paused, error = null)
        }
        leaveForegroundIfIdle()
    }

    private fun resume(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canResume) return
        val spec = specs[id]?.copy(holdState = null) ?: return
        if (callbacks.containsKey(id)) return
        specs[id] = spec
        persistSpecs()
        TransferRepository.update(id) {
            it.copy(
                status = if (spec.qrPayload == null) Status.Pairing else Status.WaitingForPeer,
                error = null,
                failureCause = null,
                retryable = false,
                recoveryAction = RecoveryAction.None,
                attempt = it.attempt + 1,
                speedBps = 0.0,
                avgBps = 0.0,
                speedHistory = emptyList(),
            )
        }
        startNative(spec)
    }

    private fun cancelTransfer(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canCancel) return
        callbacks.remove(id)?.cancelAttempt()
        progressTrackers.remove(id)
        releaseRememberedSession(specs.remove(id))
        persistSpecs()
        TransferRepository.update(id) {
            if (it.status.isTerminal) it else it.copy(status = Status.Canceled, error = null)
        }
        leaveForegroundIfIdle()
    }

    private fun removeTransfer(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canRemove) return
        callbacks.remove(id)?.cancelAttempt()
        progressTrackers.remove(id)
        val spec = specs.remove(id)
        releaseRememberedSession(spec)
        val jobId =
            spec?.jobId ?: TransferRepository.transfers.value
                .firstOrNull { it.id == id }
                ?.jobId
        // Only job-owned private/incomplete artifacts are discarded. Public
        // saved URIs returned by the result gate are never deleted here.
        receiveBase(id).deleteRecursively()
        jobId?.let { ownedJobId ->
            // A failed room send and its durable outbox entry intentionally
            // share the sealed job. Removing the old Activity card must not
            // delete that job underneath a queued or already-running retry.
            // If the outbox cannot be read, retain the private artifacts; a
            // later explicit queue removal can safely reclaim them.
            val roomOutboxJobIds =
                runCatching {
                    RoomOutboxStore
                        .get(this)
                        .entries()
                        .map(RoomOutboxEntry::jobId)
                }.getOrElse { listOf(ownedJobId) }
            if (!manifestJobHasRemainingOwner(
                    jobId = ownedJobId,
                    otherTransferJobIds = specs.values.map(ManifestSpec::jobId),
                    roomOutboxJobIds = roomOutboxJobIds,
                )
            ) {
                deleteManifestJobArtifacts(filesDir, ownedJobId)
            }
        }
        TransferLogs.delete(id)
        persistSpecs()
        TransferRepository.remove(id)
        leaveForegroundIfIdle()
    }

    private fun releaseRememberedSession(spec: ManifestSpec?) {
        spec
            ?.rememberedRelationshipId
            ?.let { RememberedPeerStore.get(this).releaseSession(it) }
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
                            Status.Canceled
                        } else if (spec.holdState == "paused") {
                            Status.Paused
                        } else {
                            if (spec.qrPayload == null) Status.Pairing else Status.WaitingForPeer
                        },
                    jobId = spec.jobId,
                )
            }
            if (spec.holdState == null && !callbacks.containsKey(spec.id)) startNative(spec)
        }
    }

    private fun updateProgress(
        id: Long,
        transferred: Long,
        total: Long,
    ) {
        progressTrackers[id]
            ?.update(transferred, total)
            ?.let { progress ->
                TransferRepository.update(id) {
                    it.copy(
                        bytes = maxOf(it.bytes, progress.bytes),
                        total = maxOf(it.total, progress.total),
                        speedBps = progress.speedBps,
                        avgBps = progress.avgBps,
                        speedHistory = progress.speedHistory,
                    )
                }
                updateNotification()
            }
    }

    private fun appendDiagnostic(
        id: Long,
        message: String,
    ) {
        if (message.isNotBlank()) {
            TransferRepository.update(id) { it.copy(log = addLog(it.log, message)) }
        }
    }

    private fun updateConnectionPath(
        id: Long,
        kind: ConnectionPathKind,
    ) {
        TransferRepository.update(id) { it.copy(pathAddr = kind.wire) }
    }

    private fun setState(
        id: Long,
        status: Status,
        log: String,
    ) {
        var presentationChanged = false
        TransferRepository.update(id) {
            if (it.status.isTerminal && it.status != status) {
                it
            } else {
                val decision =
                    TransferStatusPresentationReducer.decide(
                        direction = it.direction,
                        current = it.status,
                        reported = status,
                        bytes = it.bytes,
                        total = it.total,
                    )
                if (!decision.shouldPublish) return@update it
                presentationChanged = true
                val presentedStatus = decision.status
                val payloadComplete =
                    presentedStatus == Status.Verifying ||
                        presentedStatus == Status.Saving ||
                        presentedStatus == Status.WaitingForReceiverSave ||
                        presentedStatus == Status.FinalizingDelivery ||
                        presentedStatus == Status.Delivered
                it.copy(
                    status = presentedStatus,
                    bytes = if (payloadComplete) maxOf(it.bytes, it.total) else it.bytes,
                    speedBps = if (presentedStatus == Status.Transferring) it.speedBps else 0.0,
                    error = null,
                    log = addLog(it.log, log),
                )
            }
        }
        if (presentationChanged) updateNotification()
    }

    private fun onStageTiming(
        id: Long,
        expectedDirection: Direction,
        event: JSONObject,
    ) {
        val timing = parseStageTiming(event) ?: return
        onStageTiming(id, expectedDirection, timing)
    }

    private fun onStageTiming(
        id: Long,
        expectedDirection: Direction,
        timing: TransferStageTiming,
    ) {
        if (timing.direction != expectedDirection) return
        TransferRepository.update(id) { transfer ->
            val result = TransferStageTimingHistory.append(transfer.stageTimings, timing)
            if (!result.accepted) {
                transfer
            } else {
                transfer.copy(
                    stageTimings = result.samples,
                    log = addLog(transfer.log, timing.logLine()),
                )
            }
        }
    }

    private fun parseStageTiming(event: JSONObject): TransferStageTiming? {
        val transferId =
            when {
                !event.has("transfer_id") || event.isNull("transfer_id") -> null
                else -> event.opt("transfer_id") as? String ?: return null
            }
        return TransferStageTimingParser.parse(
            stageWire = event.strictString("stage"),
            directionWire = event.strictString("direction"),
            attemptId = event.strictNonNegativeLong("attempt_id"),
            transferId = transferId,
            elapsedUs = event.strictNonNegativeLong("elapsed_us"),
            deltaUs = event.strictNonNegativeLong("delta_us"),
        )
    }

    private fun JSONObject.strictString(name: String): String? {
        if (!has(name) || isNull(name)) return null
        return opt(name) as? String
    }

    private fun JSONObject.strictNonNegativeLong(name: String): Long? {
        if (!has(name) || isNull(name)) return null
        val value =
            when (val raw = opt(name)) {
                is Byte -> raw.toLong()
                is Short -> raw.toLong()
                is Int -> raw.toLong()
                is Long -> raw
                else -> return null
            }
        return value.takeIf { it >= 0L }
    }

    private fun TransferStageTiming.logLine(): String =
        buildString {
            append("stage_timing")
            append(" stage=")
            append(stage.wire)
            append(" direction=")
            append(direction.wire)
            append(" attempt_id=")
            append(attemptId)
            append(" transfer_id=")
            append(transferId ?: "-")
            append(" elapsed_us=")
            append(elapsedUs)
            append(" delta_us=")
            append(deltaUs)
        }

    private fun addLog(
        current: List<String>,
        message: String,
    ): List<String> = (current + "${clock.format(Instant.now())}  $message").takeLast(TransferRepository.LOG_CAP)

    private fun explainFailure(cause: String): String =
        when (cause) {
            "sender_permission_lost" ->
                uiText(
                    "The sender lost permission to read a selected item. Reauthorize it and retry.",
                    "发送端已失去所选项目的读取权限。请重新授权后重试。",
                )
            "sender_source_unavailable", "sender_item_removed" ->
                uiText(
                    "A selected source is no longer available. Review the selection and try again.",
                    "所选来源已不可用。请检查所选内容后重试。",
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
                    "The receiver could not finish saving. Resume to reconcile the destination.",
                    "接收端未能完成保存。请继续任务以核对目标位置。",
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
            "authentication_failed" ->
                uiText(
                    "The peer could not be authenticated. Pair the devices again.",
                    "无法验证对端身份。请重新配对设备。",
                )
            "room_not_found" ->
                uiText(
                    "The Room is not available yet. Ask the creator to keep it open and retry.",
                    "房间尚不可用。请让创建者保持房间开启后重试。",
                )
            "room_expired" ->
                uiText(
                    "This Room expired. Create a new Room Code.",
                    "此房间已过期。请创建新的房间码。",
                )
            "room_full" ->
                uiText(
                    "This Room is already in use. Retry shortly.",
                    "此房间正在使用中。请稍后重试。",
                )
            "room_rate_limited", "endpoint_rate_limited", "ip_rate_limited" ->
                uiText(
                    "Too many Room attempts. Wait before retrying.",
                    "房间尝试次数过多。请稍后再试。",
                )
            "room_under_attack" ->
                uiText(
                    "This Room was closed for security. Create a new Room Code.",
                    "此房间因安全原因已关闭。请创建新的房间码。",
                )
            "server_busy" ->
                uiText(
                    "The Room service is busy. Retry shortly.",
                    "房间服务繁忙。请稍后重试。",
                )
            "malformed_join", "unsupported_rendezvous_version", "unsupported_version" ->
                uiText(
                    "This app version cannot join the Room. Update Envoix.",
                    "当前应用版本无法加入房间。请更新 Envoix。",
                )
            "unsupported_feature" ->
                uiText(
                    "This transfer request is not supported.",
                    "不支持此传输请求。",
                )
            "discovery" ->
                uiText(
                    "The other device could not be reached. Check both devices and resume.",
                    "无法连接另一台设备。请检查两台设备后继续任务。",
                )
            "transport" ->
                uiText(
                    "The connection was interrupted. Resume to continue from verified data.",
                    "连接已中断。继续任务即可从已验证的数据恢复。",
                )
            else ->
                uiText(
                    "The transfer failed. Try again or open Developer mode for details.",
                    "传输失败。请重试，或打开开发者模式查看详情。",
                )
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
            .put("mode", mode)
            .put("room", room)
            .apply {
                if (mode == "invitation" && useRoom) put("invitation_ref", room)
                if (mode == "remembered") {
                    put("remembered_credential_ref", rememberedCredentialReference)
                    put("remembered_generation", rememberedGeneration)
                    put("remembered_previous_generation", rememberedPreviousGeneration)
                }
            }.put("remember_consent", rememberConsent)
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
        writeSpecs(specs.values.sortedBy(ManifestSpec::id))
    }

    private fun writeSpecs(specs: Collection<ManifestSpec>) {
        val values = JSONArray()
        specs.forEach { values.put(it.toJson()) }
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
            (0 until values.length())
                .map { ManifestSpec.fromJson(values.getJSONObject(it)) }
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
            Status.WaitingForPeer -> uiText("Waiting for peer…", "正在等待对端…")
            Status.Pairing -> uiText("Pairing…", "正在配对…")
            Status.Connecting -> uiText("Connecting…", "正在连接…")
            Status.AwaitingDecision -> uiText("Waiting for your save decision", "等待确认保存方式")
            Status.Transferring -> uiText("Transferring files…", "正在传输文件…")
            Status.Verifying -> uiText("Verifying…", "正在验证…")
            Status.Saving -> uiText("Saving…", "正在保存…")
            Status.WaitingForReceiverSave -> uiText("Waiting for receiver to save…", "等待接收端保存…")
            Status.FinalizingDelivery -> uiText("Saved; finalizing delivery…", "已保存，正在确认送达…")
            Status.Paused -> uiText("Paused", "已暂停")
            Status.Delivered -> uiText("Delivered", "已送达")
            Status.Failed -> uiText("Failed", "失败")
            Status.Canceled -> uiText("Canceled", "已取消")
        }

    companion object {
        private val nextNativeAttemptIds = AtomicLong(1)
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
        private const val EXTRA_REMEMBER_LABEL = "remember_label"
        private const val EXTRA_REMEMBERED_RELATIONSHIP_ID = "remembered_relationship_id"

        fun startSend(
            context: Context,
            room: String,
            broker: String,
            relay: String,
            jobId: String,
            qrPayload: String?,
            rememberLabel: String?,
            rememberedRelationshipId: String?,
        ): Long {
            // Own the prepared job with a visible card before crossing the
            // foreground-service boundary. Credential-store or service-launch
            // failures can then fail this exact attempt instead of orphaning
            // an unsent native job after the sheet hands ownership away.
            val id = TransferRepository.create(Direction.Send, room)
            TransferRepository.update(id) { it.copy(jobId = jobId) }
            try {
                launch(
                    context,
                    ACTION_START_SEND,
                    room,
                    broker,
                    relay,
                    qrPayload,
                    jobId,
                    copyApproved = false,
                    rememberLabel = rememberLabel,
                    rememberedRelationshipId = rememberedRelationshipId,
                    reservedId = id,
                )
            } catch (error: Throwable) {
                TransferRepository.update(id) {
                    it.copy(
                        status = Status.Failed,
                        error =
                            AppText.value(
                                "Could not start this transfer. Try again.",
                                "无法开始此次传输，请重试。",
                                SettingsStore.settings.value.language,
                            ),
                        failureCause = "start_failed",
                        retryable = true,
                        recoveryAction = RecoveryAction.Retry,
                        log =
                            (
                                it.log +
                                    "start_failed · ${error.message ?: error::class.java.simpleName}"
                            ).takeLast(TransferRepository.LOG_CAP),
                    )
                }
            }
            return id
        }

        fun startReceive(
            context: Context,
            room: String,
            broker: String,
            relay: String,
            qrPayload: String?,
            destinationCopyApproved: Boolean,
            rememberLabel: String?,
            rememberedRelationshipId: String?,
        ): Long {
            // Reserve the card synchronously. The caller can then wait for the
            // service to move it out of Connecting before accepting a room
            // offer, and can cancel this exact attempt if acceptance fails.
            val id = TransferRepository.create(Direction.Receive, room)
            launch(
                context,
                ACTION_START_RECEIVE,
                room,
                broker,
                relay,
                qrPayload,
                jobId = null,
                copyApproved = destinationCopyApproved,
                rememberLabel = rememberLabel,
                rememberedRelationshipId = rememberedRelationshipId,
                reservedId = id,
            )
            return id
        }

        private fun launch(
            context: Context,
            action: String,
            room: String,
            broker: String,
            relay: String,
            qrPayload: String?,
            jobId: String?,
            copyApproved: Boolean,
            rememberLabel: String?,
            rememberedRelationshipId: String?,
            reservedId: Long? = null,
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
                    putExtra(EXTRA_USE_ROOM, true)
                    putExtra(EXTRA_USE_MDNS, false)
                    putExtra(EXTRA_REMEMBER_LABEL, rememberLabel)
                    putExtra(EXTRA_REMEMBERED_RELATIONSHIP_ID, rememberedRelationshipId)
                    reservedId?.let { putExtra(EXTRA_ID, it) }
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

        fun nextSessionIdFloor(context: Context): Long {
            val persistedIds =
                runCatching {
                    val values = JSONArray(specFile(context).readText())
                    (0 until values.length()).map { values.getJSONObject(it).getLong("id") }
                }.getOrDefault(emptyList())
            val retainedWorkspaceNames =
                File(context.filesDir, "manifest-v2/receiver")
                    .listFiles()
                    ?.asSequence()
                    ?.filter(File::isDirectory)
                    ?.map(File::getName)
                    ?.toList()
                    .orEmpty()
            val retainedLogNames =
                File(context.filesDir, "logs/transfers")
                    .listFiles()
                    ?.map(File::getName)
                    .orEmpty()
            return nextManifestSessionIdFloor(
                persistedIds,
                retainedWorkspaceNames,
                retainedLogNames,
            )
        }

        private fun specFile(context: Context) = File(context.filesDir, "manifest-v2/android-sessions.json")
    }
}

/**
 * Receiver workspaces outlive terminal cards so save reconciliation and
 * verified artifacts are never silently overwritten. Include those retained
 * directories when seeding the process-local id allocator.
 */
internal fun nextManifestSessionIdFloor(
    persistedIds: Iterable<Long>,
    retainedWorkspaceNames: Iterable<String>,
    retainedLogNames: Iterable<String> = emptyList(),
): Long {
    val highest =
        (
            persistedIds.asSequence() +
                retainedWorkspaceNames.asSequence().mapNotNull(String::toLongOrNull) +
                retainedLogNames.asSequence().mapNotNull { name ->
                    name
                        .takeIf { it.startsWith("transfer-") }
                        ?.removePrefix("transfer-")
                        ?.substringBefore('.')
                        ?.toLongOrNull()
                }
        ).filter { it in 1 until Long.MAX_VALUE }
            .maxOrNull()
            ?: 0L
    return highest + 1L
}

internal fun deleteManifestJobArtifacts(
    filesDirectory: File,
    jobId: String,
) {
    if (jobId.length != 32 || !jobId.all(Char::isLowerCaseHexDigit)) return
    File(filesDirectory, "manifest-v2/source-staging/$jobId").deleteRecursively()
    File(filesDirectory, "manifest-v2/jobs/.envoix-staging/$jobId").deleteRecursively()
    File(filesDirectory, "manifest-v2/jobs/job-$jobId.json").delete()
    File(filesDirectory, "manifest-v2/jobs/.job-$jobId.tmp").delete()
    File(filesDirectory, "manifest-v2/destination-save")
        .listFiles()
        ?.filter { file ->
            file.isFile &&
                file.name.startsWith("$jobId-") &&
                (file.name.endsWith(".json") || file.name.endsWith(".json.tmp"))
        }?.forEach(File::delete)
}

internal fun manifestJobHasRemainingOwner(
    jobId: String,
    otherTransferJobIds: Iterable<String?>,
    roomOutboxJobIds: Iterable<String>,
): Boolean =
    otherTransferJobIds.any { it == jobId } ||
        roomOutboxJobIds.any { it == jobId }

private fun Char.isLowerCaseHexDigit(): Boolean = this in '0'..'9' || this in 'a'..'f'

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
    val mode: String,
    val rememberConsent: Boolean,
    val pendingRemember: PendingRememberedPeer?,
    val rememberedRelationshipId: String?,
    val rememberedCredentialReference: String?,
    val rememberedGeneration: Long,
    val rememberedPreviousGeneration: Long?,
    val restorable: Boolean,
) {
    fun toJson(): JSONObject =
        JSONObject()
            .put("id", id)
            .put("direction", direction.name.lowercase())
            // Active pairing handles are process-only. This tombstone keeps the
            // session id monotonic without restoring unauthenticated secrets.
            .put("room", if (restorable) room else "")
            .put("broker", broker)
            .put("relay", relay)
            .put("job_id", jobId)
            // InviteV2 credentials are process-memory-only pending state.
            .put("qr", JSONObject.NULL)
            .put("destination_copy_approved", destinationCopyApproved)
            .put("use_room", useRoom)
            .put("use_mdns", useMdns)
            .put("hold_state", holdState)
            .put("mode", mode)
            .put("remember_consent", false)
            .put("restorable", restorable)

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
                mode = value.optString("mode", "invitation"),
                rememberConsent = false,
                pendingRemember = null,
                rememberedRelationshipId = null,
                rememberedCredentialReference = null,
                rememberedGeneration = 0,
                rememberedPreviousGeneration = null,
                restorable = value.optBoolean("restorable", false),
            )
    }
}

internal class RememberedPersistenceState(
    private val pending: PendingRememberedPeer?,
    private val relationshipId: String?,
) {
    private var createdRelationshipId: String? = null

    @Synchronized
    fun persist(
        create: (PendingRememberedPeer) -> Boolean,
        rotate: (String) -> Boolean,
    ): Boolean {
        createdRelationshipId?.let { return rotate(it) }
        pending?.let {
            return create(it).also { created ->
                if (created) createdRelationshipId = it.relationshipId
            }
        }
        return relationshipId?.let(rotate) ?: false
    }
}
