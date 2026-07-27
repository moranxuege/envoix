package dev.envoix.app.ui

import android.net.Uri

internal enum class WorkflowScreen {
    Hub,
    Room,
    Rooms,
    RememberedRoom,
    Activity,
    Settings,
}

internal data class ConnectionWorkflowUiState(
    val screen: WorkflowScreen = WorkflowScreen.Hub,
    val returnScreen: WorkflowScreen = WorkflowScreen.Hub,
    val room: DeviceRoomDraft? = null,
    val selectedRememberedRelationshipId: String? = null,
    val transferDraft: RoomTransferDraft? = null,
    val pendingShares: List<Uri> = emptyList(),
    val control: RoomControlUiState = RoomControlUiState(),
    val incomingOfferBusy: Boolean = false,
    val incomingOfferError: String? = null,
)
