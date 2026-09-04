package dev.envoix.app.ui

import dev.envoix.app.ffi.FfiFailureCode
import dev.envoix.app.ffi.FfiRememberedCredentialVault
import dev.envoix.app.ffi.FfiRememberedRoomConnectException
import dev.envoix.app.ffi.FfiRoomCloseReason
import dev.envoix.app.ffi.FfiRoomControlEvent
import dev.envoix.app.ffi.FfiRoomControlException
import dev.envoix.app.ffi.FfiRoomControlInvite
import dev.envoix.app.ffi.FfiRoomControlSnapshot
import dev.envoix.app.ffi.FfiRoomLifetimePolicy
import dev.envoix.app.ffi.FfiRoomLifetimeState
import dev.envoix.app.ffi.FfiRoomOfferRejection
import dev.envoix.app.ffi.FfiRoomTransferOffer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

@OptIn(ExperimentalCoroutinesApi::class)
class NativeRoomControlGatewayTest {
    private val filesDirectory = File("/data/user/0/dev.envoix.app/files")

    @Test
    fun `one-time room keeps the legacy transport identity`() {
        assertEquals(
            File(filesDirectory, "room-control/identity.json").absolutePath,
            roomControlIdentityPath(filesDirectory, rememberedRelationshipId = null),
        )
    }

    @Test
    fun `remembered rooms receive stable distinct transport identities`() {
        val first = roomControlIdentityPath(filesDirectory, "relationship-a")
        val firstAgain = roomControlIdentityPath(filesDirectory, "relationship-a")
        val second = roomControlIdentityPath(filesDirectory, "relationship-b")

        assertEquals(first, firstAgain)
        assertNotEquals(first, second)
        assertTrue(
            first.matches(
                Regex(".*/room-control/remembered/[0-9a-f]{64}/identity\\.json$"),
            ),
        )
        assertFalse(first.contains("relationship-a"))
    }

    @Test
    fun `remembered room identity rejects an empty relationship`() {
        assertThrows(IllegalArgumentException::class.java) {
            roomControlIdentityPath(filesDirectory, "")
        }
    }

    @Test
    fun `host projects typed invitation and authenticated snapshot`() =
        runTest {
            val session = FakeRoomControlSession(snapshot(peerName = "MacBook"))
            val native = FakeRoomControlNativeCore(session)
            val gateway = gateway(native)
            val observed = backgroundScope.collect(gateway, 2)

            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()

            val events = observed.await()
            val hosting = events[0] as RoomControlEvent.Hosting
            val connected = events[1] as RoomControlEvent.Connected
            assertEquals("123456-a1b2-c3d4", hosting.invite.code)
            assertEquals("MacBook", connected.peerName)
            assertEquals("broker.test", connected.endpoint.broker)
            assertEquals("relay.test", connected.endpoint.relay)
            assertEquals("/tmp/room-control.json", native.connectionRequests.single().identityPath)
        }

    @Test
    fun `verified pairing vault accepts only a non-empty initial credential`() {
        val credential = byteArrayOf(1, 2, 3)
        var persisted: Triple<String, RoomControlEndpoint, ByteArray>? = null
        val endpoint = RoomControlEndpoint("broker.test", "relay.test")
        val vault =
            RoomControlCredentialVault("MacBook", endpoint) { label, savedEndpoint, value ->
                persisted = Triple(label, savedEndpoint, value)
                true
            }

        assertFalse(vault.storeRememberedCredential(byteArrayOf(), 0uL))
        assertFalse(vault.storeRememberedCredential(credential, 1uL))
        assertTrue(vault.storeRememberedCredential(credential, 0uL))
        assertEquals("MacBook", persisted?.first)
        assertEquals(endpoint, persisted?.second)
        assertTrue(credential.contentEquals(persisted?.third))
    }

    @Test
    fun `offer response returns only after the typed write completes`() =
        runTest {
            val delivered = CompletableDeferred<Unit>()
            val session =
                FakeRoomControlSession(snapshot()).apply {
                    acceptOffer = { offerId ->
                        acceptedOfferIds += offerId
                        delivered.await()
                        null
                    }
                }
            val native = FakeRoomControlNativeCore(session)
            val gateway = gateway(native)
            backgroundScope.collect(gateway, 2)
            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()

            val response = backgroundScope.async { gateway.respondToOffer("offer-7", true) }
            runCurrent()
            assertFalse(response.isCompleted)

            delivered.complete(Unit)
            runCurrent()
            response.await()
            assertEquals(listOf("offer-7"), session.acceptedOfferIds)
        }

