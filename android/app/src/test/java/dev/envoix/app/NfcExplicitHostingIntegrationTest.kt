package dev.envoix.app

import dev.envoix.app.ui.ConnectionWorkflowViewModel
import dev.envoix.app.ui.RoomCloseReason
import dev.envoix.app.ui.RoomControlEndpoint
import dev.envoix.app.ui.RoomControlEvent
import dev.envoix.app.ui.RoomControlGateway
import dev.envoix.app.ui.RoomControlInvite
import dev.envoix.app.ui.RoomControlPhase
import dev.envoix.app.ui.RoomLifetimePolicy
import dev.envoix.app.ui.RoomTransferOfferDraft
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class NfcExplicitHostingIntegrationTest {
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
    fun `foreground stays reader capable until explicit NFC share arms hidden HCE`() =
        runTest(dispatcher) {
            val gateway = ExplicitHostingGateway()
            val viewModel =
                ConnectionWorkflowViewModel(
                    gateway = gateway,
                    currentSettings = {
                        Settings(
                            nearbyDisplayName = "Android phone",
                            broker = "broker.example",
                            relay = "",
                        )
                    },
                )
            runCurrent()

            var armCalls = 0
            val platform = RecordingHostingPlatform()
            val hostingSession =
                NfcSafeHostingSession(
                    platform = platform,
                    armInvitation = {
                        armCalls += 1
                        true
                    },
                    clearInvitation = {},
                )
            hostingSession.onResume()
            backgroundScope.launch {
                viewModel.uiState
                    .map { activeHostedNfcInvitation(it, nowEpochMs = 1L) }
                    .distinctUntilChanged()
                    .collect(hostingSession::setInvitation)
            }
            runCurrent()

            // Merely opening Connect must not create a room, disable Android's
            // normal NFC polling, or publish an HCE carrier.
            viewModel.setForeground(true)
            runCurrent()

            assertEquals(RoomControlPhase.None, viewModel.uiState.value.control.phase)
            assertEquals(0, gateway.hostCalls)
            assertEquals(0, platform.listenOnlyCalls)
            assertFalse(hostingSession.state.value.armed)

            // The presenter/listener role begins only after this explicit user
            // action. QR visibility is independent from HCE publication.
            viewModel.shareRoomViaNfc()
            runCurrent()
            val hiddenState = viewModel.uiState.value
            val hiddenPayload = activeHostedNfcInvitation(hiddenState, nowEpochMs = 1L)

            assertEquals(RoomControlPhase.Hosting, hiddenState.control.phase)
            assertFalse(hiddenState.control.inviteRevealed)
            assertEquals(TEST_INVITATION.payload, hiddenPayload)
            assertEquals(1, gateway.hostCalls)
            assertEquals(1, platform.listenOnlyCalls)
            assertTrue(hostingSession.state.value.armed)
            assertEquals(1, armCalls)

            viewModel.revealRoomInvite()
            runCurrent()
            val revealedState = viewModel.uiState.value
            val revealedPayload = activeHostedNfcInvitation(revealedState, nowEpochMs = 1L)

            assertTrue(revealedState.control.inviteRevealed)
            assertEquals(hiddenPayload, revealedPayload)
            assertEquals(1, gateway.hostCalls)
            assertEquals(1, armCalls)
            assertTrue(hostingSession.state.value.armed)
        }
}

private class RecordingHostingPlatform : NfcSafeHostingPlatform {
    var listenOnlyCalls = 0

    override fun unavailableStatus(): NfcPhoneHostingStatus? = null

    override fun enterListenOnly(): Boolean {
        listenOnlyCalls += 1
        return true
    }

    override fun resetDiscoveryTechnology() = Unit

    override fun preferHostService(): Boolean = true

    override fun unsetPreferredHostService() = Unit
}

private class ExplicitHostingGateway : RoomControlGateway {
    private val mutableEvents = MutableSharedFlow<RoomControlEvent>(extraBufferCapacity = 4)
    override val available = true
    override val events: Flow<RoomControlEvent> = mutableEvents
    var hostCalls = 0

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostCalls += 1
        mutableEvents.emit(RoomControlEvent.Hosting(TEST_INVITATION))
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

    override suspend fun updateTransferActive(active: Boolean) = Unit

    override suspend fun close(reason: RoomCloseReason) = Unit
}

private val TEST_INVITATION =
    RoomControlInvite(
        code = "123456-abcd-efgh",
        payload =
            "envoix://room/123456-abcd-efgh" +
                "?broker=broker.example&expires=18446744073709551615",
        endpoint =
            RoomControlEndpoint(
                broker = "broker.example",
                relay = "",
            ),
        expiresAtEpochMs = Long.MAX_VALUE,
    )
