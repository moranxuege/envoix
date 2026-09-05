package dev.envoix.app.ui

import android.content.Context
import dev.envoix.app.OpLog
import dev.envoix.app.RememberedPeerStore
import dev.envoix.app.SettingsStore
import dev.envoix.app.ffi.FfiFailureCode
import dev.envoix.app.ffi.FfiRememberedCredentialVault
import dev.envoix.app.ffi.FfiRememberedRoomConnectException
import dev.envoix.app.ffi.FfiRememberedRoomConnectMode
import dev.envoix.app.ffi.FfiRoomCloseReason
import dev.envoix.app.ffi.FfiRoomConnectMode
import dev.envoix.app.ffi.FfiRoomControlEventKind
import dev.envoix.app.ffi.FfiRoomControlException
import dev.envoix.app.ffi.FfiRoomControlInvite
import dev.envoix.app.ffi.FfiRoomControlSnapshot
import dev.envoix.app.ffi.FfiRoomLifetimePolicy
import dev.envoix.app.ffi.FfiRoomLifetimeState
import dev.envoix.app.ffi.FfiRoomOfferRejection
import dev.envoix.app.ffi.FfiRoomTransferOffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.File
import java.security.MessageDigest

internal class NativeRoomControlGateway internal constructor(
    private val identityPath: String,
    private val persistVerifiedDevice: (String, RoomControlEndpoint, ByteArray) -> Boolean,
    private val native: RoomControlNativeCore,
    private val scope: CoroutineScope,
) : RoomControlGateway {
    constructor(
        context: Context,
        rememberedRelationshipId: String? = null,
    ) : this(
        identityPath = roomControlIdentityPath(context.filesDir, rememberedRelationshipId),
        persistVerifiedDevice = rememberedDevicePersistence(RememberedPeerStore.get(context)),
        native = UniFfiRoomControlNativeCore,
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
    )

    override val available: Boolean = true
    private val eventChannel = Channel<RoomControlEvent>(Channel.UNLIMITED)
    override val events: Flow<RoomControlEvent> = eventChannel.receiveAsFlow()

    private val sessionLock = Any()
    private var activeGeneration: SessionGeneration? = null
    private var hostSettings: HostSettings? = null

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostSettings = HostSettings(displayName, broker, relay)
        val invitation = native.makeInvitation(broker, relay).project()
        startSession(
            endpoint = invitation.endpoint,
            initialEvent = RoomControlEvent.Hosting(invitation),
            pendingVerification = null,
        ) { cancellation ->
            native.connect(
                RoomControlConnectionRequest(
                    input = invitation.payload,
                    displayName = displayName,
                    mode = FfiRoomConnectMode.HOST,
                    verifiedPairing = false,
                    identityPath = identityPath,
                    fallbackBroker = broker,
                    fallbackRelay = relay,
                ),
                cancellation,
            )
        }
    }

    override suspend fun hostVerified(
        input: String,
        displayName: String,
        peerLabel: String,
    ) {
        hostSettings = null
        startVerifiedSession(FfiRoomConnectMode.HOST, input, displayName, peerLabel)
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) {
        hostSettings = null
        val settings = SettingsStore.settings.value
        val invitation = native.parseInvitation(input, settings.broker, settings.relay).project()
        startSession(
            endpoint = invitation.endpoint,
            initialEvent = RoomControlEvent.Joining(invitation.endpoint),
            pendingVerification = null,
        ) { cancellation ->
            native.connect(
                RoomControlConnectionRequest(
                    input = invitation.payload,
                    displayName = displayName,
                    mode = FfiRoomConnectMode.JOIN,
                    verifiedPairing = false,
                    identityPath = identityPath,
                    fallbackBroker = settings.broker,
                    fallbackRelay = settings.relay,
                ),
                cancellation,
            )
        }
    }

    override suspend fun joinVerified(
        input: String,
        displayName: String,
        peerLabel: String,
    ) {
        hostSettings = null
        startVerifiedSession(FfiRoomConnectMode.JOIN, input, displayName, peerLabel)
    }

    private fun startVerifiedSession(
        mode: FfiRoomConnectMode,
        input: String,
        displayName: String,
        peerLabel: String,
    ) {
        val settings = SettingsStore.settings.value
        val invitation = native.parseInvitation(input, settings.broker, settings.relay).project()
        startSession(
            endpoint = invitation.endpoint,
            initialEvent =
                if (mode == FfiRoomConnectMode.HOST) {
                    RoomControlEvent.Hosting(invitation)
                } else {
                    RoomControlEvent.Joining(invitation.endpoint)
                },
            pendingVerification =
                DeviceVerificationPersistence(
                    fallbackLabel = peerLabel,
                    endpoint = invitation.endpoint,
                ),
        ) { cancellation ->
            native.connect(
                RoomControlConnectionRequest(
                    input = invitation.payload,
                    displayName = displayName,
                    mode = mode,
                    verifiedPairing = true,
                    identityPath = identityPath,
                    fallbackBroker = settings.broker,
                    fallbackRelay = settings.relay,
                ),
                cancellation,
            )
        }
    }

    override suspend fun connectRemembered(
        credentialReference: String,
        generation: Long,
        displayName: String,
        role: RememberedRoomConnectRole,
        broker: String,
        relay: String,
    ) {
        require(credentialReference.isNotBlank()) { "Remembered credential reference is missing" }
        require(generation >= 0) { "Remembered generation must be non-negative" }
        hostSettings = null
        val endpoint = RoomControlEndpoint(broker, relay)
        startSession(
            endpoint = endpoint,
            initialEvent = null,
            pendingVerification = null,
            rememberedGeneration = generation,
        ) { cancellation ->
            native.connectRemembered(
                RememberedRoomControlConnectionRequest(
                    credentialReference = credentialReference,
                    generation = generation.toULong(),
                    displayName = displayName,
                    mode =
                        when (role) {
                            RememberedRoomConnectRole.Connector ->
                                FfiRememberedRoomConnectMode.CONNECTOR
                            RememberedRoomConnectRole.Responder ->
                                FfiRememberedRoomConnectMode.RESPONDER
                        },
                    identityPath = identityPath,
                    broker = broker,
                    relay = relay,
                ),
                cancellation,
            )
        }
    }

    override suspend fun refreshInvite() {
        val settings = hostSettings ?: error("Only a hosted room invitation can be refreshed")
        host(settings.displayName, settings.broker, settings.relay)
    }

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) {
        val result =
            runCommand { session ->
                session.offerTransfer(
                    FfiRoomTransferOffer(
                        offerId = draft.id,
                        transferInvite = draft.transferInvite,
                        rootNames = draft.rootNames.take(3),
                        itemCount = draft.itemCount.coerceAtLeast(0).toUInt(),
                        directoryCount = draft.directoryCount.coerceAtLeast(0).toUInt(),
                        totalBytes = draft.totalBytes.coerceAtLeast(0L).toULong(),
                    ),
                )
            }
        emitLifetime(result)
    }

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) {
        val result =
            runCommand { session ->
                if (accept) {
                    session.acceptOffer(offerId)
                } else {
                    session.rejectOffer(offerId, FfiRoomOfferRejection.DECLINED)
                }
            }
        emitLifetime(result)
    }

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) {
        val result =
            runCommand { session ->
                session.setPolicy(policy.toFfi())
            }
        emitLifetime(result)
    }

    override suspend fun updateTransferActive(active: Boolean) {
        val result =
            runCommand { session ->
                session.setLocalTransferActive(active)
            }
        emitLifetime(result)
    }

    override suspend fun close(reason: RoomCloseReason) {
        val generation = synchronized(sessionLock) { activeGeneration } ?: return
        val connecting =
            synchronized(sessionLock) {
                if (activeGeneration !== generation) return@synchronized false
                generation.localCloseReason = reason
                if (generation.session == null) {
                    activeGeneration = null
                    generation.cancellation.cancel()
                    eventChannel.trySend(RoomControlEvent.Closed(reason))
                    true
                } else {
                    false
                }
            }
        if (connecting) {
            generation.job?.cancel()
            return
        }

        try {
            val closed =
                generation.commands.withLock {
                    val session =
                        synchronized(sessionLock) {
                            generation.session.takeIf { activeGeneration === generation }
                        } ?: return@withLock false
                    session.close(reason.toFfi())
                    true
                }
            if (closed) terminate(generation, RoomControlEvent.Closed(reason))
        } catch (cancelled: CancellationException) {
            clearLocalCloseReason(generation)
            throw cancelled
        } catch (error: FfiRoomControlException.Rejected) {
            clearLocalCloseReason(generation)
            throw IllegalStateException(error.reason)
        } catch (error: FfiRoomControlException) {
            terminate(generation, error.terminalEvent(reason))
        } catch (error: Throwable) {
            terminate(
                generation,
                RoomControlEvent.Failed(error.message ?: "Room control close failed"),
            )
        }
    }

    private fun startSession(
        endpoint: RoomControlEndpoint,
        initialEvent: RoomControlEvent?,
        pendingVerification: DeviceVerificationPersistence?,
        rememberedGeneration: Long? = null,
        connect: suspend (RoomControlNativeCancellation) -> RoomControlNativeSession,
    ) {
        val generation =
            SessionGeneration(
                endpoint = endpoint,
                cancellation = native.newCancellation(),
                pendingVerification = pendingVerification,
                rememberedGeneration = rememberedGeneration,
            )
        val job =
            scope.launch(start = CoroutineStart.LAZY) {
                runGeneration(generation, connect)
            }
        generation.job = job
        val previous =
            synchronized(sessionLock) {
                val current = activeGeneration
                activeGeneration = generation
                initialEvent?.let(eventChannel::trySend)
                current
            }
        previous?.let(::stopReplacedGeneration)
        job.start()
    }

    private suspend fun runGeneration(
        generation: SessionGeneration,
        connect: suspend (RoomControlNativeCancellation) -> RoomControlNativeSession,
    ) {
        var openedSession: RoomControlNativeSession? = null
        try {
            val session = connect(generation.cancellation)
            openedSession = session
            val snapshot = session.snapshot()
            val current =
                synchronized(sessionLock) {
                    if (activeGeneration !== generation) {
                        false
                    } else {
                        generation.session = session
                        true
                    }
                }
            if (!current) {
                runCatching { session.close(FfiRoomCloseReason.BACKGROUNDED) }
                return
            }
            if (!persistVerification(generation, session, snapshot)) {
                runCatching { session.close(FfiRoomCloseReason.PROTOCOL_FAILURE) }
                terminate(
                    generation,
                    RoomControlEvent.Failed(
                        message =
                            "The verified device credential could not be protected on this device",
                        peerAuthenticated = true,
                    ),
                )
                return
            }
            val rememberedGeneration = snapshot.rememberedGeneration?.toPlatformLong()
            if (generation.rememberedGeneration != null &&
                rememberedGeneration != generation.rememberedGeneration
            ) {
                runCatching { session.close(FfiRoomCloseReason.PROTOCOL_FAILURE) }
                terminate(
                    generation,
                    RoomControlEvent.Failed(
                        message = "Remembered-room authentication returned inconsistent state",
                        peerAuthenticated = true,
                        attemptedRememberedGeneration = generation.rememberedGeneration,
                    ),
                )
                return
            }
            val connected =
                RoomControlEvent.Connected(
                    peerName = snapshot.peerName.takeIf(String::isNotBlank),
                    creator = snapshot.creator,
                    endpoint = generation.endpoint,
                    lifetime = snapshot.lifetime.project(),
                    rememberedGeneration = rememberedGeneration,
                )
            if (!emitIfActive(generation, connected)) return
            OpLog.add("ROOM_CONTROL state=connected")
            runEventLoop(generation, session)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: FfiRememberedRoomConnectException) {
            handleRememberedConnectFailure(generation, error)
        } catch (error: Throwable) {
            if (isActive(generation)) {
                val cause = if (openedSession == null) "connect" else "session"
                OpLog.add("ROOM_CONTROL state=failed cause=$cause")
                terminate(
                    generation,
                    RoomControlEvent.Failed(error.message ?: "Room connection failed"),
                )
            }
        } finally {
            openedSession?.close()
            generation.cancellation.close()
        }
    }

    private suspend fun runEventLoop(
        generation: SessionGeneration,
        session: RoomControlNativeSession,
    ) {
        while (scope.isActive && isActive(generation)) {
            val event =
                try {
                    session.nextEvent()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (error: FfiRoomControlException) {
                    terminate(generation, error.terminalEvent(localCloseReason(generation)))
                    return
                }
            when (event.kind) {
                FfiRoomControlEventKind.VERIFICATION_REQUESTED,
                FfiRoomControlEventKind.VERIFICATION_SUCCEEDED,
                FfiRoomControlEventKind.VERIFICATION_FAILED,
                FfiRoomControlEventKind.RELATIONSHIP_UPGRADE_REQUESTED,
                FfiRoomControlEventKind.RELATIONSHIP_UPGRADE_ACCEPTED,
                FfiRoomControlEventKind.RELATIONSHIP_UPGRADE_REJECTED,
                FfiRoomControlEventKind.RELATIONSHIP_UPGRADE_PREPARED,
                FfiRoomControlEventKind.RELATIONSHIP_UPGRADE_COMMITTED,
                FfiRoomControlEventKind.RELATIONSHIP_CONFIRMATION_REQUESTED,
                FfiRoomControlEventKind.RELATIONSHIP_CONFIRMATION_ACKNOWLEDGED,
                FfiRoomControlEventKind.PONG,
                -> Unit
                FfiRoomControlEventKind.INCOMING_OFFER -> {
                    val offer = requireNotNull(event.offer) { "Room offer is missing" }
                    emitIfActive(generation, RoomControlEvent.IncomingOffer(offer.project()))
                }
                FfiRoomControlEventKind.OFFER_ACCEPTED ->
                    emitIfActive(generation, RoomControlEvent.OfferAccepted(event.offerId))
                FfiRoomControlEventKind.OFFER_REJECTED ->
                    emitIfActive(
                        generation,
                        RoomControlEvent.OfferRejected(
                            offerId = event.offerId,
                            reason = event.rejection?.wireName(),
                        ),
                    )
                FfiRoomControlEventKind.LIFETIME_CHANGED -> {
                    val lifetime = requireNotNull(event.lifetime) { "Room lifetime is missing" }
                    emitIfActive(
                        generation,
                        RoomControlEvent.LifetimeChanged(lifetime.project()),
                    )
                }
                FfiRoomControlEventKind.PEER_CLOSED -> {
                    val nativeReason =
                        requireNotNull(event.closeReason) { "Room close reason is missing" }
                            .project()
                    val localReason = synchronized(sessionLock) { generation.localCloseReason }
                    val reason =
                        localReason
                            ?: if (nativeReason == RoomCloseReason.UserEnded) {
                                RoomCloseReason.PeerEnded
                            } else {
                                nativeReason
                            }
                    terminate(generation, RoomControlEvent.Closed(reason))
                    return
                }
            }
        }
    }

    private suspend fun <T> runCommand(operation: suspend (RoomControlNativeSession) -> T): NativeCommandResult<T> {
        val generation =
            synchronized(sessionLock) { activeGeneration }
                ?: error("Room control is not active")
        return generation.commands.withLock {
            val session =
                synchronized(sessionLock) {
                    generation.session.takeIf { activeGeneration === generation }
                } ?: error("Room control is not connected")
            try {
                NativeCommandResult(generation, operation(session))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: FfiRoomControlException.Rejected) {
                throw IllegalStateException(error.reason)
            } catch (error: FfiRoomControlException) {
                terminate(generation, error.terminalEvent(localCloseReason(generation)))
                throw IllegalStateException(error.reason())
            }
        }
    }

    private fun emitLifetime(result: NativeCommandResult<FfiRoomLifetimeState?>) {
        val lifetime = result.value ?: return
        emitIfActive(result.generation, RoomControlEvent.LifetimeChanged(lifetime.project()))
    }

    private fun persistVerification(
        generation: SessionGeneration,
        session: RoomControlNativeSession,
        snapshot: FfiRoomControlSnapshot,
    ): Boolean {
        val pending = generation.pendingVerification ?: return true
        val label = snapshot.peerName.takeIf(String::isNotBlank) ?: pending.fallbackLabel
        return session.storePairingCredential(
            RoomControlCredentialVault(label, pending.endpoint, persistVerifiedDevice),
        )
    }

    private fun handleRememberedConnectFailure(
        generation: SessionGeneration,
        error: FfiRememberedRoomConnectException,
    ) {
        if (!isActive(generation)) return
        when (error) {
            is FfiRememberedRoomConnectException.Failed -> {
                OpLog.add("ROOM_CONTROL state=failed cause=remembered_connect")
                terminate(
                    generation,
                    RoomControlEvent.Failed(
                        message = error.reason,
                        peerAuthenticated = error.peerAuthenticated,
                        attemptedRememberedGeneration = generation.rememberedGeneration,
                        failureCode = error.failureCode?.projectRoomConnectFailure(),
                        retryAfterSeconds = error.retryAfterSeconds?.toPlatformLong(),
                    ),
                )
            }
        }
    }

    private fun stopReplacedGeneration(generation: SessionGeneration) {
        generation.cancellation.cancel()
        val session = synchronized(sessionLock) { generation.session }
        if (session == null) {
            generation.job?.cancel()
            return
        }
        scope.launch {
            generation.commands.withLock {
                runCatching { session.close(FfiRoomCloseReason.BACKGROUNDED) }
            }
            generation.job?.cancel()
        }
    }

    private fun terminate(
        generation: SessionGeneration,
        event: RoomControlEvent,
    ): Boolean {
        val job =
            synchronized(sessionLock) {
                if (activeGeneration !== generation) return false
                activeGeneration = null
                generation.localCloseReason = null
                generation.cancellation.cancel()
                eventChannel.trySend(event)
                generation.job
            }
        job?.cancel()
        return true
    }

    private fun emitIfActive(
        generation: SessionGeneration,
        event: RoomControlEvent,
    ): Boolean =
        synchronized(sessionLock) {
            if (activeGeneration !== generation) return@synchronized false
            eventChannel.trySend(event)
            true
        }

    private fun isActive(generation: SessionGeneration): Boolean =
        synchronized(sessionLock) {
            activeGeneration === generation
        }

    private fun clearLocalCloseReason(generation: SessionGeneration) {
        synchronized(sessionLock) {
            if (activeGeneration === generation) generation.localCloseReason = null
        }
    }

    private fun localCloseReason(generation: SessionGeneration): RoomCloseReason? =
        synchronized(sessionLock) { generation.localCloseReason }

    private data class SessionGeneration(
        val endpoint: RoomControlEndpoint,
        val cancellation: RoomControlNativeCancellation,
        val pendingVerification: DeviceVerificationPersistence?,
        val commands: Mutex = Mutex(),
        var session: RoomControlNativeSession? = null,
        var job: Job? = null,
        var localCloseReason: RoomCloseReason? = null,
        val rememberedGeneration: Long? = null,
    )

    private data class NativeCommandResult<T>(
        val generation: SessionGeneration,
        val value: T,
    )

    private data class HostSettings(
        val displayName: String,
        val broker: String,
        val relay: String,
    )
}

