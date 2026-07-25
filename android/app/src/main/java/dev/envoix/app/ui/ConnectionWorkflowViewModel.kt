package dev.envoix.app.ui

import android.net.Uri
import androidx.lifecycle.ViewModel
import dev.envoix.app.InviteCodec
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import dev.envoix.app.discovery.NearbyRendezvousOffer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

internal enum class WorkflowScreen {
    Hub,
    Room,
    Activity,
    Settings,
}

internal data class ConnectionWorkflowUiState(
    val screen: WorkflowScreen = WorkflowScreen.Hub,
    val returnScreen: WorkflowScreen = WorkflowScreen.Hub,
    val room: DeviceRoomDraft? = null,
    val transferDraft: RoomTransferDraft? = null,
    val pendingShares: List<Uri> = emptyList(),
)

/**
 * Owns the connection-first shell. Invitations and peer observations remain
 * memory-only: process recreation returns to the hub while durable transfers
 * continue to live in [dev.envoix.app.TransferRepository].
 */
internal class ConnectionWorkflowViewModel : ViewModel() {
    private val _uiState = MutableStateFlow(ConnectionWorkflowUiState())
    val uiState: StateFlow<ConnectionWorkflowUiState> = _uiState.asStateFlow()

    fun captureSharedUris(uris: List<Uri>) {
        if (uris.isEmpty()) return
        val state = _uiState.value
        _uiState.value =
            state.copy(
                pendingShares = uris.distinct(),
                room =
                    state.room?.copy(
                        pendingRoleAdapter = "send",
                    ),
            )
    }

    fun openRoom(draft: DeviceRoomDraft) {
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
                        transferCodes =
                            draft.transferCodes +
                                listOfNotNull(initialCode),
                    ),
                transferDraft = null,
            )
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
            returnToHub()
        }
    }

    fun returnToHub() {
        discardTransferDraft()
        _uiState.value =
            _uiState.value.copy(
                screen = WorkflowScreen.Hub,
                returnScreen = WorkflowScreen.Hub,
                room = null,
                transferDraft = null,
            )
    }

    fun beginTransfer(
        roleAdapter: String,
        usesPendingAction: Boolean,
    ) {
        if (_uiState.value.screen != WorkflowScreen.Room || _uiState.value.transferDraft != null) return
        _uiState.value =
            _uiState.value.copy(
                transferDraft =
                    RoomTransferDraft(
                        roleAdapter = roleAdapter.validDirection(),
                        usesPendingAction = usesPendingAction,
                    ),
            )
    }

    fun dismissTransferDraft() {
        discardTransferDraft()
        _uiState.value = _uiState.value.copy(transferDraft = null)
    }

    fun showRoomQr() {
        if (_uiState.value.screen != WorkflowScreen.Room || _uiState.value.transferDraft != null) return
        _uiState.value =
            _uiState.value.copy(
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

    fun acceptIncomingOffer(
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

    private fun openUtilityScreen(screen: WorkflowScreen) {
        require(screen == WorkflowScreen.Activity || screen == WorkflowScreen.Settings)
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

    private fun discardTransferDraft() {
        _uiState.value.transferDraft
            ?.preparation
            ?.discard()
    }
}

private fun String.validDirection(): String = if (this == "receive") "receive" else "send"
