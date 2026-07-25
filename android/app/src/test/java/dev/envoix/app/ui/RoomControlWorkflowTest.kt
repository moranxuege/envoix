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
            assertNull(connectedPeer)

            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = true,
                    policy = RoomLifetimePolicy.Idle15Minutes,
                ),
            )
            runCurrent()

            assertTrue(workflow.state.connected)
            assertEquals("iPhone", connectedPeer)
            assertNotNull(workflow.state.idleDeadlineMs)
        }

    @Test
    fun `hosting invitation closes at its wall clock expiry`() =
        runTest {
            val gateway = FakeRoomControlGateway()
            val workflow =
                RoomControlWorkflow(
                    gateway = gateway,
                    scope = backgroundScope,
                    nowMs = { testScheduler.currentTime },
                    wallClockMs = { testScheduler.currentTime },
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
                        code = "R123456-amber-comet",
                        payload = "envoix://room/R123456-amber-comet",
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
            assertEquals("invalid broker", workflow.state.error)
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
                    code = "R123456-amber-comet",
                    payload = "envoix://room/R123456-amber-comet",
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
            assertEquals("refresh failed", workflow.state.error)
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
            gateway.emit(
                RoomControlEvent.Connected(
                    peerName = "iPhone",
                    creator = false,
                    policy = RoomLifetimePolicy.Idle15Minutes,
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
    fun `active transfer pauses idle expiry and completion starts a fresh window`() =
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
                    policy = RoomLifetimePolicy.Idle15Minutes,
                ),
            )
            runCurrent()
            assertEquals(RoomControlWorkflow.ROOM_IDLE_TIMEOUT_MS, workflow.state.idleDeadlineMs)

            advanceTimeBy(60_000L)
            workflow.updateActiveTransfers(1)
            assertNull(workflow.state.idleDeadlineMs)
            advanceTimeBy(RoomControlWorkflow.ROOM_IDLE_TIMEOUT_MS)
            runCurrent()
            assertTrue(workflow.state.connected)

            workflow.updateActiveTransfers(0)
            val freshDeadline = testScheduler.currentTime + RoomControlWorkflow.ROOM_IDLE_TIMEOUT_MS
            assertEquals(freshDeadline, workflow.state.idleDeadlineMs)
            advanceTimeBy(RoomControlWorkflow.ROOM_IDLE_TIMEOUT_MS)
            runCurrent()

            assertEquals(RoomControlPhase.Closed, workflow.state.phase)
            assertEquals(RoomCloseReason.IdleExpired, gateway.closedWith)
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
                    policy = RoomLifetimePolicy.Idle15Minutes,
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
                    policy = RoomLifetimePolicy.Idle15Minutes,
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
        nowMs = { testScheduler.currentTime },
        wallClockMs = { testScheduler.currentTime },
        onStateChanged = states::add,
        onHosting = {},
        onConnected = onConnected,
        onCloseAcknowledged = {},
    )

    private companion object {
        val TEST_OFFER =
            RoomTransferOffer(
                id = "offer-1",
                transferInvite = "envoix://pair/123456-alpha-bravo?role=send",
                rootNames = listOf("Photos"),
                itemCount = 3,
                totalBytes = 42,
            )
        val TEST_DRAFT =
            RoomTransferOfferDraft(
                id = TEST_OFFER.id,
                transferInvite = TEST_OFFER.transferInvite,
                rootNames = TEST_OFFER.rootNames,
                itemCount = TEST_OFFER.itemCount,
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
    val offered = mutableListOf<RoomTransferOfferDraft>()
    val responses = mutableListOf<Pair<String, Boolean>>()

    suspend fun emit(event: RoomControlEvent) {
        mutableEvents.emit(event)
    }

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostError?.let(::error)
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) = Unit

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

    override suspend fun close(reason: RoomCloseReason) {
        closedWith = reason
    }
}