private data class DeviceVerificationPersistence(
    val fallbackLabel: String,
    val endpoint: RoomControlEndpoint,
)

private typealias VerifiedDevicePersistence = (String, RoomControlEndpoint, ByteArray) -> Boolean

internal class RoomControlCredentialVault(
    private val label: String,
    private val endpoint: RoomControlEndpoint,
    private val persist: VerifiedDevicePersistence,
) : FfiRememberedCredentialVault {
    override fun storeRememberedCredential(
        opaqueCredential: ByteArray,
        generation: ULong,
    ): Boolean =
        generation == 0uL &&
            opaqueCredential.isNotEmpty() &&
            persist(label, endpoint, opaqueCredential)
}

private fun rememberedDevicePersistence(store: RememberedPeerStore): VerifiedDevicePersistence =
    { label, endpoint, credential ->
        val pending = runCatching { store.prepare(label, endpoint.broker, endpoint.relay) }.getOrNull()
        if (pending == null) {
            false
        } else {
            store.create(pending, credential, 0L).also { persisted ->
                if (!persisted) store.discard(pending)
            }
        }
    }

/**
 * Every concurrently live Iroh endpoint needs a distinct transport identity.
 * Reusing one endpoint ID lets the newest relay connection steal packets from
 * an already connected room. Keep the existing one-time identity for
 * continuity, and assign each remembered relationship a stable private path.
 */