    @Test
    fun `typed command rejection keeps the connected room usable`() =
        runTest {
            val session =
                FakeRoomControlSession(snapshot()).apply {
                    setPolicy = {
                        throw FfiRoomControlException.Rejected("deadline is still active")
                    }
                }
            val native = FakeRoomControlNativeCore(session)
            val gateway = gateway(native)
            val connected = backgroundScope.collect(gateway, 2)
            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()
            connected.await()

            val failure = runCatching { gateway.updatePolicy(RoomLifetimePolicy.UntilForegroundEnds) }
            assertTrue(failure.exceptionOrNull() is IllegalStateException)
            assertFalse(native.cancellations.single().canceled)

            session.setPolicy = { lifetime(revision = 2uL) }
            val changed = backgroundScope.collect(gateway, 1)
            gateway.updatePolicy(RoomLifetimePolicy.UntilForegroundEnds)
            runCurrent()
            val event = changed.await().single() as RoomControlEvent.LifetimeChanged
            assertEquals(2L, event.lifetime.revision)
        }

    @Test
    fun `typed network loss closes without inspecting diagnostic text`() =
        runTest {
            val session = FakeRoomControlSession(snapshot())
            val native = FakeRoomControlNativeCore(session)
            val gateway = gateway(native)
            val observed = backgroundScope.collect(gateway, 3)
            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()

            session.events.send(
                Result.failure(
                    FfiRoomControlException.NetworkLost(
                        "protocol failure words deliberately appear in this diagnostic",
                    ),
                ),
            )
            runCurrent()

            val closed = observed.await().last() as RoomControlEvent.Closed
            assertEquals(RoomCloseReason.NetworkLost, closed.reason)
        }

    @Test
    fun `remembered connection keeps typed broker recovery metadata`() =
        runTest {
            val native =
                FakeRoomControlNativeCore(FakeRoomControlSession(snapshot())).apply {
                    connectRemembered = { _, _ ->
                        throw FfiRememberedRoomConnectException.Failed(
                            reason = "expired rendezvous",
                            peerAuthenticated = false,
                            failureCode = FfiFailureCode.ROOM_EXPIRED,
                            retryAfterSeconds = 17uL,
                        )
                    }
                }
            val gateway = gateway(native)
            val observed = backgroundScope.collect(gateway, 1)

            gateway.connectRemembered(
                credentialReference = "credential-7",
                generation = 7,
                displayName = "Android",
                role = RememberedRoomConnectRole.Connector,
                broker = "broker.test",
                relay = "relay.test",
            )
            runCurrent()

            val failed = observed.await().single() as RoomControlEvent.Failed
            assertEquals(7L, failed.attemptedRememberedGeneration)
            assertEquals(RoomConnectFailureCode.RoomExpired, failed.failureCode)
            assertEquals(17L, failed.retryAfterSeconds)
            assertFalse(failed.peerAuthenticated)
        }

    @Test
    fun `replacement ignores a canceled generation that connects late`() =
        runTest {
            val firstConnection = CompletableDeferred<RoomControlNativeSession>()
            val secondSession = FakeRoomControlSession(snapshot(peerName = "Second peer"))
            val native =
                FakeRoomControlNativeCore(secondSession).apply {
                    var invitationNumber = 0
                    makeInvitation = { broker, relay ->
                        invitationNumber += 1
                        invitation(
                            payload = "envoix://room/$invitationNumber",
                            broker = broker,
                            relay = relay,
                        )
                    }
                    connect = { request, _ ->
                        if (request.input.endsWith("/1")) firstConnection.await() else secondSession
                    }
                }
            val gateway = gateway(native)
            val observed = backgroundScope.collect(gateway, 3)

            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()
            gateway.host("Android", "broker.test", "relay.test")
            runCurrent()
            firstConnection.complete(FakeRoomControlSession(snapshot(peerName = "Stale peer")))
            runCurrent()

            val events = observed.await()
            assertTrue(events[0] is RoomControlEvent.Hosting)
            assertTrue(events[1] is RoomControlEvent.Hosting)
            assertEquals("Second peer", (events[2] as RoomControlEvent.Connected).peerName)
            assertTrue(native.cancellations.first().canceled)
        }

    private fun kotlinx.coroutines.CoroutineScope.collect(
        gateway: NativeRoomControlGateway,
        count: Int,
    ) = async { gateway.events.take(count).toList() }

    private fun kotlinx.coroutines.test.TestScope.gateway(native: RoomControlNativeCore) =
        NativeRoomControlGateway(
            identityPath = "/tmp/room-control.json",
            persistVerifiedDevice = { _, _, _ -> true },
            native = native,
            scope = backgroundScope,
        )
}

