package dev.envoix.app.ui

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RoomControlWorkflowTest {
    @Test
    fun `room becomes connected only after an authenticated gateway event`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val states = mutableListOf<RoomControlUiState>()
            var connectedPeer: String? = null
            val workflow =
                workflow(
                    gateway = gateway,
                    states = states,
                    onConnected = { peer, _ -> connectedPeer = peer },
                )
            workflow.start()
            runCurrent()

            workflow.host("Android", "broker", "relay")
            runCurrent()
            assertEquals(RoomControlPhase.Hosting, workflow.state.phase)
            assertFalse(workflow.state.connected)
            assertEquals(TEST_ROOM_ENDPOINT, workflow.state.endpoint)
            assertNull(connectedPeer)

            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(),
                ),
            )
            runCurrent()

            assertTrue(workflow.state.connected)
            assertEquals("iPhone", connectedPeer)
            assertEquals(TEST_ROOM_ENDPOINT, workflow.state.endpoint)
            assertEquals(900_000L, workflow.state.idleDeadlineEpochMs)
        }

    @Test
    fun `hosting invitation closes at its wall clock expiry`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow =
                RoomControlWorkflow(
                    gateway = gateway,
                    scope = backgroundScope,
                    clockEpochMs = { testScheduler.currentTime },
                    onStateChanged = {},
                    onHosting = {},
                    onConnected = { _, _ -> },
                    onCloseAcknowledged = {},
                )
            workflow.start()
            runCurrent()
            workflow.host("Android", "broker", "relay")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Hosting(
                    RoomControlInvite(
                        code = "123456-a1b2-c3d4",
                        payload = "envoix://room/123456-a1b2-c3d4",
                        endpoint = TEST_ROOM_ENDPOINT,
                        expiresAtEpochMs = 5_000L,
                    ),
                ),
            )
            runCurrent()

            advanceTimeBy(5_000L)
            runCurrent()

            assertEquals(RoomControlPhase.Closed, workflow.state.phase)
            assertEquals(RoomCloseReason.InvitationExpired, workflow.state.closeReason)
            assertEquals(RoomCloseReason.InvitationExpired, gateway.closedWith)
        }

    @Test
    fun `hosting startup failure leaves no phantom live room`() =
        runTest {
            val gateway = FakeRoomControlGateway().apply { hostError = "invalid broker" }
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()

            workflow.host("Android", "bad broker", "relay")
            runCurrent()

            assertEquals(RoomControlPhase.Failed, workflow.state.phase)
            assertFalse(workflow.state.live)
            assertEquals(UiMessage.Dynamic("invalid broker"), workflow.state.error)
        }

    @Test
    fun `late native terminal event cannot close a legacy room`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.setLegacy("Older device")

            gateway.emit(RoomControlEvent.Closed(RoomCloseReason.PeerEnded))
            runCurrent()

            assertEquals(RoomControlPhase.Legacy, workflow.state.phase)
            assertEquals("Older device", workflow.state.peerName)
        }

    @Test
    fun `failed refresh preserves the current invitation`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            val invite =
                RoomControlInvite(
                    code = "123456-a1b2-c3d4",
                    payload = "envoix://room/123456-a1b2-c3d4",
                    endpoint = TEST_ROOM_ENDPOINT,
                    expiresAtEpochMs = 60_000L,
                )
            workflow.host("Android", "broker", "relay")
            runCurrent()
            gateway.emit(RoomControlEvent.Hosting(invite))
            runCurrent()
            gateway.refreshError = "refresh failed"

            workflow.refreshInvite()
            runCurrent()

            assertEquals(RoomControlPhase.Hosting, workflow.state.phase)
            assertEquals(invite, workflow.state.invite)
            assertEquals(UiMessage.Dynamic("refresh failed"), workflow.state.error)
        }

    @Test
    fun `incoming acceptance is retained until gateway confirms the response`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.join(TEST_OFFER.transferInvite, "Android", "iPhone")
            runCurrent()
            assertEquals(TEST_ROOM_ENDPOINT, workflow.state.endpoint)
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = false,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(),
                ),
            )
            gateway.emit(RoomControlEvent.IncomingOffer(TEST_OFFER))
            runCurrent()
            assertEquals(TEST_OFFER, workflow.state.incomingOffer)

            val responseGate = CompletableDeferred<Unit>()
            gateway.responseGate = responseGate
            var completion: String? = "not completed"
            workflow.respondToOffer(
                offerId = TEST_OFFER.id,
                accept = true,
                completion = { completion = it },
            )
            runCurrent()

            assertEquals(TEST_OFFER, workflow.state.incomingOffer)
            assertEquals("not completed", completion)

            responseGate.complete(Unit)
            runCurrent()

            assertNull(workflow.state.incomingOffer)
            assertNull(completion)
        }

    @Test
    fun `creator uses only transmitted lifetime snapshots and reports transfer activity edges`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.host("Android", "broker", "relay")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(),
                ),
            )
            runCurrent()
            assertEquals(900_000L, workflow.state.idleDeadlineEpochMs)

            workflow.updateActiveTransfers(1)
            runCurrent()
            assertEquals(listOf(true), gateway.transferActivity)
            assertEquals(900_000L, workflow.state.idleDeadlineEpochMs)
            gateway.emit(RoomControlEvent.LifetimeChanged(lifetime(revision = 2, deadlineEpochMs = null)))
            runCurrent()
            assertNull(workflow.state.idleDeadlineEpochMs)

            workflow.updateActiveTransfers(2)
            workflow.updateActiveTransfers(0)
            runCurrent()
            assertEquals(listOf(true, false), gateway.transferActivity)
            assertNull(workflow.state.idleDeadlineEpochMs)

            gateway.emit(RoomControlEvent.LifetimeChanged(lifetime(revision = 3, deadlineEpochMs = 5_000L)))
            runCurrent()
            assertEquals(5_000L, workflow.state.idleDeadlineEpochMs)
            advanceTimeBy(5_000L)
            runCurrent()
            assertTrue(workflow.state.connected)
            assertEquals(listOf(RoomCloseReason.IdleExpired), gateway.closeReasons)

            gateway.emit(RoomControlEvent.Closed(RoomCloseReason.IdleExpired))
            runCurrent()
            assertEquals(RoomControlPhase.Closed, workflow.state.phase)
            assertEquals(RoomCloseReason.IdleExpired, gateway.closedWith)
        }

    @Test
    fun `failed creator expiry stays connected until a newer lifetime permits one new attempt`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.host("Android", "broker", "relay")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(deadlineEpochMs = 1_000L),
                ),
            )
            runCurrent()

            advanceTimeBy(1_000L)
            runCurrent()
            assertEquals(listOf(RoomCloseReason.IdleExpired), gateway.closeReasons)
            assertTrue(workflow.state.connected)

            gateway.emit(
                RoomControlEvent.CommandFailed(
                    command = "close",
                    offerId = null,
                    message = "authoritative deadline changed",
                ),
            )
            runCurrent()
            assertTrue(workflow.state.connected)
            gateway.emit(RoomControlEvent.LifetimeChanged(lifetime(revision = 2, deadlineEpochMs = 3_000L)))
            runCurrent()

            advanceTimeBy(2_000L)
            runCurrent()
            assertEquals(
                listOf(RoomCloseReason.IdleExpired, RoomCloseReason.IdleExpired),
                gateway.closeReasons,
            )
            assertTrue(workflow.state.connected)
        }

    @Test
    fun `joiner ignores stale lifetime revisions and never expires the creator deadline`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.join(TEST_OFFER.transferInvite, "Android", "iPhone")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = false,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(revision = 7, deadlineEpochMs = 5_000L),
                ),
            )
            runCurrent()

            gateway.emit(
                RoomControlEvent.LifetimeChanged(
                    lifetime(
                        revision = 6,
                        deadlineEpochMs = null,
                        policy = RoomLifetimePolicy.UntilForegroundEnds,
                    ),
                ),
            )
            runCurrent()
            assertEquals(7L, workflow.state.lifetimeRevision)
            assertEquals(RoomLifetimePolicy.Idle15Minutes, workflow.state.policy)
            assertEquals(5_000L, workflow.state.idleDeadlineEpochMs)

            advanceTimeBy(5_000L)
            runCurrent()
            assertTrue(workflow.state.connected)
            assertNull(gateway.closedWith)

            gateway.emit(RoomControlEvent.LifetimeChanged(lifetime(revision = 8, deadlineEpochMs = null)))
            runCurrent()
            assertEquals(8L, workflow.state.lifetimeRevision)
            assertNull(workflow.state.idleDeadlineEpochMs)
        }

    @Test
    fun `only one unresolved offer can be sent`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.host("Android", "broker", "relay")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(),
                ),
            )
            runCurrent()

            var firstResult: String? = "waiting"
            var secondResult: String? = null
            workflow.offer(TEST_DRAFT) { firstResult = it }
            workflow.offer(TEST_DRAFT.copy(id = "offer-2")) { secondResult = it }
            runCurrent()

            assertEquals(1, gateway.offered.size)
            assertEquals("waiting", firstResult)
            assertNotNull(secondResult)

            gateway.emit(RoomControlEvent.OfferAccepted(TEST_DRAFT.id))
            runCurrent()
            assertNull(firstResult)
        }

    @Test
    fun `unanswered incoming offer is rejected after sixty seconds`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow = workflow(gateway)
            workflow.start()
            runCurrent()
            workflow.join(TEST_OFFER.transferInvite, "Android", "iPhone")
            runCurrent()
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = false,
                    endpoint = TEST_ROOM_ENDPOINT,
                    lifetime = lifetime(),
                ),
            )
            gateway.emit(RoomControlEvent.IncomingOffer(TEST_OFFER))
            runCurrent()

            advanceTimeBy(RoomControlWorkflow.INCOMING_OFFER_TIMEOUT_MS)
            runCurrent()

            assertNull(workflow.state.incomingOffer)
            assertEquals(listOf(TEST_OFFER.id to false), gateway.responses)
        }

    private fun kotlinx.coroutines.test.TestScope.workflow(
        gateway: FakeRoomControlGateway,
        states: MutableList<RoomControlUiState> = mutableListOf(),
        onConnected: (String, Boolean) -> Unit = { _, _ -> },
    ) = RoomControlWorkflow(
        gateway = gateway,
        scope = backgroundScope,
        clockEpochMs = { testScheduler.currentTime },
        onStateChanged = states::add,
        onHosting = {},
        onConnected = onConnected,
        onCloseAcknowledged = {},
    )

    private companion object {
        fun lifetime(
            revision: Long = 1,
            deadlineEpochMs: Long? = 900_000L,
            policy: RoomLifetimePolicy = RoomLifetimePolicy.Idle15Minutes,
        ) = RoomLifetimeSnapshot(
            revision = revision,
            policy = policy,
            idleDeadlineEpochMs = deadlineEpochMs,
        )

        val TEST_OFFER =
            RoomTransferOffer(
                id = "offer-1",
                transferInvite = "envoix://invite/v2/test-payload",
                rootNames = listOf("Photos"),
                itemCount = 3,
                directoryCount = 1,
                totalBytes = 42,
            )
        val TEST_DRAFT =
            RoomTransferOfferDraft(
                id = TEST_OFFER.id,
                transferInvite = TEST_OFFER.transferInvite,
                rootNames = TEST_OFFER.rootNames,
                itemCount = TEST_OFFER.itemCount,
                directoryCount = TEST_OFFER.directoryCount,
                totalBytes = TEST_OFFER.totalBytes,
            )
    }
}

