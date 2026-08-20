package dev.envoix.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.annotation.StringRes
import androidx.core.app.NotificationCompat
import dev.envoix.app.ffi.registerProtectedRememberedCredential
import kotlinx.coroutines.CompletableDeferred
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
    private val receiveGateway = ManifestV2ReceiveGateway.shared
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
                uiText(R.string.service_notification_channel),
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
                uiText(R.string.service_start_failed),
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
                uiText(R.string.service_remembered_device_missing),
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
                        uiText(R.string.service_room_unavailable),
                        "room_unavailable",
                        RecoveryAction.RePair,
                    )
                    return
                }
        val broker = remembered?.summary?.broker ?: intent.getStringExtra(EXTRA_BROKER).orEmpty()
        val relay = remembered?.summary?.relay ?: intent.getStringExtra(EXTRA_RELAY).orEmpty()
        val qrPayload = intent.getStringExtra(EXTRA_QR)
        val invitationCreator =
            intent.getBooleanExtra(EXTRA_INVITATION_CREATOR, qrPayload != null)
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
                        uiText(R.string.service_remembered_device_unlock_failed),
                        "remembered_credential_unavailable",
                        RecoveryAction.Retry,
                    )
                    return
                }
                reference
            }
        val rememberLabel =
            intent
                .getStringExtra(EXTRA_REMEMBER_LABEL)
                ?.trim()
                ?.takeIf(String::isNotEmpty)
        val activityReference =
            if (remembered == null) {
                InviteCodec.activityReference(
                    room,
                    if (direction == Direction.Send) "send" else "receive",
                    invitationCreator,
                )
            } else {
                room
            }
        val id =
            if (reservedId >= 0L &&
                TransferRepository.transfers.value.any {
                    it.id == reservedId && it.direction == direction
                }
            ) {
                reservedId
            } else {
                TransferRepository.create(direction, activityReference)
            }
        val jobId = intent.getStringExtra(EXTRA_JOB_ID)
        TransferRepository.update(id) {
            it.copy(
                room = activityReference,
                jobId = jobId,
            )
        }
        if (!useRoom && !useMdns) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText(R.string.service_pairing_route_required),
                )
            }
            return
        }
        if (useRoom && broker.isBlank()) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText(R.string.service_room_broker_required),
                )
            }
            return
        }
        if (direction == Direction.Send && jobId.isNullOrBlank()) {
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText(R.string.service_prepared_job_missing),
                )
            }
            return
        }
        val pendingRemember =
            if (rememberLabel == null) {
                null
            } else {
                try {
                    RememberedPeerStore.get(this).prepare(rememberLabel, broker, relay)
                } catch (_: Exception) {
                    failReservedStart(
                        id,
                        uiText(R.string.service_start_failed),
                        "relationship_prepare_failed",
                        RecoveryAction.Retry,
                    )
                    return
                }
            }
        val sessionRelationshipId =
            rememberedRelationshipId ?: pendingRemember?.relationshipId
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
                jobId = jobId,
                qrPayload = qrPayload,
                invitationCreator = invitationCreator,
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
        if (
            sessionRelationshipId != null &&
            !RememberedPeerStore.get(this).acquireSession(sessionRelationshipId)
        ) {
            pendingRemember?.let { RememberedPeerStore.get(this).discard(it) }
            TransferRepository.update(id) {
                it.copy(
                    status = Status.Failed,
                    error = uiText(R.string.service_remembered_device_busy),
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
        val callback = ManifestCallback(spec)
        val credentialVault = ManifestCredentialVault(spec)
        callbacks[spec.id] = callback
        val cancellation =
            if (spec.direction == Direction.Send) {
                sendGateway.newCancellation()
            } else {
                receiveGateway.newCancellation()
            }
        callback.bindTypedCancellation(cancellation)
        scope.launch {
            if (spec.direction == Direction.Send) {
                runTypedSend(spec, callback, credentialVault, cancellation)
            } else {
                runTypedReceive(spec, callback, credentialVault, cancellation)
            }
        }
        updateNotification()
    }

    private suspend fun runTypedSend(
        spec: ManifestSpec,
        callback: ManifestCallback,
        credentialVault: ManifestCredentialVault,
        cancellation: ManifestV2SessionCancellation,
    ) {
        try {
            val jobStorePath = jobStoreDirectory(this).absolutePath
            val jobId = requireNotNull(spec.jobId)
            val statePath = stateDirectory(spec.id).apply { mkdirs() }.absolutePath
            val language = SettingsStore.settings.value.language
            val completion =
                if (spec.mode == "remembered") {
                    sendGateway.sendRemembered(
                        request =
                            RememberedManifestV2SendRequest(
                                jobStoreDirectory = jobStorePath,
                                jobId = jobId,
                                stateDirectory = statePath,
                                language = language,
                                broker = spec.broker,
                                relay = spec.relay,
                                credentialReference = requireNotNull(spec.rememberedCredentialReference),
                                generation = spec.rememberedGeneration,
                                previousGeneration = spec.rememberedPreviousGeneration,
                            ),
                        cancellation = cancellation,
                        credentialVault = credentialVault,
                        observer = callback,
                    )
                } else {
                    sendGateway.sendInvitation(
                        request =
                            InvitationManifestV2SendRequest(
                                jobStoreDirectory = jobStorePath,
                                jobId = jobId,
                                stateDirectory = statePath,
                                language = language,
                                broker = spec.broker,
                                relay = spec.relay,
                                invitationReference = spec.room,
                                creator = spec.invitationCreator,
                                rememberConsent = spec.rememberConsent,
                            ),
                        cancellation = cancellation,
                        credentialVault = credentialVault,
                        observer = callback,
                    )
                }
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

    private suspend fun runTypedReceive(
        spec: ManifestSpec,
        callback: ManifestCallback,
        credentialVault: ManifestCredentialVault,
        cancellation: ManifestV2SessionCancellation,
    ) {
        var pending: ManifestV2PendingReceive? = null
        try {
            val statePath = stateDirectory(spec.id).apply { mkdirs() }.absolutePath
            val language = SettingsStore.settings.value.language
            pending =
                if (spec.mode == "remembered") {
                    receiveGateway.receiveRememberedOffer(
                        request =
                            RememberedManifestV2ReceiveRequest(
                                stateDirectory = statePath,
                                language = language,
                                broker = spec.broker,
                                relay = spec.relay,
                                credentialReference = requireNotNull(spec.rememberedCredentialReference),
                                generation = spec.rememberedGeneration,
                                previousGeneration = spec.rememberedPreviousGeneration,
                            ),
                        cancellation = cancellation,
                        credentialVault = credentialVault,
                        observer = callback,
                    )
                } else {
                    receiveGateway.receiveInvitationOffer(
                        request =
                            InvitationManifestV2ReceiveRequest(
                                stateDirectory = statePath,
                                language = language,
                                broker = spec.broker,
                                relay = spec.relay,
                                invitationReference = spec.room,
                                creator = spec.invitationCreator,
                                rememberConsent = spec.rememberConsent,
                            ),
                        cancellation = cancellation,
                        credentialVault = credentialVault,
                        observer = callback,
                    )
                }
            callback.bindPendingReceive(pending)
            if (callbacks[spec.id] !== callback) {
                pending.cancel()
                return
            }
            onOffer(spec.id, pending.offer, callback)
            val destination = callback.awaitReceiveDestination()
            if (callbacks[spec.id] !== callback) {
                pending.cancel()
                return
            }
            val completion =
                pending.receive(
                    destination = destination,
                    platformDestination =
                        AndroidManifestV2PlatformDestination(
                            writer = destinationWriter,
                            isActive = { callbacks[spec.id] === callback },
                            onCommitted = { label ->
                                if (callbacks[spec.id] === callback) {
                                    TransferRepository.update(spec.id) {
                                        it.copy(savedDestinationLabel = label)
                                    }
                                }
                            },
                        ),
                    observer = callback,
                )
            if (callbacks[spec.id] === callback) {
                finishCompleted(
                    id = spec.id,
                    callback = callback,
                    uris = completion.savedRoots.map(ManifestV2SavedRoot::uri),
                    names = completion.savedRoots.map(ManifestV2SavedRoot::finalName),
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
            pending?.let(callback::closePendingReceive)
            callback.closeTypedCancellation(cancellation)
        }
    }

    private inner class ManifestCallback(
        private val spec: ManifestSpec,
    ) : ManifestV2SessionObserver {
        private val id = spec.id
        private val typedCancellation = AtomicReference<ManifestV2SessionCancellation?>()
        private val pendingReceive = AtomicReference<ManifestV2PendingReceive?>()
        private val receiveDestination = CompletableDeferred<ManifestV2ReceiveDestination>()

        fun bindTypedCancellation(cancellation: ManifestV2SessionCancellation) {
            check(typedCancellation.compareAndSet(null, cancellation)) {
                "Manifest v2 cancellation is already bound"
            }
        }

        fun bindPendingReceive(pending: ManifestV2PendingReceive) {
            check(pendingReceive.compareAndSet(null, pending)) {
                "Manifest v2 pending receive is already bound"
            }
        }

        fun cancelAttempt() {
            typedCancellation.get()?.cancel()
            pendingReceive.get()?.cancel()
            receiveDestination.cancel()
        }

        fun closeTypedCancellation(cancellation: ManifestV2SessionCancellation) {
            if (typedCancellation.compareAndSet(cancellation, null)) cancellation.close()
        }

        fun closePendingReceive(pending: ManifestV2PendingReceive) {
            if (pendingReceive.compareAndSet(pending, null)) pending.close()
        }

        suspend fun awaitReceiveDestination(): ManifestV2ReceiveDestination = receiveDestination.await()

        fun commitReceiveDestination(destination: ManifestV2ReceiveDestination): Boolean = receiveDestination.complete(destination)

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

        override fun onFailure(failure: ManifestV2SessionFailure) {
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
    }

    private inner class ManifestCredentialVault(
        spec: ManifestSpec,
    ) : ManifestV2RememberedCredentialVault {
        private val id = spec.id
        private val rememberedPersistence =
            RememberedPersistenceState(spec.pendingRemember, spec.rememberedRelationshipId)

        override fun storeRememberedCredential(
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
        offer: ManifestV2ReceiveOffer,
        callback: ManifestCallback,
    ) {
        if (callbacks[id] !== callback) return
        val spec = specs[id] ?: return
        val hasDirectories = offer.directoryCount > 0
        val needsFolder =
            hasDirectories &&
                SettingsStore.settings.value.saveTreeUri
                    .isBlank()
        TransferRepository.update(id) {
            it.copy(
                status =
                    if (offer.exceptional || needsFolder || !spec.destinationCopyApproved) {
                        Status.AwaitingDecision
                    } else {
                        Status.Transferring
                    },
                jobId = offer.jobId,
                rootCount = offer.rootCount,
                fileCount = offer.fileCount,
                directoryCount = offer.directoryCount,
                total = offer.totalBytes,
                exceptionalOffer = offer.exceptional,
                inventoryPreview = offer.inventoryPreview,
                inventoryHasMore = offer.inventoryHasMore,
                error =
                    when {
                        needsFolder ->
                            uiText(R.string.service_writable_folder_required)
                        !spec.destinationCopyApproved ->
                            uiText(R.string.service_destination_extra_copy)
                        offer.exceptional ->
                            uiText(R.string.service_large_transfer_review)
                        else -> null
                    },
                log = addLog(it.log, "authenticated inventory received"),
            )
        }
        if (!offer.exceptional && !needsFolder && spec.destinationCopyApproved) {
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
                    error = uiText(R.string.service_settings_folder_required),
                )
            }
            return
        }
        val target = receiveTarget(id).apply { mkdirs() }
        val callback = callbacks[id] ?: return
        val committed =
            callback.commitReceiveDestination(
                ManifestV2ReceiveDestination(
                    verifiedStagingDirectory = target.absolutePath,
                    verifiedStagingAllocatableBytes = target.usableSpace,
                    exceptionalTransferApproved = exceptionalApproved || !transfer.exceptionalOffer,
                ),
            )
        if (!committed) return
        specs[id] = spec.copy(destinationCopyApproved = true)
        persistSpecs()
        setState(id, Status.Transferring, "destination decision committed")
    }

    private fun approveReceive(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!TransferPresentationPolicy.actions(transfer).canApprove) return
        continueReceive(id, exceptionalApproved = true)
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
        val store = RememberedPeerStore.get(this)
        spec?.pendingRemember?.let(store::discard)
        spec?.rememberedRelationshipId?.let(store::releaseSession)
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
                uiText(R.string.service_failure_sender_permission_lost)
            "sender_source_unavailable", "sender_item_removed" ->
                uiText(R.string.service_failure_sender_source_unavailable)
            "sender_source_changed" ->
                uiText(R.string.service_failure_sender_source_changed)
            "receiver_space_insufficient" ->
                uiText(R.string.service_failure_receiver_space_insufficient)
            "receiver_destination_decision_required" ->
                uiText(R.string.service_failure_destination_decision_required)
            "receiver_destination_unavailable" ->
                uiText(R.string.service_failure_destination_unavailable)
            "receiver_save_failed" ->
                uiText(R.string.service_failure_receiver_save_failed)
            "receiver_reused_object_lost" ->
                uiText(R.string.service_failure_reused_object_lost)
            "receiver_finalization_outcome_unknown" ->
                uiText(R.string.service_failure_finalization_unknown)
            "protocol_or_integrity_failure" ->
                uiText(R.string.service_failure_integrity)
            "authentication_failed" ->
                uiText(R.string.service_failure_authentication)
            "room_not_found" ->
                uiText(R.string.service_failure_room_not_found)
            "room_expired" ->
                uiText(R.string.service_failure_room_expired)
            "room_full" ->
                uiText(R.string.service_failure_room_full)
            "room_rate_limited", "endpoint_rate_limited", "ip_rate_limited" ->
                uiText(R.string.service_failure_room_rate_limited)
            "room_under_attack" ->
                uiText(R.string.service_failure_room_security)
            "server_busy" ->
                uiText(R.string.service_failure_server_busy)
            "malformed_join", "unsupported_rendezvous_version", "unsupported_version" ->
                uiText(R.string.service_failure_unsupported_version)
            "unsupported_feature" ->
                uiText(R.string.service_failure_unsupported_feature)
            "discovery" ->
                uiText(R.string.service_failure_discovery)
            "transport" ->
                uiText(R.string.service_failure_transport)
            else ->
                uiText(R.string.service_failure_unknown)
        }

    private fun uiText(
        @StringRes id: Int,
    ): String = localizedString(id, SettingsStore.settings.value.language)

    private fun receiveBase(id: Long) = File(filesDir, "manifest-v2/receiver/$id")

    private fun receiveTarget(id: Long) = File(receiveBase(id), "final")

    private fun stateDirectory(id: Long) = File(receiveBase(id), "state")

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
                notification(uiText(R.string.service_notification_preparing_transfer)),
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
                ?: uiText(R.string.service_notification_no_active_transfer)
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
            .setContentTitle(uiText(R.string.app_name))
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
            Status.Preparing -> uiText(R.string.service_notification_preparing_files)
            Status.WaitingForPeer -> uiText(R.string.service_notification_waiting_peer)
            Status.Pairing -> uiText(R.string.service_notification_pairing)
            Status.Connecting -> uiText(R.string.service_notification_connecting)
            Status.AwaitingDecision -> uiText(R.string.service_notification_awaiting_decision)
            Status.Transferring -> uiText(R.string.service_notification_transferring)
            Status.Verifying -> uiText(R.string.service_notification_verifying)
            Status.Saving -> uiText(R.string.service_notification_saving)
            Status.WaitingForReceiverSave -> uiText(R.string.service_notification_receiver_saving)
            Status.FinalizingDelivery -> uiText(R.string.service_notification_finalizing)
            Status.Paused -> uiText(R.string.transfer_status_paused)
            Status.Delivered -> uiText(R.string.transfer_status_delivered)
            Status.Failed -> uiText(R.string.transfer_status_failed)
            Status.Canceled -> uiText(R.string.transfer_status_canceled)
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
        private const val EXTRA_INVITATION_CREATOR = "invitation_creator"
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
            invitationCreator: Boolean = qrPayload != null,
        ): Long {
            // Own the prepared job with a visible card before crossing the
            // foreground-service boundary. Credential-store or service-launch
            // failures can then fail this exact attempt instead of orphaning
            // an unsent native job after the sheet hands ownership away.
            val activityReference =
                if (rememberedRelationshipId == null) {
                    InviteCodec.activityReference(room, "send", invitationCreator)
                } else {
                    room
                }
            val id = TransferRepository.create(Direction.Send, activityReference)
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
                    invitationCreator = invitationCreator,
                    reservedId = id,
                )
            } catch (error: Throwable) {
                TransferRepository.update(id) {
                    it.copy(
                        status = Status.Failed,
                        error =
                            context.localizedString(
                                R.string.service_start_failed,
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
            invitationCreator: Boolean = qrPayload != null,
        ): Long {
            // Reserve the card synchronously. The caller can then wait for the
            // service to move it out of Connecting before accepting a room
            // offer, and can cancel this exact attempt if acceptance fails.
            val activityReference =
                if (rememberedRelationshipId == null) {
                    InviteCodec.activityReference(room, "receive", invitationCreator)
                } else {
                    room
                }
            val id = TransferRepository.create(Direction.Receive, activityReference)
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
                invitationCreator = invitationCreator,
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
            invitationCreator: Boolean = qrPayload != null,
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
                    putExtra(EXTRA_INVITATION_CREATOR, invitationCreator)
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
    val invitationCreator: Boolean,
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
            .put("invitation_creator", invitationCreator)
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
                invitationCreator = value.optBoolean("invitation_creator"),
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
