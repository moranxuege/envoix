package dev.envoix.app.ui

import dev.envoix.app.CreatedInvite
import dev.envoix.app.ParsedInvite
import dev.envoix.app.R
import dev.envoix.app.Settings
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.NearbyInviteRoute
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
        var activityRequest: Triple<String, String, Boolean>? = null
        val viewModel =
            ConnectionWorkflowViewModel(
                invitationActivityReference = { reference, role, creator ->
                    activityRequest = Triple(reference, role, creator)
                    "654321"
                },
            )
        viewModel.openRoom(DeviceRoomDraft(displayName = "First"))
        viewModel.beginTransfer("send", usesPendingAction = false)
        assertTrue(
            viewModel.uiState.value.transferDraft
                ?.preparation
                ?.transferOwnership() == true,
        )
        viewModel.completeTransferDraft("1234-alpha-beta", consumePendingShares = false)

        assertEquals(Triple("1234-alpha-beta", "send", false), activityRequest)
        assertEquals(
            setOf("654321"),
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
    fun `completed receive uses the same secret-free activity identity`() {
        var activityRequest: Triple<String, String, Boolean>? = null
        val viewModel =
            ConnectionWorkflowViewModel(
                invitationActivityReference = { reference, role, creator ->
                    activityRequest = Triple(reference, role, creator)
                    "654321"
                },
            )
        viewModel.openRoom(DeviceRoomDraft(displayName = "Phone"))
        viewModel.beginTransfer("receive", usesPendingAction = false)
        viewModel.uiState.value.transferDraft
            ?.preparation
            ?.generatedInvite
            ?.value =
            CreatedInvite(
                roomCode = "123456-a1b2-c3d4",
                payload = "envoix://invite/v2/secret",
                reference = "123456-a1b2-c3d4",
                broker = "broker.example",
                relay = null,
                creatorRole = "receive",
                joinerRole = "send",
                expiresAt = 1,
            )
        assertTrue(
            viewModel.uiState.value.transferDraft
                ?.preparation
                ?.transferOwnership() == true,
        )

        viewModel.completeTransferDraft("123456-a1b2-c3d4", consumePendingShares = false)

        assertEquals(Triple("123456-a1b2-c3d4", "receive", true), activityRequest)
        assertEquals(
            setOf("654321"),
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
                pairingInput = "envoix://invite/v2/test-payload",
                directionAdapter = "receive",
            ),
        )

        assertEquals(
            "receive",
            viewModel.uiState.value.room
                ?.pendingRoleAdapter,
        )
        assertEquals(
            emptySet<String>(),
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
    fun `saved rooms navigate independently from one-time room state`() {
        val viewModel = ConnectionWorkflowViewModel()

        viewModel.openRooms()
        assertEquals(WorkflowScreen.Rooms, viewModel.uiState.value.screen)
        assertNull(viewModel.uiState.value.room)

        viewModel.openRememberedRoom("relationship-1")
        assertEquals(WorkflowScreen.RememberedRoom, viewModel.uiState.value.screen)
        assertEquals(
            "relationship-1",
            viewModel.uiState.value.selectedRememberedRelationshipId,
        )

        viewModel.navigateBack()
        assertEquals(WorkflowScreen.Rooms, viewModel.uiState.value.screen)
        assertNull(viewModel.uiState.value.selectedRememberedRelationshipId)

        viewModel.navigateBack()
        assertEquals(WorkflowScreen.Hub, viewModel.uiState.value.screen)
    }

    @Test
    fun `nearby delivery hosts without revealing the room invite`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()

            var deliveredInvite: String? = null
            viewModel.startNearbyRoom(TEST_SELECTION) { invite, completion ->
                deliveredInvite = invite
                completion(null)
            }
            runCurrent()

            assertEquals(RoomControlPhase.Hosting, viewModel.uiState.value.control.phase)
            assertEquals(TEST_INVITE.payload, deliveredInvite)
            assertFalse(viewModel.uiState.value.control.inviteRevealed)
            viewModel.endWaitingRoom()
            runCurrent()
        }

    @Test
    fun `Bluetooth delivery exposes only locator and displays verification code`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel = ConnectionWorkflowViewModel(gateway, { TEST_SETTINGS })
            runCurrent()
            var delivered: String? = null

            viewModel.startNearbyRoom(BLE_SELECTION) { invite, completion ->
                delivered = invite
                completion(null)
            }
            runCurrent()

            assertTrue(delivered?.startsWith("envoix://ble/v1/") == true)
            assertTrue(gateway.verifiedHostInput?.startsWith("envoix://room/") == true)
            assertFalse(gateway.verifiedHostInput == delivered)
            assertEquals(
                6,
                viewModel.uiState.value.control.verificationCode
                    ?.length,
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
            assertTrue(viewModel.uiState.value.control.inviteRevealed)

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
            assertEquals(TEST_ROOM_ENDPOINT, viewModel.uiState.value.control.endpoint)

            viewModel.endWaitingRoom()
            runCurrent()

            assertEquals(RoomControlPhase.Closed, viewModel.uiState.value.control.phase)
            assertEquals(RoomCloseReason.UserEnded, gateway.closedWith)
        }

    @Test
    fun `naked no-R code routes only to foreground room control`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                )
            runCurrent()

            viewModel.joinRoom("123456A1B2C3D4")
            runCurrent()

            assertEquals("123456A1B2C3D4", gateway.joinedInput)
            assertEquals(RoomControlPhase.Joining, viewModel.uiState.value.control.phase)
        }

    @Test
    fun `connected joiner remains live when Android backgrounds`() =
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
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = false,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime =
                        RoomLifetimeSnapshot(
                            revision = 1,
                            policy = RoomLifetimePolicy.Idle15Minutes,
                            idleDeadlineEpochMs = 900_000,
                        ),
                ),
            )
            runCurrent()

            assertEquals(TEST_ROOM_ENDPOINT, viewModel.uiState.value.control.endpoint)
            assertEquals(
                TEST_ROOM_ENDPOINT,
                viewModel.uiState.value.room
                    ?.controlEndpoint,
            )
            viewModel.setForeground(false)
            runCurrent()

            assertNull(gateway.closedWith)
            assertEquals(RoomControlPhase.Connected, viewModel.uiState.value.control.phase)
        }

    @Test
    fun `connected creator remains live when Android backgrounds`() =
        runTest(dispatcher) {
            val gateway = HostedInviteGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = { TEST_SETTINGS },
                    clockEpochMs = { 0L },
                )
            runCurrent()
            viewModel.revealRoomInvite()
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime =
                        RoomLifetimeSnapshot(
                            revision = 1,
                            policy = RoomLifetimePolicy.Idle15Minutes,
                            idleDeadlineEpochMs = 900_000,
                        ),
                ),
            )
            runCurrent()

            viewModel.setForeground(false)
            runCurrent()

            assertNull(gateway.closedWith)
            assertEquals(RoomControlPhase.Connected, viewModel.uiState.value.control.phase)
            viewModel.endRoom()
            runCurrent()
        }

    @Test
    fun `background hides hosted QR even while an external activity is open`() =
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
            assertTrue(viewModel.uiState.value.control.inviteRevealed)

            viewModel.setExternalActivityActive(true)
            viewModel.setForeground(false)
            assertFalse(viewModel.uiState.value.control.inviteRevealed)

            viewModel.setExternalActivityActive(false)
            runCurrent()

            assertFalse(viewModel.uiState.value.control.inviteRevealed)
            assertNull(gateway.closedWith)
            assertEquals(RoomControlPhase.Hosting, viewModel.uiState.value.control.phase)
        }

    @Test
    fun `accepting an incoming offer attaches its prepared receiver to the room`() =
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
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "Nearby phone",
                    creator = false,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime =
                        RoomLifetimeSnapshot(
                            revision = 1,
                            policy = RoomLifetimePolicy.Idle15Minutes,
                            idleDeadlineEpochMs = 900_000,
                        ),
                ),
            )
            runCurrent()
            gateway.emit(RoomControlEvent.IncomingOffer(TEST_TRANSFER_OFFER))
            runCurrent()

            var preparedReceivers = 0
            var canceledReceiver: Long? = null
            viewModel.acceptIncomingRoomOffer(
                parseInvitation = {
                    ParsedInvite(
                        reference = "private-transfer-reference",
                        broker = TEST_ROOM_ENDPOINT.broker,
                        relay = TEST_ROOM_ENDPOINT.relay,
                        creatorRole = "send",
                        joinerRole = "receive",
                        expiresAt = Long.MAX_VALUE,
                    )
                },
                onPrepareReceive = { _, _, _, _, _, completion ->
                    preparedReceivers += 1
                    completion(42L, null)
                },
                onCancelReceive = { canceledReceiver = it },
            )
            viewModel.acceptIncomingRoomOffer(
                parseInvitation = { error("a busy offer must not be parsed twice") },
                onPrepareReceive = { _, _, _, _, _, _ ->
                    error("a busy offer must not start a second receiver")
                },
                onCancelReceive = { canceledReceiver = it },
            )
            runCurrent()

            assertEquals(1, preparedReceivers)
            assertNull(canceledReceiver)
            assertEquals(TEST_TRANSFER_OFFER.id to true, gateway.respondedOffer)
            assertEquals(
                setOf("private-transfer-reference"),
                viewModel.uiState.value.room
                    ?.transferCodes,
            )
            assertNull(viewModel.uiState.value.control.incomingOffer)
        }

    @Test
    fun `incoming offer on another endpoint is rejected without preparing a receiver`() =
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
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "Nearby phone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime =
                        RoomLifetimeSnapshot(
                            revision = 1,
                            policy = RoomLifetimePolicy.Idle15Minutes,
                            idleDeadlineEpochMs = 900_000,
                        ),
                ),
            )
            gateway.emit(RoomControlEvent.IncomingOffer(TEST_TRANSFER_OFFER))
            runCurrent()

            var receiverPrepared = false
            viewModel.acceptIncomingRoomOffer(
                parseInvitation = {
                    ParsedInvite(
                        reference = "private-transfer-reference",
                        broker = "different-broker@127.0.0.1:8445",
                        relay = TEST_ROOM_ENDPOINT.relay,
                        creatorRole = "send",
                        joinerRole = "receive",
                        expiresAt = Long.MAX_VALUE,
                    )
                },
                onPrepareReceive = { _, _, _, _, _, _ -> receiverPrepared = true },
                onCancelReceive = {},
            )
            runCurrent()

            assertFalse(receiverPrepared)
            assertEquals(TEST_TRANSFER_OFFER.id to false, gateway.respondedOffer)
            assertEquals(
                UiMessage.Resource(R.string.room_file_offer_wrong_room),
                viewModel.uiState.value.control.error,
            )
            assertNull(viewModel.uiState.value.control.incomingOffer)

            val nextOffer = TEST_TRANSFER_OFFER.copy(id = "offer-after-rejection")
            gateway.emit(RoomControlEvent.IncomingOffer(nextOffer))
            runCurrent()
            assertEquals(nextOffer, viewModel.uiState.value.control.incomingOffer)
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
                code = "123456-a1b2-c3d4",
                payload = "envoix://room/123456-a1b2-c3d4",
                endpoint = TEST_ROOM_ENDPOINT,
                expiresAtEpochMs = Long.MAX_VALUE,
            )
        val TEST_SELECTION =
            NearbyPairingSelection(
                discoveryPeerKey = "nearby-peer",
                displayName = "Nearby phone",
                sources = setOf(DiscoverySource.Mdns, DiscoverySource.Bluetooth),
                nearbyInviteRoute =
                    NearbyInviteRoute.normalized(
                        endpointId = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst",
                        relayUrl = "https://relay.example",
                        directAddresses = emptyList(),
                    ),
            )
        val BLE_SELECTION =
            TEST_SELECTION.copy(
                sources = setOf(DiscoverySource.Bluetooth),
                nearbyInviteRoute = null,
            )
        val TEST_TRANSFER_OFFER =
            RoomTransferOffer(
                id = "offer-1",
                transferInvite = "envoix://invite/v2/redacted",
                rootNames = listOf("notes.txt"),
                itemCount = 1,
                directoryCount = 0,
                totalBytes = 42,
            )
        val TEST_SETTINGS =
            Settings(
                broker = TEST_ROOM_ENDPOINT.broker,
                relay = TEST_ROOM_ENDPOINT.relay,
                nearbyDisplayName = "Android phone",
            )
    }
}