internal fun roomControlIdentityPath(
    filesDirectory: File,
    rememberedRelationshipId: String?,
): String {
    val relativePath =
        if (rememberedRelationshipId == null) {
            "room-control/identity.json"
        } else {
            require(rememberedRelationshipId.isNotBlank()) {
                "Remembered relationship ID is required"
            }
            val digest =
                MessageDigest
                    .getInstance("SHA-256")
                    .digest(rememberedRelationshipId.toByteArray(Charsets.UTF_8))
                    .joinToString(separator = "") { byte ->
                        "%02x".format(byte.toInt() and 0xff)
                    }
            "room-control/remembered/$digest/identity.json"
        }
    return File(filesDirectory, relativePath).absolutePath
}

private fun FfiRoomControlInvite.project(): RoomControlInvite =
    RoomControlInvite(
        code = code,
        payload = payload,
        endpoint = RoomControlEndpoint(broker, relay),
        expiresAtEpochMs = expiresAtEpochMs.toPlatformLong(),
    )

private fun FfiRoomTransferOffer.project(): RoomTransferOffer =
    RoomTransferOffer(
        id = offerId,
        transferInvite = transferInvite,
        rootNames = rootNames.filter(String::isNotBlank).take(3),
        itemCount = itemCount.coerceAtMost(Int.MAX_VALUE.toUInt()).toInt(),
        directoryCount = directoryCount.coerceAtMost(Int.MAX_VALUE.toUInt()).toInt(),
        totalBytes = totalBytes.toPlatformLong(),
    )