private class FakeRoomControlNativeCore(
    private val defaultSession: RoomControlNativeSession,
) : RoomControlNativeCore {
    val cancellations = mutableListOf<FakeRoomControlCancellation>()
    val connectionRequests = mutableListOf<RoomControlConnectionRequest>()
    var makeInvitation: (String, String) -> FfiRoomControlInvite = { broker, relay ->
        invitation(broker = broker, relay = relay)
    }
    var connect:
        suspend (RoomControlConnectionRequest, RoomControlNativeCancellation) ->
        RoomControlNativeSession = { _, _ -> defaultSession }
    var connectRemembered:
        suspend (RememberedRoomControlConnectionRequest, RoomControlNativeCancellation) ->
        RoomControlNativeSession = { _, _ -> defaultSession }

    override fun makeInvitation(
        broker: String,
        relay: String,
    ): FfiRoomControlInvite = makeInvitation.invoke(broker, relay)

    override fun parseInvitation(
        input: String,
        fallbackBroker: String,
        fallbackRelay: String,
    ): FfiRoomControlInvite =
        invitation(
            payload = input,
            broker = fallbackBroker,
            relay = fallbackRelay,
        )

    override fun newCancellation(): RoomControlNativeCancellation = FakeRoomControlCancellation().also(cancellations::add)

    override suspend fun connect(
        request: RoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession {
        connectionRequests += request
        return connect.invoke(request, cancellation)
    }

    override suspend fun connectRemembered(
        request: RememberedRoomControlConnectionRequest,
        cancellation: RoomControlNativeCancellation,
    ): RoomControlNativeSession = connectRemembered.invoke(request, cancellation)
}

private class FakeRoomControlCancellation : RoomControlNativeCancellation {
    var canceled = false
    var released = false

    override fun cancel() {
        canceled = true
    }

    override fun close() {
        released = true
    }
}

private class FakeRoomControlSession(
    private val currentSnapshot: FfiRoomControlSnapshot,
    private val credentialToStore: ByteArray? = null,
) : RoomControlNativeSession {
    val events = Channel<Result<FfiRoomControlEvent>>(Channel.UNLIMITED)
    val acceptedOfferIds = mutableListOf<String>()
    var acceptOffer: suspend (String) -> FfiRoomLifetimeState? = { null }
    var setPolicy: suspend (FfiRoomLifetimePolicy) -> FfiRoomLifetimeState? = { null }

    override fun snapshot(): FfiRoomControlSnapshot = currentSnapshot

    override fun storePairingCredential(vault: FfiRememberedCredentialVault): Boolean =
        credentialToStore?.let { vault.storeRememberedCredential(it, 0uL) } ?: false

    override suspend fun nextEvent(): FfiRoomControlEvent = events.receive().getOrThrow()

    override suspend fun offerTransfer(offer: FfiRoomTransferOffer): FfiRoomLifetimeState? = null

    override suspend fun acceptOffer(offerId: String): FfiRoomLifetimeState? = acceptOffer.invoke(offerId)

    override suspend fun rejectOffer(
        offerId: String,
        reason: FfiRoomOfferRejection,
    ): FfiRoomLifetimeState? = null

    override suspend fun setPolicy(policy: FfiRoomLifetimePolicy): FfiRoomLifetimeState? = setPolicy.invoke(policy)

    override suspend fun setLocalTransferActive(active: Boolean): FfiRoomLifetimeState? = null

    override suspend fun close(reason: FfiRoomCloseReason) = Unit

    override fun close() = Unit
}

private fun invitation(
    payload: String = "envoix://room/123456-a1b2-c3d4",
    broker: String = "broker.test",
    relay: String = "relay.test",
) = FfiRoomControlInvite(
    code = "123456-a1b2-c3d4",
    payload = payload,
    broker = broker,
    relay = relay,
    expiresAtEpochMs = 4_102_444_800_000uL,
)

private fun snapshot(
    peerName: String = "Peer",
    rememberedGeneration: ULong? = null,
) = FfiRoomControlSnapshot(
    peerName = peerName,
    creator = true,
    rememberedGeneration = rememberedGeneration,
    lifetime = lifetime(),
)

private fun lifetime(revision: ULong = 1uL) =
    FfiRoomLifetimeState(
        revision = revision,
        policy = FfiRoomLifetimePolicy.IDLE15_MINUTES,
        idleDeadlineEpochMs = 4_102_444_800_000uL,
    )
