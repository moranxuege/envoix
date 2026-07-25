package dev.envoix.app.ui

import dev.envoix.app.Settings
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyPairingSelection
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ConnectionWorkflowViewModelTest {
    private lateinit var dispatcher: TestDispatcher

    @Before
    fun installMainDispatcher() {
        dispatcher = StandardTestDispatcher()
        Dispatchers.setMain(dispatcher)
    }

    @After
    fun resetMainDispatcher() {
        Dispatchers.resetMain()
    }

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

    @Test
    fun `nearby delivery reuses a valid hosted invite without replacement`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()
            viewModel.revealRoomInvite()
            runCurrent()
            assertEquals(RoomControlPhase.Hosting, viewModel.uiState.value.control.phase)

            var deliveredInvite: String? = null
            viewModel.startNearbyRoom(TEST_SELECTION) { invite, completion ->
                deliveredInvite = invite
                completion(null)
            }

            assertEquals(TEST_INVITE.payload, deliveredInvite)
            assertEquals(1, gateway.hostCalls)
            assertFalse(viewModel.uiState.value.control.replacementRequested)
            assertEquals(
                TEST_SELECTION,
                viewModel.uiState.value.room
                    ?.nearbySelection,
            )
            viewModel.endWaitingRoom()
            runCurrent()
        }

    @Test
    fun `stop waiting closes the hosted room and clears its nearby target`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()
            viewModel.revealRoomInvite()
            runCurrent()
            viewModel.startNearbyRoom(TEST_SELECTION) { _, completion -> completion(null) }

            viewModel.endWaitingRoom()
            runCurrent()

            assertEquals(RoomControlPhase.Closed, viewModel.uiState.value.control.phase)
            assertEquals(RoomCloseReason.UserEnded, gateway.closedWith)
            assertNull(viewModel.uiState.value.room)
            assertNull(viewModel.uiState.value.transferDraft)
        }

    @Test
    fun `joining room exposes the same explicit cancel path`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()

            viewModel.joinRoom(TEST_INVITE.payload)
            runCurrent()
            assertEquals(RoomControlPhase.Joining, viewModel.uiState.value.control.phase)

            viewModel.endWaitingRoom()
            runCurrent()

            assertEquals(RoomControlPhase.Closed, viewModel.uiState.value.control.phase)
            assertEquals(RoomCloseReason.UserEnded, gateway.closedWith)
        }

    @Test
    fun `legacy replacement request moves to its dialog and can return`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()
            viewModel.openRoom(DeviceRoomDraft(displayName = "Legacy phone"))
            assertEquals(RoomControlPhase.Legacy, viewModel.uiState.value.control.phase)

            viewModel.joinRoom(TEST_INVITE.payload)

            assertEquals(WorkflowScreen.Hub, viewModel.uiState.value.screen)
            assertTrue(viewModel.uiState.value.control.replacementRequested)

            viewModel.returnToCurrentRoom()

            assertEquals(WorkflowScreen.Room, viewModel.uiState.value.screen)
            assertFalse(viewModel.uiState.value.control.replacementRequested)
            assertEquals(RoomControlPhase.Legacy, viewModel.uiState.value.control.phase)
        }

    private companion object {
        val TEST_INVITE =
            RoomControlInvite(
                code = "R123456-amber-comet",
                payload = "envoix://room/R123456-amber-comet",
                expiresAtEpochMs = Long.MAX_VALUE,
            )
        val TEST_SELECTION =
            NearbyPairingSelection(
                discoveryPeerKey = "nearby-peer",
                displayName = "Nearby phone",
                sources = setOf(DiscoverySource.Bluetooth),
            )
        val TEST_SETTINGS = Settings(nearbyDisplayName = "Android phone")
    }
}

private class HostedInviteGateway : RoomControlGateway {
    private val mutableEvents = MutableSharedFlow<RoomControlEvent>(extraBufferCapacity = 8)
    override val available = true
    override val events: Flow<RoomControlEvent> = mutableEvents
    var hostCalls = 0
    var closedWith: RoomCloseReason? = null

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostCalls += 1
        mutableEvents.emit(
            RoomControlEvent.Hosting(
                RoomControlInvite(
                    code = "R123456-amber-comet",
                    payload = "envoix://room/R123456-amber-comet",
                    expiresAtEpochMs = Long.MAX_VALUE,
                ),
            ),
        )
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) = Unit

    override suspend fun refreshInvite() = Unit

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) = Unit

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) = Unit

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) = Unit

    override suspend fun close(reason: RoomCloseReason) {
        closedWith = reason
    }
}
