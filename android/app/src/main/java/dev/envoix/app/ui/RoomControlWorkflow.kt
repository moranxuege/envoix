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
    val inviteRevealed: Boolean = false,
    val peerName: String? = null,
    val creator: Boolean = false,
    val policy: RoomLifetimePolicy = RoomLifetimePolicy.Idle15Minutes,
    val idleDeadlineMs: Long? = null,
    val nowMs: Long = 0L,
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
 * Owns the native control-session lifecycle. Navigation and legacy Invite v1
 * adaptation stay in [ConnectionWorkflowViewModel].
 */
internal class RoomControlWorkflow(
    private val gateway: RoomControlGateway,
    private val scope: CoroutineScope,
    private val nowMs: () -> Long,
    private val wallClockMs: () -> Long = System::currentTimeMillis,
    private val onStateChanged: (RoomControlUiState) -> Unit,
    private val onHosting: (RoomControlInvite) -> Unit,
    private val onConnected: (peerName: String, creator: Boolean) -> Unit,
    private val onCloseAcknowledged: (RoomCloseReason) -> Unit,
) {
    var state = RoomControlUiState(nowMs = nowMs())
        private set

    val available: Boolean
        get() = gateway.available

    private var activeTransferCount = 0
    private var idleTicker: Job? = null
    private var hostingExpiry: Job? = null
    private var incomingOfferExpiry: Job? = null
    private var outgoingOfferCompletion: ((String?) -> Unit)? = null
    private var outgoingOfferId: String? = null

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
        update {
            RoomControlUiState(
                phase = RoomControlPhase.Hosting,
                inviteRevealed = true,
                nowMs = nowMs(),
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
                nowMs = nowMs(),
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
        suspendIdle()
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
                resetIdleDeadline()
            },
        ) {
            gateway.respondToOffer(offerId, accept)
            if (accept) {
                update { it.copy(incomingOffer = null) }
                onAcceptedLocally()
            }
            completion(null)
            resetIdleDeadline()
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
        val hadActive = activeTransferCount > 0
        activeTransferCount = normalized
        if (normalized > 0) {
            suspendIdle()
        } else if (hadActive) {
            resetIdleDeadline()
        }
    }

    fun close(reason: RoomCloseReason) {
        if (state.phase == RoomControlPhase.Legacy) {
            clear()
            return
        }
        if (!state.live) return
        markClosed(reason)
        launchGateway { gateway.close(reason) }
    }

    fun setLegacy(peerName: String) {
        idleTicker?.cancel()
        update {
            RoomControlUiState(
                phase = RoomControlPhase.Legacy,
                peerName = peerName,
                nowMs = nowMs(),
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
        outgoingOfferCompletion = null
        outgoingOfferId = null
        update { RoomControlUiState(nowMs = nowMs()) }
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
                        closeReason = null,
                        error = null,
                    )
                }
                scheduleHostingExpiry(event.invite)
                onHosting(event.invite)
            }
            RoomControlEvent.Joining -> {
                if (state.phase != RoomControlPhase.Joining) return
                update { it.copy(error = null) }
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
                hostingExpiry?.cancel()
                hostingExpiry = null
                update {
                    RoomControlUiState(
                        phase = RoomControlPhase.Connected,
                        peerName = peerName,
                        creator = event.creator,
                        policy = event.policy,
                        nowMs = nowMs(),
                    )
                }
                resetIdleDeadline()
                onConnected(peerName, event.creator)
            }
            is RoomControlEvent.IncomingOffer -> {
                if (!state.connected) return
                suspendIdle()
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
                } else {
                    showError(event.message)
                }
            }
            is RoomControlEvent.PolicyChanged -> {
                if (!state.connected) return
                update { it.copy(policy = event.policy) }
                resetIdleDeadline()
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
                idleTicker?.cancel()
                hostingExpiry?.cancel()
                incomingOfferExpiry?.cancel()
                update {
                    it.copy(
                        phase = RoomControlPhase.Failed,
                        invite = null,
                        incomingOffer = null,
                        outgoingOfferPending = false,
                        idleDeadlineMs = null,
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
        resetIdleDeadline()
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
                idleDeadlineMs = null,
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
        update {
            it.copy(
                phase = RoomControlPhase.Closed,
                invite = null,
                inviteRevealed = false,
                incomingOffer = null,
                outgoingOfferPending = false,
                idleDeadlineMs = null,
                closeReason = reason,
            )
        }
    }

    private fun resetIdleDeadline() {
        if (!state.connected ||
            state.policy != RoomLifetimePolicy.Idle15Minutes ||
            activeTransferCount > 0 ||
            state.incomingOffer != null ||
            state.outgoingOfferPending
        ) {
            suspendIdle()
            return
        }
        val now = nowMs()
        update { it.copy(nowMs = now, idleDeadlineMs = now + ROOM_IDLE_TIMEOUT_MS) }
        idleTicker?.cancel()
        idleTicker =
            scope.launch {
                while (isActive && state.connected) {
                    val current = nowMs()
                    val deadline = state.idleDeadlineMs ?: return@launch
                    update { it.copy(nowMs = current) }
                    if (current >= deadline) {
                        close(RoomCloseReason.IdleExpired)
                        return@launch
                    }
                    delay(IDLE_TICK_MS)
                }
            }
    }

    private fun suspendIdle() {
        idleTicker?.cancel()
        idleTicker = null
        update { it.copy(idleDeadlineMs = null, nowMs = nowMs()) }
    }

    private fun scheduleHostingExpiry(invite: RoomControlInvite) {
        hostingExpiry?.cancel()
        hostingExpiry =
            scope.launch {
                val remaining = invite.expiresAtEpochMs - wallClockMs()
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
        internal const val ROOM_IDLE_TIMEOUT_MS = 15L * 60L * 1_000L
        internal const val INCOMING_OFFER_TIMEOUT_MS = 60L * 1_000L
        private const val IDLE_TICK_MS = 1_000L
    }
}
