package dev.envoix.app.ui

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

internal enum class RoomControlPhase {
    None,
    Hosting,
    Joining,
    Connected,
    Legacy,
    Closed,
    Failed,
}

internal data class RoomControlUiState(
    val phase: RoomControlPhase = RoomControlPhase.None,
    val invite: RoomControlInvite? = null,
    val endpoint: RoomControlEndpoint? = null,
    val inviteRevealed: Boolean = false,
    val peerName: String? = null,
    val creator: Boolean = false,
    val lifetimeRevision: Long = -1L,
    val policy: RoomLifetimePolicy = RoomLifetimePolicy.Idle15Minutes,
    val idleDeadlineEpochMs: Long? = null,
    val nowEpochMs: Long = 0L,
    val incomingOffer: RoomTransferOffer? = null,
    val outgoingOfferPending: Boolean = false,
    val replacementRequested: Boolean = false,
    val closeReason: RoomCloseReason? = null,
    val error: String? = null,
) {
    val connected: Boolean
        get() = phase == RoomControlPhase.Connected

    val live: Boolean
        get() =
            phase == RoomControlPhase.Hosting ||
                phase == RoomControlPhase.Joining ||
                phase == RoomControlPhase.Connected
}

/**
 * Owns the native control-session lifecycle. Navigation and direct InviteV2
 * adaptation stay in [ConnectionWorkflowViewModel].
 */
