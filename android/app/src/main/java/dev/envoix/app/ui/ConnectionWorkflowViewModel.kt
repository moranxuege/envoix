package dev.envoix.app.ui

import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.InviteCodec
import dev.envoix.app.Settings
import dev.envoix.app.SettingsStore
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.NearbyRendezvousOffer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.TimeUnit

/**
 * Navigation and legacy Invite v1 adapter around one foreground room-control
 * workflow. File-transfer lifetime remains owned by TransferRepository.
 */
internal class ConnectionWorkflowViewModel(
    gateway: RoomControlGateway = RoomControlGatewayProvider.gateway,
    private val currentSettings: () -> Settings = {
        SettingsStore.settings.value
    },
    nowMs: () -> Long = {
        TimeUnit.NANOSECONDS.toMillis(System.nanoTime())
    },
) : ViewModel() {
    private val _uiState = MutableStateFlow(ConnectionWorkflowUiState())
    val uiState: StateFlow<ConnectionWorkflowUiState> = _uiState.asStateFlow()

    private var pendingReplacement: (() -> Unit)? = null
    private var replaceAfterClose: (() -> Unit)? = null
    private var pendingNearbyDelivery: (((String?) -> Unit) -> Unit)? = null
    private var foreground = true
    private var externalActivityLeases = 0

    private val controlWorkflow =
        RoomControlWorkflow(
            gateway = gateway,
            scope = viewModelScope,
            nowMs = nowMs,
            onStateChanged = { control ->
                val current = _uiState.value
                val terminal =
                    control.phase == RoomControlPhase.Closed ||
                        control.phase == RoomControlPhase.Failed
                if (terminal) pendingNearbyDelivery = null
                val transferDraft = current.transferDraft
                if (terminal &&
                    transferDraft != null &&
                    !transferDraft.preparation.ownershipWasTransferred()
                ) {
                    transferDraft.preparation.discard()
                }
                _uiState.value =
                    current.copy(
                        control = control,
                        // A terminal control room must never expose its setup
                        // as a legacy standalone transfer. Started repository
                        // jobs are independent and continue in Activity.
                        transferDraft = if (terminal) null else transferDraft,
                    )
            },
            onHosting = { invite ->
                pendingNearbyDelivery?.let { delivery ->
                    pendingNearbyDelivery = null
                    delivery { error ->
                        if (error != null) showControlError(error)
                    }
                }
            },
            onConnected = ::openConnectedRoom,
            onCloseAcknowledged = {
                if (replaceAfterClose != null) finishReplacement()
            },
        )

    init {
        controlWorkflow.start()
    }

    fun captureSharedUris(uris: List<Uri>) {
        if (uris.isEmpty()) return
        val state = _uiState.value
        _uiState.value =
            state.copy(
                pendingShares = uris.distinct(),
                room = state.room?.copy(pendingRoleAdapter = "send"),
            )
    }

    fun revealRoomInvite() {
        controlWorkflow.setInviteRevealed(true)
        when (controlWorkflow.state.phase) {
            RoomControlPhase.None, RoomControlPhase.Closed, RoomControlPhase.Failed ->
                requestRoomAction(::startHosting)
            RoomControlPhase.Connected ->
                _uiState.value = _uiState.value.copy(screen = WorkflowScreen.Room)
            else -> Unit
        }
    }

    fun hideRoomInvite() {
        controlWorkflow.setInviteRevealed(false)
    }

    fun refreshRoomInvite() {
        controlWorkflow.refreshInvite()
    }

    fun endWaitingRoom() {
        if (controlWorkflow.state.phase != RoomControlPhase.Hosting &&
            controlWorkflow.state.phase != RoomControlPhase.Joining
        ) {
            return
        }
        pendingNearbyDelivery = null
        discardTransferDraft()
        _uiState.value =
            _uiState.value.copy(
                room = null,
                transferDraft = null,
            )
        controlWorkflow.close(RoomCloseReason.UserEnded)
    }

    fun joinRoom(
        input: String,
        peerName: String? = null,
    ) {
        val normalized = input.trim()
        if (!RoomControlInviteFormat.looksLikeRoomInvite(normalized)) {
            openLegacyInvite(normalized, peerName ?: "Device from invite")
            return
        }
        requestRoomAction {
            if (!controlWorkflow.available) {
                controlWorkflow.showError("Room connections are unavailable in this build")
                return@requestRoomAction
            }
            discardTransferDraft()
            _uiState.value =
                _uiState.value.copy(
                    screen = WorkflowScreen.Hub,
                    room = null,
                    transferDraft = null,
                )
            controlWorkflow.join(
                input = normalized,
                displayName = currentSettings().nearbyDisplayName,
                peerName = peerName,
            )
        }
    }

    fun startNearbyRoom(
        selection: NearbyPairingSelection,
        deliver: (invite: String, completion: (String?) -> Unit) -> Unit,
    ) {
        if (controlWorkflow.state.phase == RoomControlPhase.Hosting) {
            prepareNearbyDelivery(selection, deliver)
            controlWorkflow.state.invite?.payload?.let { payload ->
                pendingNearbyDelivery = null
                deliver(payload) { error ->
                    if (error != null) showControlError(error)
                }
            }
            return
        }
        requestRoomAction {
            prepareNearbyDelivery(selection, deliver)
            startHosting()
        }
    }

    fun cancelReplacement() {
        pendingReplacement = null
        controlWorkflow.setReplacementRequested(false)
    }

    fun returnToCurrentRoom() {
        cancelReplacement()
        val canReturn =
            controlWorkflow.state.connected ||
                controlWorkflow.state.phase == RoomControlPhase.Legacy
        if (canReturn && _uiState.value.room != null) {
            _uiState.value = _uiState.value.copy(screen = WorkflowScreen.Room)
        }
    }

    fun confirmReplacement() {
        val action = pendingReplacement ?: return
        pendingReplacement = null
        replaceAfterClose = action
        controlWorkflow.setReplacementRequested(false)
        if (controlWorkflow.state.live) {
            controlWorkflow.close(RoomCloseReason.UserEnded)
        } else {
            finishReplacement()
        }
    }

    /** Legacy Invite v1 room retained so existing links and old clients work. */
    fun openRoom(draft: DeviceRoomDraft) {
        requestRoomAction { openLegacyRoomNow(draft) }
    }

    fun openActivity() {
        openUtilityScreen(WorkflowScreen.Activity)
    }

    fun openSettings() {
        openUtilityScreen(WorkflowScreen.Settings)
    }

    fun navigateBack() {
        val state = _uiState.value
        if ((state.screen == WorkflowScreen.Activity || state.screen == WorkflowScreen.Settings) &&
            state.returnScreen == WorkflowScreen.Room &&
            state.room != null
        ) {
            _uiState.value = state.copy(screen = WorkflowScreen.Room)
        } else {
            _uiState.value = state.copy(screen = WorkflowScreen.Hub, returnScreen = WorkflowScreen.Hub)
        }
    }

    fun returnToHub() {
        if (controlWorkflow.state.connected) {
            _uiState.value = _uiState.value.copy(screen = WorkflowScreen.Hub)
        } else {
            clearRoom()
        }
    }

    fun beginTransfer(
        roleAdapter: String,
        usesPendingAction: Boolean,
    ) {
        val state = _uiState.value
        if (state.screen != WorkflowScreen.Room || state.transferDraft != null) return
        if (state.control.phase != RoomControlPhase.Connected &&
            state.control.phase != RoomControlPhase.Legacy
        ) {
            return
        }
        if (state.control.connected && roleAdapter != "send") return
        _uiState.value =
            state.copy(
                transferDraft =
                    RoomTransferDraft(
                        roleAdapter = roleAdapter.validDirection(),
                        usesPendingAction = usesPendingAction,
                    ),
            )
    }

    fun dismissTransferDraft() {
        if (transferDecisionPending()) return
        discardTransferDraft()
        _uiState.value = _uiState.value.copy(transferDraft = null)
    }

    fun showRoomQr() {
        val state = _uiState.value
        if (state.control.phase != RoomControlPhase.Legacy ||
            state.screen != WorkflowScreen.Room ||
            state.transferDraft != null
        ) {
            return
        }
        _uiState.value =
            state.copy(
                transferDraft =
                    RoomTransferDraft(
                        roleAdapter = "receive",
                        usesPendingAction = false,
                        showQrInitially = true,
                    ),
            )
    }

    fun completeTransferDraft(
        code: String,
        consumePendingShares: Boolean,
    ) {
        val room = _uiState.value.room ?: return
        val transferDraft = _uiState.value.transferDraft ?: return
        if (!transferDraft.preparation.ownershipWasTransferred()) return
        val usedPending = transferDraft.usesPendingAction
        _uiState.value =
            _uiState.value.copy(
                room =
                    room.copy(
                        pairingInput = if (usedPending) null else room.pairingInput,
                        pairingCode = if (usedPending) null else room.pairingCode,
                        hostedCode = if (usedPending) null else room.hostedCode,
                        hostedPayload = if (usedPending) null else room.hostedPayload,
                        pendingRoleAdapter = if (usedPending) null else room.pendingRoleAdapter,
                        transferCodes = room.transferCodes + code,
                    ),
                transferDraft = null,
                pendingShares =
                    if (consumePendingShares) {
                        emptyList()
                    } else {
                        _uiState.value.pendingShares
                    },
            )
    }

    fun offerRoomTransfer(
        draft: RoomTransferOfferDraft,
        completion: (String?) -> Unit,
    ) {
        controlWorkflow.offer(draft, completion)
    }

    fun acceptIncomingRoomOffer() {
        val offer = controlWorkflow.state.incomingOffer ?: return
        val parsed = InviteCodec.parse(offer.transferInvite)
        if (parsed == null || InviteCodec.oppositeRole(parsed.role) != "receive") {
            rejectIncomingRoomOffer()
            controlWorkflow.showError("The file invitation is invalid")
            return
        }
        val room = _uiState.value.room ?: return
        _uiState.value =
            _uiState.value.copy(
                room =
                    room.copy(
                        pairingInput = offer.transferInvite,
                        pairingCode = parsed.code,
                        pendingRoleAdapter = "receive",
                        transferCodes = room.transferCodes + parsed.code,
                    ),
                transferDraft =
                    RoomTransferDraft(
                        roleAdapter = "receive",
                        usesPendingAction = true,
                    ),
            )
    }

    fun confirmIncomingRoomOffer(completion: (String?) -> Unit) {
        val offer = controlWorkflow.state.incomingOffer
        if (offer == null) {
            completion("The file offer is no longer available")
            return
        }
        controlWorkflow.respondToOffer(
            offerId = offer.id,
            accept = true,
            completion = completion,
        )
    }

    fun rejectIncomingRoomOffer() {
        val offer = controlWorkflow.state.incomingOffer ?: return
        controlWorkflow.respondToOffer(offer.id, false)
    }

    fun setKeepOpen(keepOpen: Boolean) {
        controlWorkflow.setKeepOpen(keepOpen)
    }

    fun updateRoomTransferActivity(activeCount: Int) {
        controlWorkflow.updateActiveTransfers(activeCount)
    }

    fun endRoom(reason: RoomCloseReason = RoomCloseReason.UserEnded) {
        if (controlWorkflow.state.phase == RoomControlPhase.Legacy) {
            clearRoom()
        } else if (controlWorkflow.state.live) {
            controlWorkflow.close(reason)
        } else {
            clearRoom()
        }
    }

    fun dismissEndedRoom() {
        clearRoom()
    }

    fun setForeground(foreground: Boolean) {
        this.foreground = foreground
        if (foreground) return
        controlWorkflow.setInviteRevealed(false)
        if (externalActivityLeases == 0 && controlWorkflow.state.live) {
            controlWorkflow.close(RoomCloseReason.Backgrounded)
        }
    }

    fun setExternalActivityActive(active: Boolean) {
        if (active) {
            externalActivityLeases += 1
            return
        }
        externalActivityLeases = (externalActivityLeases - 1).coerceAtLeast(0)
        if (externalActivityLeases == 0 && !foreground && controlWorkflow.state.live) {
            controlWorkflow.close(RoomCloseReason.Backgrounded)
        }
    }

    fun acceptIncomingOffer(
        offer: NearbyRendezvousOffer,
        fallbackRole: String,
    ): Boolean {
        if (RoomControlInviteFormat.looksLikeRoomInvite(offer.invite)) {
            if (!controlWorkflow.available) return false
            joinRoom(offer.invite, offer.senderDisplayName)
            return true
        }
        return acceptLegacyNearbyOffer(offer, fallbackRole)
    }

    private fun requestRoomAction(action: () -> Unit) {
        if (controlWorkflow.state.live ||
            controlWorkflow.state.phase == RoomControlPhase.Legacy
        ) {
            pendingReplacement = action
            controlWorkflow.setReplacementRequested(true)
            if (_uiState.value.screen == WorkflowScreen.Room) {
                _uiState.value = _uiState.value.copy(screen = WorkflowScreen.Hub)
            }
        } else {
            action()
        }
    }

    private fun prepareNearbyDelivery(
        selection: NearbyPairingSelection,
        deliver: (invite: String, completion: (String?) -> Unit) -> Unit,
    ) {
        pendingNearbyDelivery = { completion ->
            val invite = controlWorkflow.state.invite?.payload
            if (invite == null) {
                completion("Could not create a room invitation")
            } else {
                deliver(invite, completion)
            }
        }
        _uiState.value =
            _uiState.value.copy(
                room =
                    DeviceRoomDraft(
                        displayName = selection.displayName ?: "Nearby Envoix device",
                        nearbySelection = selection,
                        controlSession = true,
                    ),
            )
    }

    private fun startHosting() {
        val settings = currentSettings()
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Hub,
                transferDraft = null,
            )
        controlWorkflow.host(
            displayName = settings.nearbyDisplayName,
            broker = settings.broker,
            relay = settings.relay,
        )
    }

    private fun openConnectedRoom(
        peerName: String,
        creator: Boolean,
    ) {
        val existingRoom = _uiState.value.room
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Room,
                returnScreen = WorkflowScreen.Hub,
                room =
                    (existingRoom ?: DeviceRoomDraft(displayName = peerName))
                        .copy(
                            displayName = peerName,
                            controlSession = true,
                            hostedCode = null,
                            hostedPayload = null,
                            pendingRoleAdapter =
                                "send".takeIf {
                                    _uiState.value.pendingShares.isNotEmpty()
                                },
                        ),
                transferDraft = null,
            )
    }

    private fun openLegacyInvite(
        input: String,
        displayName: String,
    ) {
        val parsed = InviteCodec.parse(input)
        if (parsed == null) {
            controlWorkflow.showError("That is not a valid Envoix invitation")
            return
        }
        openRoom(
            DeviceRoomDraft(
                displayName = displayName,
                pairingInput = input,
                pairingCode = parsed.code,
                directionAdapter =
                    InviteCodec.oppositeRole(parsed.role)
                        ?: currentSettings()
                            .defaultRole
                            .validDirection(),
            ),
        )
    }

    private fun openLegacyRoomNow(draft: DeviceRoomDraft) {
        discardTransferDraft()
        val pendingRole =
            when {
                _uiState.value.pendingShares.isNotEmpty() -> "send"
                draft.pendingRoleAdapter != null -> draft.pendingRoleAdapter
                draft.pairingInput != null || draft.hostedPayload != null -> draft.directionAdapter.validDirection()
                else -> null
            }
        val initialCode = draft.hostedCode ?: draft.pairingCode
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Room,
                room =
                    draft.copy(
                        pendingRoleAdapter = pendingRole,
                        transferCodes = draft.transferCodes + listOfNotNull(initialCode),
                    ),
                transferDraft = null,
            )
        controlWorkflow.setLegacy(draft.displayName)
    }

    private fun acceptLegacyNearbyOffer(
        offer: NearbyRendezvousOffer,
        fallbackRole: String,
    ): Boolean {
        val parsed = InviteCodec.parse(offer.invite) ?: return false
        val role = InviteCodec.oppositeRole(parsed.role) ?: fallbackRole.validDirection()
        val selection =
            NearbyPairingSelection(
                discoveryPeerKey = offer.senderPeerKey,
                displayName = offer.senderDisplayName,
                sources = setOf(DiscoverySource.Bluetooth),
            )
        val currentRoom = _uiState.value.room
        if (_uiState.value.screen == WorkflowScreen.Room &&
            currentRoom?.nearbySelection?.discoveryPeerKey == offer.senderPeerKey
        ) {
            _uiState.value =
                _uiState.value.copy(
                    room =
                        currentRoom.copy(
                            pairingInput = offer.invite,
                            pairingCode = parsed.code,
                            hostedCode = null,
                            hostedPayload = null,
                            directionAdapter = role,
                            pendingRoleAdapter = role,
                            transferCodes = currentRoom.transferCodes + parsed.code,
                        ),
                )
        } else {
            openRoom(
                DeviceRoomDraft(
                    displayName = offer.senderDisplayName ?: "Nearby Envoix device",
                    pairingInput = offer.invite,
                    pairingCode = parsed.code,
                    directionAdapter = role,
                    nearbySelection = selection,
                    pendingRoleAdapter = role,
                ),
            )
        }
        return true
    }

    private fun finishReplacement() {
        val action = replaceAfterClose ?: pendingReplacement ?: return
        replaceAfterClose = null
        pendingReplacement = null
        clearRoom()
        action()
    }

    private fun openUtilityScreen(screen: WorkflowScreen) {
        require(screen == WorkflowScreen.Activity || screen == WorkflowScreen.Settings)
        if (transferDecisionPending()) return
        discardTransferDraft()
        val state = _uiState.value
        val returnScreen =
            when (state.screen) {
                WorkflowScreen.Hub, WorkflowScreen.Room -> state.screen
                WorkflowScreen.Activity, WorkflowScreen.Settings -> state.returnScreen
            }
        _uiState.value =
            state.copy(
                screen = screen,
                returnScreen = returnScreen,
                transferDraft = null,
            )
    }

    private fun clearRoom() {
        discardTransferDraft()
        pendingNearbyDelivery = null
        controlWorkflow.clear()
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Hub,
                returnScreen = WorkflowScreen.Hub,
                room = null,
                transferDraft = null,
            )
    }

    private fun discardTransferDraft() {
        _uiState.value.transferDraft
            ?.preparation
            ?.discard()
    }

    private fun transferDecisionPending(): Boolean {
        val state = _uiState.value
        return state.control.outgoingOfferPending ||
            state.transferDraft
                ?.preparation
                ?.rendezvousBusy
                ?.value == true
    }

    private fun showControlError(message: String) {
        controlWorkflow.showError(message)
    }

    override fun onCleared() {
        controlWorkflow.stop()
        super.onCleared()
    }
}

private fun String.validDirection(): String = if (this == "receive") "receive" else "send"
