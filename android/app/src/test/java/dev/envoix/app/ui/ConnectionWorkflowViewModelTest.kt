package dev.envoix.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionWorkflowViewModelTest {
    @Test
    fun `room and transfer draft identities remain stable across state changes`() {
        val viewModel = ConnectionWorkflowViewModel()
        val room = DeviceRoomDraft(displayName = "Nearby phone")

        viewModel.openRoom(room)
        assertEquals(WorkflowScreen.Room, viewModel.uiState.value.screen)
        assertEquals(
            room.id,
            viewModel.uiState.value.room
                ?.id,
        )

        viewModel.beginTransfer("send", usesPendingAction = false)
        val firstDraftId =
            viewModel.uiState.value.transferDraft
                ?.id
        viewModel.uiState.value.transferDraft
            ?.preparation
            ?.preparedJobId
            ?.value = "job-first-room"
        viewModel.beginTransfer("receive", usesPendingAction = true)
        assertEquals(
            firstDraftId,
            viewModel.uiState.value.transferDraft
                ?.id,
        )

        viewModel.dismissTransferDraft()
        viewModel.showRoomQr()
        assertEquals(
            "receive",
            viewModel.uiState.value.transferDraft
                ?.roleAdapter,
        )
        assertEquals(
            true,
            viewModel.uiState.value.transferDraft
                ?.showQrInitially,
        )
        viewModel.dismissTransferDraft()

        viewModel.beginTransfer("send", usesPendingAction = false)
        assertNotEquals(
            firstDraftId,
            viewModel.uiState.value.transferDraft
                ?.id,
        )
        assertNull(
            viewModel.uiState.value.transferDraft
                ?.preparation
                ?.preparedJobId
                ?.value,
        )
        assertEquals(
            room.id,
            viewModel.uiState.value.room
                ?.id,
        )
    }

    @Test
    fun `completed transfer attaches only to its current room`() {
        val viewModel = ConnectionWorkflowViewModel()
        viewModel.openRoom(DeviceRoomDraft(displayName = "First"))
        viewModel.beginTransfer("send", usesPendingAction = false)
        assertTrue(
            viewModel.uiState.value.transferDraft
                ?.preparation
                ?.transferOwnership() == true,
        )
        viewModel.completeTransferDraft("1234-alpha-beta", consumePendingShares = false)

        assertEquals(
            setOf("1234-alpha-beta"),
            viewModel.uiState.value.room
                ?.transferCodes,
        )
        assertNull(viewModel.uiState.value.transferDraft)

        viewModel.returnToHub()
        viewModel.openRoom(DeviceRoomDraft(displayName = "Second"))
        assertEquals(
            emptySet<String>(),
            viewModel.uiState.value.room
                ?.transferCodes,
        )
    }

    @Test
    fun `closing a room clears ephemeral setup but not the screen history owner`() {
        val viewModel = ConnectionWorkflowViewModel()
        viewModel.openRoom(
            DeviceRoomDraft(
                displayName = "Waiting room",
                hostedCode = "4321-alpha-beta",
                hostedPayload = "payload",
                directionAdapter = "receive",
            ),
        )

        assertEquals(
            "receive",
            viewModel.uiState.value.room
                ?.pendingRoleAdapter,
        )
        assertEquals(
            setOf("4321-alpha-beta"),
            viewModel.uiState.value.room
                ?.transferCodes,
        )

        viewModel.returnToHub()

        assertEquals(WorkflowScreen.Hub, viewModel.uiState.value.screen)
        assertNull(viewModel.uiState.value.room)
        assertNull(viewModel.uiState.value.transferDraft)
    }

    @Test
    fun `activity and settings return to the room that opened them`() {
        val viewModel = ConnectionWorkflowViewModel()
        val room = DeviceRoomDraft(displayName = "Nearby phone")
        viewModel.openRoom(room)

        viewModel.openActivity()
        assertEquals(WorkflowScreen.Activity, viewModel.uiState.value.screen)
        viewModel.openSettings()
        assertEquals(WorkflowScreen.Settings, viewModel.uiState.value.screen)

        viewModel.navigateBack()

        assertEquals(WorkflowScreen.Room, viewModel.uiState.value.screen)
        assertEquals(
            room.id,
            viewModel.uiState.value.room
                ?.id,
        )
    }
}