private class FakeRoomControlGateway : RoomControlGateway {
    private val mutableEvents = MutableSharedFlow<RoomControlEvent>(extraBufferCapacity = 16)
    override val available = true
    override val events: Flow<RoomControlEvent> = mutableEvents
    var responseGate: CompletableDeferred<Unit>? = null
    var hostError: String? = null
    var refreshError: String? = null
    var closedWith: RoomCloseReason? = null
    val closeReasons = mutableListOf<RoomCloseReason>()
    val offered = mutableListOf<RoomTransferOfferDraft>()
    val responses = mutableListOf<Pair<String, Boolean>>()
    val transferActivity = mutableListOf<Boolean>()

    suspend fun emit(event: RoomControlEvent) {
        mutableEvents.emit(event)
    }

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostError?.let(::error)
        mutableEvents.emit(RoomControlEvent.Hosting(TEST_ROOM_INVITE))
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) {
        mutableEvents.emit(RoomControlEvent.Joining(TEST_ROOM_ENDPOINT))
    }

    override suspend fun refreshInvite() {
        refreshError?.let(::error)
    }

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) {
        offered += draft
    }

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) {
        responseGate?.await()
        responses += offerId to accept
    }

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) = Unit

    override suspend fun updateTransferActive(active: Boolean) {
        transferActivity += active
    }

    override suspend fun close(reason: RoomCloseReason) {
        closeReasons += reason
        closedWith = reason
    }
}

private val TEST_ROOM_ENDPOINT =
    RoomControlEndpoint(
        broker = "room-broker@127.0.0.1:8555",
        relay = "https://room-relay.example",
    )

private val TEST_ROOM_INVITE =
    RoomControlInvite(
        code = "123456-a1b2-c3d4",
        payload = "envoix://room/123456-a1b2-c3d4",
        endpoint = TEST_ROOM_ENDPOINT,
        expiresAtEpochMs = Long.MAX_VALUE,
    )