private class HostedInviteGateway : RoomControlGateway {
    private val mutableEvents = MutableSharedFlow<RoomControlEvent>(extraBufferCapacity = 8)
    override val available = true
    override val events: Flow<RoomControlEvent> = mutableEvents
    var hostCalls = 0
    var closedWith: RoomCloseReason? = null
    var respondedOffer: Pair<String, Boolean>? = null
    var joinedInput: String? = null
    var verifiedHostInput: String? = null

    fun emit(event: RoomControlEvent) {
        check(mutableEvents.tryEmit(event))
    }

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostCalls += 1
        mutableEvents.emit(
            RoomControlEvent.Hosting(
                RoomControlInvite(
                    code = "123456-a1b2-c3d4",
                    payload = "envoix://room/123456-a1b2-c3d4",
                    endpoint = TEST_ROOM_ENDPOINT,
                    expiresAtEpochMs = Long.MAX_VALUE,
                ),
            ),
        )
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) {
        joinedInput = input
        mutableEvents.emit(RoomControlEvent.Joining(TEST_ROOM_ENDPOINT))
    }

    override suspend fun hostVerified(
        input: String,
        displayName: String,
        peerLabel: String,
    ) {
        hostCalls += 1
        verifiedHostInput = input
        mutableEvents.emit(
            RoomControlEvent.Hosting(
                RoomControlInvite(
                    code = "123456-v100-0000",
                    payload = input,
                    endpoint = TEST_ROOM_ENDPOINT,
                    expiresAtEpochMs = Long.MAX_VALUE,
                ),
            ),
        )
    }

    override suspend fun refreshInvite() = Unit

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) = Unit

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) {
        respondedOffer = offerId to accept
    }

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) = Unit

    override suspend fun updateTransferActive(active: Boolean) = Unit

    override suspend fun close(reason: RoomCloseReason) {
        closedWith = reason
    }
}

private val TEST_ROOM_ENDPOINT =
    RoomControlEndpoint(
        broker = "room-broker@127.0.0.1:8555",
        relay = "https://room-relay.example",
    )