private fun FfiRoomLifetimeState.project(): RoomLifetimeSnapshot {
    val projectedRevision = revision.toPlatformLong()
    require(projectedRevision > 0) { "Room lifetime revision is invalid" }
    val projectedDeadline = idleDeadlineEpochMs?.toPlatformLong()
    require(projectedDeadline == null || projectedDeadline > 0) {
        "Room lifetime deadline is invalid"
    }
    val projectedPolicy = policy.project()
    require(projectedPolicy != RoomLifetimePolicy.UntilForegroundEnds || projectedDeadline == null) {
        "Foreground room lifetime cannot have an idle deadline"
    }
    return RoomLifetimeSnapshot(
        revision = projectedRevision,
        policy = projectedPolicy,
        idleDeadlineEpochMs = projectedDeadline,
    )
}

private fun FfiRoomLifetimePolicy.project(): RoomLifetimePolicy =
    when (this) {
        FfiRoomLifetimePolicy.IDLE15_MINUTES -> RoomLifetimePolicy.Idle15Minutes
        FfiRoomLifetimePolicy.UNTIL_FOREGROUND_ENDS -> RoomLifetimePolicy.UntilForegroundEnds
    }

private fun RoomLifetimePolicy.toFfi(): FfiRoomLifetimePolicy =
    when (this) {
        RoomLifetimePolicy.Idle15Minutes -> FfiRoomLifetimePolicy.IDLE15_MINUTES
        RoomLifetimePolicy.UntilForegroundEnds -> FfiRoomLifetimePolicy.UNTIL_FOREGROUND_ENDS
    }