internal class RoomControlWorkflow(
    private val gateway: RoomControlGateway,
    private val scope: CoroutineScope,
    private val clockEpochMs: () -> Long = System::currentTimeMillis,
    private val onStateChanged: (RoomControlUiState) -> Unit,
    private val onHosting: (RoomControlInvite) -> Unit,
    private val onConnected: (peerName: String, creator: Boolean) -> Unit,
    private val onCloseAcknowledged: (RoomCloseReason) -> Unit,
) {
    var state = RoomControlUiState(nowEpochMs = clockEpochMs())
        private set

    val available: Boolean
        get() = gateway.available

    private var activeTransferCount = 0
    private var idleTicker: Job? = null
    private var hostingExpiry: Job? = null
    private var incomingOfferExpiry: Job? = null
    private var outgoingOfferCompletion: ((String?) -> Unit)? = null
    private var outgoingOfferId: String? = null
    private var idleCloseRequestedRevision: Long? = null

    fun start() {
        if (!gateway.available) return
        scope.launch {
            gateway.events.collect(::handle)
        }
    }

    fun setInviteRevealed(revealed: Boolean) {
        update { current ->
            current.copy(
                inviteRevealed = revealed,
                error = if (revealed) null else current.error,
            )
        }
    }

    fun setReplacementRequested(requested: Boolean) {
        update { it.copy(replacementRequested = requested) }
    }

    fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        if (!gateway.available) {
            showError("Room connections are unavailable in this build")
            return
        }
        val shouldRevealInvite = state.inviteRevealed
        update {
            RoomControlUiState(
                phase = RoomControlPhase.Hosting,
                inviteRevealed = shouldRevealInvite,
                nowEpochMs = clockEpochMs(),
            )
        }
        launchGateway(onError = ::failLifecycle) {
            gateway.host(displayName, broker, relay)
        }
    }

    fun join(
        input: String,
        displayName: String,
        peerName: String?,
    ) {
        if (!gateway.available) {
            showError("Room connections are unavailable in this build")
            return
        }
        update {
            RoomControlUiState(
                phase = RoomControlPhase.Joining,
                peerName = peerName,
                nowEpochMs = clockEpochMs(),
            )
        }
        launchGateway(onError = ::failLifecycle) {
            gateway.join(input, displayName)
        }
    }

    fun refreshInvite() {
        if (state.phase != RoomControlPhase.Hosting || !gateway.available) return
        // Keep the current invitation/session valid until the replacement has
        // actually been generated. A refresh failure is recoverable.
        update { it.copy(error = null) }
        launchGateway(onError = ::showError) { gateway.refreshInvite() }
    }

    fun offer(
        draft: RoomTransferOfferDraft,
        completion: (String?) -> Unit,
    ) {
        if (!state.connected || state.incomingOffer != null || outgoingOfferId != null) {
            completion("Another file offer is already waiting")
            return
        }
        outgoingOfferId = draft.id
        outgoingOfferCompletion = completion
        update { it.copy(outgoingOfferPending = true, error = null) }
        launchGateway(onError = ::completeOutgoingOffer) {
            gateway.offerTransfer(draft)
        }
    }

    fun respondToOffer(
        offerId: String,
        accept: Boolean,
        onAcceptedLocally: () -> Unit = {},
        completion: (String?) -> Unit = {},
    ) {
        val currentOffer = state.incomingOffer
        if (currentOffer?.id != offerId) {
            completion("The file offer has expired")
            return
        }
        incomingOfferExpiry?.cancel()
        incomingOfferExpiry = null
        if (!accept) update { it.copy(incomingOffer = null) }
        launchGateway(
            onError = { error ->
                if (accept) update { it.copy(incomingOffer = null) }
                showError(error)
                completion(error)
            },
        ) {
            gateway.respondToOffer(offerId, accept)
            if (accept) {
                update { it.copy(incomingOffer = null) }
                onAcceptedLocally()
            }
            completion(null)
        }
    }

    fun setKeepOpen(keepOpen: Boolean) {
        if (!state.connected || !state.creator) return
        val policy =
            if (keepOpen) {
                RoomLifetimePolicy.UntilForegroundEnds
            } else {
                RoomLifetimePolicy.Idle15Minutes
            }
        launchGateway { gateway.updatePolicy(policy) }
    }

    fun updateActiveTransfers(count: Int) {
        val normalized = count.coerceAtLeast(0)
        if (activeTransferCount == normalized) return
        if (!state.connected) {
            activeTransferCount = normalized
            return
        }
        val wasActive = activeTransferCount > 0
        activeTransferCount = normalized
        val isActive = normalized > 0
        if (wasActive != isActive) {
            launchGateway { gateway.updateTransferActive(isActive) }
        }
    }

    fun close(reason: RoomCloseReason) {
        if (state.phase == RoomControlPhase.Legacy) {
            clear()
            return
        }
        if (!state.live) return
        if (reason == RoomCloseReason.IdleExpired) {
            if (
                !state.connected ||
                !state.creator ||
                idleCloseRequestedRevision == state.lifetimeRevision
            ) {
                return
            }
            idleCloseRequestedRevision = state.lifetimeRevision
            launchGateway(
                onError = { error ->
                    idleCloseRequestedRevision = null
                    showError(error)
                },
            ) {
                gateway.close(reason)
            }
            return
        }
        markClosed(reason)
        launchGateway { gateway.close(reason) }
    }

    fun setLegacy(peerName: String) {
        idleTicker?.cancel()
        update {
            RoomControlUiState(
                phase = RoomControlPhase.Legacy,
                peerName = peerName,
                nowEpochMs = clockEpochMs(),
            )
        }
    }

    fun clear() {
        idleTicker?.cancel()
        hostingExpiry?.cancel()
        incomingOfferExpiry?.cancel()
        idleTicker = null
        hostingExpiry = null
        incomingOfferExpiry = null
        activeTransferCount = 0
        idleCloseRequestedRevision = null
        outgoingOfferCompletion = null
        outgoingOfferId = null
        update { RoomControlUiState(nowEpochMs = clockEpochMs()) }
    }

    fun showError(message: String) {
        update { it.copy(error = message) }
    }

    fun stop() {
        idleTicker?.cancel()
        hostingExpiry?.cancel()
        incomingOfferExpiry?.cancel()
    }

    private fun handle(event: RoomControlEvent) {
        when (event) {
            is RoomControlEvent.Hosting -> {
                if (state.phase != RoomControlPhase.Hosting) return
                update {
                    it.copy(
                        phase = RoomControlPhase.Hosting,
                        invite = event.invite,
                        endpoint = event.invite.endpoint,
                        closeReason = null,
                        error = null,
                    )
                }
                scheduleHostingExpiry(event.invite)
                onHosting(event.invite)
            }
            is RoomControlEvent.Joining -> {
                if (state.phase != RoomControlPhase.Joining) return
                update {
                    it.copy(
                        endpoint = event.endpoint,
                        error = null,
                    )
                }
            }
            is RoomControlEvent.Connected -> {
                if (state.phase != RoomControlPhase.Hosting &&
                    state.phase != RoomControlPhase.Joining &&
                    state.phase != RoomControlPhase.Connected
                ) {
                    return
                }
                val peerName = event.peerName?.takeIf(String::isNotBlank) ?: "Connected device"
                activeTransferCount = 0
                idleCloseRequestedRevision = null
                hostingExpiry?.cancel()
                hostingExpiry = null
                update {
                    RoomControlUiState(
                        phase = RoomControlPhase.Connected,
                        endpoint = event.endpoint,
                        peerName = peerName,
                        creator = event.creator,
                        lifetimeRevision = event.lifetime.revision,
                        policy = event.lifetime.policy,
                        idleDeadlineEpochMs = event.lifetime.idleDeadlineEpochMs,
                        nowEpochMs = clockEpochMs(),
                    )
                }
                startLifetimeTicker()
                onConnected(peerName, event.creator)
            }
            is RoomControlEvent.IncomingOffer -> {
                if (!state.connected) return
                update { it.copy(incomingOffer = event.offer) }
                scheduleIncomingOfferExpiry(event.offer)
            }
            is RoomControlEvent.OfferAccepted -> {
                if (event.offerId == outgoingOfferId) completeOutgoingOffer(null)
            }
            is RoomControlEvent.OfferRejected -> {
                if (event.offerId == outgoingOfferId) {
                    completeOutgoingOffer(event.reason ?: "The other device declined the file offer")
                }
            }
            is RoomControlEvent.CommandFailed -> {
                if (event.command == "offer" && event.offerId == outgoingOfferId) {
                    completeOutgoingOffer(event.message)
                } else if (event.command == "close") {
                    idleCloseRequestedRevision = null
                    showError(event.message)
                } else {
                    showError(event.message)
                }
            }
            is RoomControlEvent.LifetimeChanged -> {
                if (!state.connected) return
                applyLifetime(event.lifetime)
            }
            is RoomControlEvent.Closed -> {
                if (state.phase == RoomControlPhase.None ||
                    state.phase == RoomControlPhase.Legacy
                ) {
                    return
                }
                hostingExpiry?.cancel()
                hostingExpiry = null
                markClosed(event.reason)
                onCloseAcknowledged(event.reason)
            }
            is RoomControlEvent.Failed -> {
                if (state.phase == RoomControlPhase.None ||
                    state.phase == RoomControlPhase.Legacy
                ) {
                    return
                }
                outgoingOfferCompletion?.invoke(event.message)
                outgoingOfferCompletion = null
                outgoingOfferId = null
                idleCloseRequestedRevision = null
                idleTicker?.cancel()
                hostingExpiry?.cancel()
                incomingOfferExpiry?.cancel()
                update {
                    it.copy(
                        phase = RoomControlPhase.Failed,
                        invite = null,
                        incomingOffer = null,
                        outgoingOfferPending = false,
                        idleDeadlineEpochMs = null,
                        error = event.message,
                    )
                }
                // A native terminal failure is also the final lifecycle
                // acknowledgment for any pending replace-room request.
                onCloseAcknowledged(RoomCloseReason.ProtocolFailure)
            }
        }
    }

    private fun completeOutgoingOffer(error: String?) {
        outgoingOfferCompletion?.invoke(error)
        outgoingOfferCompletion = null
        outgoingOfferId = null
        update { it.copy(outgoingOfferPending = false) }
    }

    private fun failLifecycle(message: String) {
        idleTicker?.cancel()
        hostingExpiry?.cancel()
        incomingOfferExpiry?.cancel()
        update {
            it.copy(
                phase = RoomControlPhase.Failed,
                invite = null,
                inviteRevealed = false,
                incomingOffer = null,
                outgoingOfferPending = false,
                idleDeadlineEpochMs = null,
                error = message,
            )
        }
    }

    private fun markClosed(reason: RoomCloseReason) {
        idleTicker?.cancel()
        hostingExpiry?.cancel()
        incomingOfferExpiry?.cancel()
        idleTicker = null
        hostingExpiry = null
        incomingOfferExpiry = null
        outgoingOfferCompletion?.invoke("The room closed before the offer was accepted")
        outgoingOfferCompletion = null
        outgoingOfferId = null
        idleCloseRequestedRevision = null
        update {
            it.copy(
                phase = RoomControlPhase.Closed,
                invite = null,
                inviteRevealed = false,
                incomingOffer = null,
                outgoingOfferPending = false,
                idleDeadlineEpochMs = null,
                closeReason = reason,
            )
        }
    }

    private fun applyLifetime(lifetime: RoomLifetimeSnapshot) {
        if (lifetime.revision <= state.lifetimeRevision) return
        idleCloseRequestedRevision = null
        update {
            it.copy(
                lifetimeRevision = lifetime.revision,
                policy = lifetime.policy,
                idleDeadlineEpochMs = lifetime.idleDeadlineEpochMs,
                nowEpochMs = clockEpochMs(),
            )
        }
        startLifetimeTicker()
    }

    /**
     * This ticker renders the creator-stamped epoch deadline. At zero the
     * creator asks Rust to close against that authoritative snapshot; a joiner
     * only stops the countdown and never expires the room itself.
     */
    private fun startLifetimeTicker() {
        idleTicker?.cancel()
        idleTicker = null
        val revision = state.lifetimeRevision
        val deadline = state.idleDeadlineEpochMs ?: return
        idleTicker =
            scope.launch {
                while (isActive &&
                    state.connected &&
                    state.lifetimeRevision == revision &&
                    state.idleDeadlineEpochMs == deadline
                ) {
                    val current = clockEpochMs()
                    update { it.copy(nowEpochMs = current) }
                    if (current >= deadline) {
                        if (state.creator && activeTransferCount == 0) {
                            close(RoomCloseReason.IdleExpired)
                        }
                        return@launch
                    }
                    delay(IDLE_TICK_MS)
                }
            }
    }

    private fun scheduleHostingExpiry(invite: RoomControlInvite) {
        hostingExpiry?.cancel()
        hostingExpiry =
            scope.launch {
                val remaining = invite.expiresAtEpochMs - clockEpochMs()
                if (remaining > 0) delay(remaining)
                if (state.phase == RoomControlPhase.Hosting && state.invite == invite) {
                    close(RoomCloseReason.InvitationExpired)
                }
            }
    }

    private fun scheduleIncomingOfferExpiry(offer: RoomTransferOffer) {
        incomingOfferExpiry?.cancel()
        incomingOfferExpiry =
            scope.launch {
                delay(INCOMING_OFFER_TIMEOUT_MS)
                if (state.incomingOffer?.id == offer.id) {
                    respondToOffer(offer.id, false)
                }
            }
    }

    private fun launchGateway(
        onError: (String) -> Unit = ::showError,
        block: suspend () -> Unit,
    ) {
        scope.launch {
            runCatching { block() }
                .onFailure { error ->
                    onError(error.message ?: "Room connection failed")
                }
        }
    }

    private inline fun update(transform: (RoomControlUiState) -> RoomControlUiState) {
        state = transform(state)
        onStateChanged(state)
    }

    companion object {
        internal const val INCOMING_OFFER_TIMEOUT_MS = 60L * 1_000L
        private const val IDLE_TICK_MS = 1_000L
    }
}
