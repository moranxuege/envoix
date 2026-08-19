package dev.envoix.app.ui

import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.InviteCodec
import dev.envoix.app.ParsedInvite
import dev.envoix.app.Settings
import dev.envoix.app.SettingsStore
import dev.envoix.app.TransferActivityGroup
import dev.envoix.app.TransferRepository
import dev.envoix.app.discovery.BleVerificationInvitation
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.NearbyRendezvousOffer
import dev.envoix.app.discovery.preferredRendezvousSource
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Navigation and direct InviteV2 adapter around one live room-control
 * workflow. File-transfer lifetime remains owned by TransferRepository.
 */
internal class ConnectionWorkflowViewModel(
    gateway: RoomControlGateway = RoomControlGatewayProvider.gateway,
    private val currentSettings: () -> Settings = {
        SettingsStore.settings.value
    },
    clockEpochMs: () -> Long = System::currentTimeMillis,
    private val invitationActivityReference: (String, String, Boolean) -> String =
        InviteCodec::activityReference,
) : ViewModel() {
    private val _uiState = MutableStateFlow(ConnectionWorkflowUiState())
    val uiState: StateFlow<ConnectionWorkflowUiState> = _uiState.asStateFlow()

    private var pendingReplacement: (() -> Unit)? = null
    private var replaceAfterClose: (() -> Unit)? = null
    private var pendingNearbyDelivery: (((String?) -> Unit) -> Unit)? = null
    private var activeBleVerification: BleVerificationInvitation? = null
    private var activeBleSelection: NearbyPairingSelection? = null
    private var foreground = true
    private var externalActivityLeases = 0
    private var incomingOfferAttempt = 0L

    private val controlWorkflow =
        RoomControlWorkflow(
            gateway = gateway,
            scope = viewModelScope,
            clockEpochMs = clockEpochMs,
            onStateChanged = { control ->
                val current = _uiState.value
                val incomingOfferChanged =
                    current.control.incomingOffer?.id != control.incomingOffer?.id
                if (incomingOfferChanged) incomingOfferAttempt += 1
                val terminal =
                    control.phase == RoomControlPhase.Closed ||
                        control.phase == RoomControlPhase.Failed
                if (terminal) pendingNearbyDelivery = null
                if (terminal || control.connected) {
                    activeBleVerification = null
                    activeBleSelection = null
                }
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
                        // as a direct standalone transfer. Started repository
                        // jobs are independent and continue in Activity.
                        transferDraft = if (terminal) null else transferDraft,
                        incomingOfferBusy =
                            if (incomingOfferChanged) false else current.incomingOfferBusy,
                        incomingOfferError =
                            if (incomingOfferChanged) null else current.incomingOfferError,
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

    fun shareRoomViaNfc() {
        when (controlWorkflow.state.phase) {
            RoomControlPhase.None, RoomControlPhase.Closed, RoomControlPhase.Failed -> {
                controlWorkflow.setInviteRevealed(false)
                requestRoomAction(::startHosting)
            }
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
        verifiedPeerLabel: String? = null,
    ) {
        val normalized = input.trim()
        if (!RoomControlInviteFormat.looksLikeRoomInvite(normalized)) {
            openTransferInvite(normalized, peerName ?: "Device from invite")
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
                verifiedPeerLabel = verifiedPeerLabel,
            )
        }
    }

    fun startNearbyRoom(
        selection: NearbyPairingSelection,
        deliver: (invite: String, completion: (String?) -> Unit) -> Unit,
    ) {
        val bluetooth = preferredRendezvousSource(selection) == DiscoverySource.Bluetooth
        if (controlWorkflow.state.phase == RoomControlPhase.Hosting) {
            if (bluetooth && activeBleSelection == selection) {
                activeBleVerification?.publicOffer?.let { offer ->
                    deliver(offer) { error -> if (error != null) showControlError(error) }
                    return
                }
            } else if (!bluetooth && activeBleVerification == null) {
                controlWorkflow.state.invite?.payload?.let { payload ->
                    prepareNearbyDelivery(selection, deliver)
                    pendingNearbyDelivery = null
                    deliver(payload) { error -> if (error != null) showControlError(error) }
                    return
                }
            }
        }
        requestRoomAction {
            if (bluetooth) {
                val settings = currentSettings()
                val verification =
                    runCatching {
                        BleVerificationInvitation.create(settings.broker, settings.relay)
                    }.getOrElse {
                        showControlError(it.message ?: "Could not create a verification code")
                        return@requestRoomAction
                    }
                activeBleVerification = verification
                activeBleSelection = selection
                prepareNearbyDelivery(selection, deliver, verification.publicOffer)
                startHosting(
                    verification,
                    selection.displayName ?: "Nearby Envoix device",
                )
            } else {
                activeBleVerification = null
                activeBleSelection = null
                prepareNearbyDelivery(selection, deliver)
                startHosting()
            }
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

    /** Direct InviteV2 room used when no long-lived control tunnel is involved. */
    fun openRoom(draft: DeviceRoomDraft) {
        requestRoomAction { openDirectRoomNow(draft) }
    }

    fun openActivity() {
        openUtilityScreen(WorkflowScreen.Activity)
    }

    fun openRooms() {
        if (transferDecisionPending()) return
        discardTransferDraft()
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Rooms,
                returnScreen = WorkflowScreen.Hub,
                selectedRememberedRelationshipId = null,
                transferDraft = null,
            )
    }

    fun openRememberedRoom(relationshipId: String) {
        if (relationshipId.isBlank() || transferDecisionPending()) return
        discardTransferDraft()
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.RememberedRoom,
                returnScreen = WorkflowScreen.Rooms,
                selectedRememberedRelationshipId = relationshipId,
                transferDraft = null,
            )
    }

    fun openSettings() {
        openUtilityScreen(WorkflowScreen.Settings)
    }

    fun navigateBack() {
        val state = _uiState.value
        when (state.screen) {
            WorkflowScreen.RememberedRoom ->
                _uiState.value =
                    state.copy(
                        screen = WorkflowScreen.Rooms,
                        returnScreen = WorkflowScreen.Hub,
                        selectedRememberedRelationshipId = null,
                    )
            WorkflowScreen.Rooms ->
                _uiState.value =
                    state.copy(
                        screen = WorkflowScreen.Hub,
                        returnScreen = WorkflowScreen.Hub,
                        selectedRememberedRelationshipId = null,
                    )
            WorkflowScreen.Activity, WorkflowScreen.Settings -> {
                val destination =
                    when (state.returnScreen) {
                        WorkflowScreen.Room ->
                            WorkflowScreen.Room.takeIf { state.room != null }
                        WorkflowScreen.RememberedRoom ->
                            WorkflowScreen.RememberedRoom.takeIf {
                                state.selectedRememberedRelationshipId != null
                            }
                        WorkflowScreen.Rooms -> WorkflowScreen.Rooms
                        else -> WorkflowScreen.Hub
                    } ?: WorkflowScreen.Hub
                _uiState.value = state.copy(screen = destination)
            }
            else ->
                _uiState.value =
                    state.copy(
                        screen = WorkflowScreen.Hub,
                        returnScreen = WorkflowScreen.Hub,
                        selectedRememberedRelationshipId = null,
                    )
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
        val activityReference =
            if (transferDraft.roleAdapter == "send") {
                invitationActivityReference(
                    code,
                    "send",
                    transferDraft.preparation.generatedInvite.value
                        ?.reference == code,
                )
            } else {
                code
            }
        TransferRepository.assignActivityGroupByRoom(
            roomReference = activityReference,
            groupId = TransferActivityGroup.oneTime(room.id),
            groupLabel = room.displayName,
            replaceExisting = true,
        )
        _uiState.value =
            _uiState.value.copy(
                room =
                    room.copy(
                        pairingInput = if (usedPending) null else room.pairingInput,
                        pendingRoleAdapter = if (usedPending) null else room.pendingRoleAdapter,
                        transferCodes = room.transferCodes + activityReference,
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

    fun acceptIncomingRoomOffer(
        parseInvitation: (String) -> ParsedInvite?,
        onPrepareReceive: PrepareReceiveBeforeDecision,
        onCancelReceive: (Long) -> Unit,
    ) {
        val offer = controlWorkflow.state.incomingOffer
        if (offer == null) {
            return
        }
        if (_uiState.value.incomingOfferBusy) return
        val attempt = ++incomingOfferAttempt
        setIncomingOfferAcceptance(busy = true, error = null)
        val invitation =
            runCatching { parseInvitation(offer.transferInvite) }
                .getOrNull()
        val transferReference = invitation?.reference
        if (invitation == null || transferReference.isNullOrBlank()) {
            rejectUnusableIncomingOffer(
                offer,
                AppText.value(
                    "This file invitation is invalid or expired.",
                    "此文件邀请无效或已过期。",
                    currentSettings().language,
                ),
            )
            return
        }
        val roomEndpoint = _uiState.value.control.endpoint
        if (roomEndpoint == null) {
            rejectUnusableIncomingOffer(
                offer,
                AppText.value(
                    "The room route is unavailable. Reconnect before receiving files.",
                    "房间连接地址不可用，请重新连接后再接收文件。",
                    currentSettings().language,
                ),
            )
            return
        }
        val belongsToRoom =
            invitation.broker == roomEndpoint.broker &&
                invitation.relay.orEmpty() == roomEndpoint.relay
        if (!belongsToRoom) {
            rejectUnusableIncomingOffer(
                offer,
                AppText.value(
                    "This file offer does not belong to the current room.",
                    "此文件邀请不属于当前房间。",
                    currentSettings().language,
                ),
            )
            return
        }
        var receiveCompletionInvoked = false
        val receiveCompletion = receiveCompletion@{ receiveId: Long, startError: String? ->
            receiveCompletionInvoked = true
            if (attempt != incomingOfferAttempt ||
                controlWorkflow.state.incomingOffer?.id != offer.id
            ) {
                if (receiveId >= 0L) onCancelReceive(receiveId)
                return@receiveCompletion
            }
            if (startError != null) {
                if (receiveId >= 0L) onCancelReceive(receiveId)
                rejectUnusableIncomingOffer(offer, startError)
                return@receiveCompletion
            }
            val room = _uiState.value.room
            if (room != null) {
                TransferRepository.assignActivityGroup(
                    id = receiveId,
                    groupId = TransferActivityGroup.oneTime(room.id),
                    groupLabel = room.displayName,
                    replaceExisting = true,
                )
            }
            confirmIncomingRoomOffer(
                offer = offer,
                transferReference = transferReference,
                receiveId = receiveId,
                onCancelReceive = onCancelReceive,
                attempt = attempt,
            )
        }
        runCatching {
            onPrepareReceive(
                transferReference,
                invitation.broker,
                invitation.relay.orEmpty(),
                null,
                true,
                receiveCompletion,
            )
        }.onFailure { error ->
            if (!receiveCompletionInvoked && attempt == incomingOfferAttempt) {
                rejectUnusableIncomingOffer(
                    offer,
                    error.message ?: "The receiver could not start",
                )
            }
        }
    }

    private fun rejectUnusableIncomingOffer(
        offer: RoomTransferOffer,
        message: String,
    ) {
        controlWorkflow.respondToOffer(
            offerId = offer.id,
            accept = false,
            completion = { responseError ->
                controlWorkflow.showError(responseError ?: message)
            },
        )
    }

    private fun setIncomingOfferAcceptance(
        busy: Boolean,
        error: String?,
    ) {
        _uiState.value =
            _uiState.value.copy(
                incomingOfferBusy = busy,
                incomingOfferError = error,
            )
    }

    private fun attachIncomingTransfer(transferReference: String) {
        _uiState.value.room?.let { room ->
            _uiState.value =
                _uiState.value.copy(
                    room =
                        room.copy(
                            transferCodes = room.transferCodes + transferReference,
                        ),
                )
        }
    }

    private fun confirmIncomingRoomOffer(
        offer: RoomTransferOffer,
        transferReference: String,
        receiveId: Long,
        onCancelReceive: (Long) -> Unit,
        attempt: Long,
    ) {
        controlWorkflow.respondToOffer(
            offerId = offer.id,
            accept = true,
            onAcceptedLocally = { attachIncomingTransfer(transferReference) },
            completion = { decisionError ->
                if (decisionError != null) onCancelReceive(receiveId)
                if (attempt == incomingOfferAttempt) {
                    setIncomingOfferAcceptance(
                        busy = false,
                        error = decisionError,
                    )
                }
            },
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
        // The authenticated room survives backgrounding, but its invitation is
        // still a secret and must not remain visible in task snapshots or
        // behind an external picker.
        controlWorkflow.setInviteRevealed(false)
    }

    fun setExternalActivityActive(active: Boolean) {
        if (active) {
            externalActivityLeases += 1
            return
        }
        externalActivityLeases = (externalActivityLeases - 1).coerceAtLeast(0)
        if (externalActivityLeases == 0 && !foreground) controlWorkflow.setInviteRevealed(false)
    }

    fun acceptIncomingOffer(
        offer: NearbyRendezvousOffer,
        verificationCode: String? = null,
    ): Boolean {
        if (BleVerificationInvitation.isPublicOffer(offer.invite)) {
            if (offer.source != DiscoverySource.Bluetooth || verificationCode == null) return false
            val invitation =
                BleVerificationInvitation.resolve(offer.invite, verificationCode) ?: return false
            joinRoom(
                invitation,
                offer.senderDisplayName,
                offer.senderDisplayName ?: "Nearby Envoix device",
            )
            return true
        }
        if (RoomControlInviteFormat.looksLikeRoomInvite(offer.invite)) {
            if (!controlWorkflow.available) return false
            joinRoom(offer.invite, offer.senderDisplayName)
            return true
        }
        return acceptTransferNearbyOffer(offer)
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
        fixedPayload: String? = null,
    ) {
        pendingNearbyDelivery = { completion ->
            val invite = fixedPayload ?: controlWorkflow.state.invite?.payload
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
        startHosting(verification = null, peerLabel = null)
    }

    private fun startHosting(
        verification: BleVerificationInvitation?,
        peerLabel: String?,
    ) {
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
            verification = verification,
            peerLabel = peerLabel,
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
                            controlEndpoint = _uiState.value.control.endpoint,
                            pendingRoleAdapter =
                                "send".takeIf {
                                    _uiState.value.pendingShares.isNotEmpty()
                                },
                        ),
                transferDraft = null,
            )
    }

    private fun openTransferInvite(
        input: String,
        displayName: String,
    ) {
        val parsed = InviteCodec.parseForRouting(input)
        if (parsed == null) {
            controlWorkflow.showError("That is not a valid Envoix invitation")
            return
        }
        openRoom(
            DeviceRoomDraft(
                displayName = displayName,
                pairingInput = input,
                directionAdapter = parsed.joinerRole.validDirection(),
            ),
        )
    }

    private fun openDirectRoomNow(draft: DeviceRoomDraft) {
        discardTransferDraft()
        val pendingRole =
            when {
                _uiState.value.pendingShares.isNotEmpty() -> "send"
                draft.pendingRoleAdapter != null -> draft.pendingRoleAdapter
                draft.pairingInput != null -> draft.directionAdapter.validDirection()
                else -> null
            }
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Room,
                room =
                    draft.copy(
                        pendingRoleAdapter = pendingRole,
                    ),
                transferDraft = null,
            )
        controlWorkflow.setLegacy(draft.displayName)
    }

    private fun acceptTransferNearbyOffer(offer: NearbyRendezvousOffer): Boolean {
        val parsed = InviteCodec.parseForRouting(offer.invite) ?: return false
        val role = parsed.joinerRole.validDirection()
        val selection =
            NearbyPairingSelection(
                discoveryPeerKey = offer.senderPeerKey,
                displayName = offer.senderDisplayName,
                sources = setOf(offer.source),
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
                            directionAdapter = role,
                            pendingRoleAdapter = role,
                        ),
                )
        } else {
            openRoom(
                DeviceRoomDraft(
                    displayName = offer.senderDisplayName ?: "Nearby Envoix device",
                    pairingInput = offer.invite,
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
                WorkflowScreen.Hub,
                WorkflowScreen.Room,
                WorkflowScreen.Rooms,
                WorkflowScreen.RememberedRoom,
                -> state.screen
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
                selectedRememberedRelationshipId = null,
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