private fun FfiRoomOfferRejection.wireName(): String =
    when (this) {
        FfiRoomOfferRejection.DECLINED -> "declined"
        FfiRoomOfferRejection.BUSY -> "busy"
        FfiRoomOfferRejection.EXPIRED -> "expired"
        FfiRoomOfferRejection.INVALID -> "invalid"
    }

private fun FfiRoomCloseReason.project(): RoomCloseReason =
    when (this) {
        FfiRoomCloseReason.USER_ENDED -> RoomCloseReason.UserEnded
        FfiRoomCloseReason.IDLE_EXPIRED -> RoomCloseReason.IdleExpired
        FfiRoomCloseReason.INVITATION_EXPIRED -> RoomCloseReason.InvitationExpired
        FfiRoomCloseReason.PEER_ENDED -> RoomCloseReason.PeerEnded
        FfiRoomCloseReason.BACKGROUNDED -> RoomCloseReason.Backgrounded
        FfiRoomCloseReason.NETWORK_LOST -> RoomCloseReason.NetworkLost
        FfiRoomCloseReason.PROTOCOL_FAILURE -> RoomCloseReason.ProtocolFailure
    }

private fun RoomCloseReason.toFfi(): FfiRoomCloseReason =
    when (this) {
        RoomCloseReason.UserEnded -> FfiRoomCloseReason.USER_ENDED
        RoomCloseReason.IdleExpired -> FfiRoomCloseReason.IDLE_EXPIRED
        RoomCloseReason.InvitationExpired -> FfiRoomCloseReason.INVITATION_EXPIRED
        RoomCloseReason.PeerEnded -> FfiRoomCloseReason.PEER_ENDED
        RoomCloseReason.Backgrounded -> FfiRoomCloseReason.BACKGROUNDED
        RoomCloseReason.NetworkLost -> FfiRoomCloseReason.NETWORK_LOST
        RoomCloseReason.ProtocolFailure -> FfiRoomCloseReason.PROTOCOL_FAILURE
    }

private fun FfiRoomControlException.reason(): String =
    when (this) {
        is FfiRoomControlException.Rejected -> reason
        is FfiRoomControlException.NetworkLost -> reason
        is FfiRoomControlException.Canceled -> reason
        is FfiRoomControlException.Failed -> reason
    }

private fun FfiRoomControlException.terminalEvent(localCloseReason: RoomCloseReason?): RoomControlEvent =
    when (this) {
        is FfiRoomControlException.NetworkLost ->
            RoomControlEvent.Closed(localCloseReason ?: RoomCloseReason.NetworkLost)
        is FfiRoomControlException.Canceled ->
            localCloseReason?.let(RoomControlEvent::Closed)
                ?: RoomControlEvent.Failed(reason)
        is FfiRoomControlException.Rejected -> RoomControlEvent.Failed(reason)
        is FfiRoomControlException.Failed -> RoomControlEvent.Failed(reason)
    }

private fun FfiFailureCode.projectRoomConnectFailure(): RoomConnectFailureCode? =
    when (this) {
        FfiFailureCode.ROOM_NOT_FOUND -> RoomConnectFailureCode.RoomNotFound
        FfiFailureCode.ROOM_EXPIRED -> RoomConnectFailureCode.RoomExpired
        FfiFailureCode.ROOM_FULL -> RoomConnectFailureCode.RoomFull
        FfiFailureCode.ROOM_RATE_LIMITED -> RoomConnectFailureCode.RoomRateLimited
        FfiFailureCode.ROOM_UNDER_ATTACK -> RoomConnectFailureCode.RoomUnderAttack
        FfiFailureCode.ENDPOINT_RATE_LIMITED -> RoomConnectFailureCode.EndpointRateLimited
        FfiFailureCode.IP_RATE_LIMITED -> RoomConnectFailureCode.IpRateLimited
        FfiFailureCode.SERVER_BUSY -> RoomConnectFailureCode.ServerBusy
        FfiFailureCode.MALFORMED_JOIN -> RoomConnectFailureCode.MalformedJoin
        FfiFailureCode.UNSUPPORTED_RENDEZVOUS_VERSION ->
            RoomConnectFailureCode.UnsupportedVersion
        else -> null
    }

private fun ULong.toPlatformLong(): Long {
    require(this <= Long.MAX_VALUE.toULong()) { "Native unsigned value exceeds platform range" }
    return toLong()
}
