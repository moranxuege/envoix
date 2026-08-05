package dev.envoix.app

import dev.envoix.app.ui.ConnectionWorkflowUiState
import dev.envoix.app.ui.RoomControlEndpoint
import dev.envoix.app.ui.RoomControlInvite
import dev.envoix.app.ui.RoomControlPhase
import dev.envoix.app.ui.RoomControlUiState
import dev.envoix.app.ui.WorkflowScreen
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NfcInvitationHostingLifecycleTest {
    @Test
    fun `phone presentation lease leaves enough time to align both NFC coils`() {
        assertTrue(
            NfcInvitationHostController.PRESENTATION_LEASE_MS >= 120_000L,
        )
    }

    @Test
    fun `unchanged hidden invitation can be recomputed after pause clears process store`() {
        val workflow = hostingWorkflow(inviteRevealed = false)
        var processOnlyHostedValue = activeHostedNfcInvitation(workflow, nowEpochMs = 1)
        assertEquals(INVITATION, processOnlyHostedValue)

        processOnlyHostedValue = null
        assertNull(processOnlyHostedValue)

        processOnlyHostedValue = activeHostedNfcInvitation(workflow, nowEpochMs = 1)
        assertEquals(INVITATION, processOnlyHostedValue)
    }

    @Test
    fun `hosting publication requires Connect but ignores QR visibility`() {
        val hosting = hostingWorkflow(inviteRevealed = false)

        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(screen = WorkflowScreen.Settings),
                nowEpochMs = 1,
            ),
        )
        assertEquals(
            INVITATION,
            activeHostedNfcInvitation(
                hosting.copy(
                    control = hosting.control.copy(inviteRevealed = true),
                ),
                nowEpochMs = 1,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(
                    control = hosting.control.copy(phase = RoomControlPhase.Joining),
                ),
                nowEpochMs = 1,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(
                    control = hosting.control.copy(invite = null),
                ),
                nowEpochMs = 1,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(
                    control = hosting.control.copy(verificationCode = "123456"),
                ),
                nowEpochMs = 1,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting,
                nowEpochMs = Long.MAX_VALUE,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(
                    control =
                        hosting.control.copy(
                            invite =
                                requireNotNull(hosting.control.invite).copy(
                                    payload = "envoix://invite/v2/not-a-room",
                                ),
                        ),
                ),
                nowEpochMs = 1,
            ),
        )
        assertNull(
            activeHostedNfcInvitation(
                hosting.copy(
                    control =
                        hosting.control.copy(
                            invite =
                                requireNotNull(hosting.control.invite).copy(
                                    payload = "envoix://room/",
                                ),
                        ),
                ),
                nowEpochMs = 1,
            ),
        )
    }

    @Test
    fun `raw custom scheme views require canonical bytes and native validation`() {
        val fileInvite = "envoix://invite/v2/abc_DEF-123"
        val malformedRoomInvite =
            "envoix://room/123456-a1b2-c3d4" +
                "?broker=broker.example&expires=not-a-number"

        assertEquals(
            INVITATION,
            validatedRawNfcViewInvitation(
                value = INVITATION,
                validateRoomInvite = { true },
                validateTransferInvite = {
                    throw AssertionError("room URI entered the InviteV2 validator")
                },
            ),
        )
        assertEquals(
            fileInvite,
            validatedRawNfcViewInvitation(
                value = fileInvite,
                validateRoomInvite = {
                    throw AssertionError("InviteV2 entered the room validator")
                },
                validateTransferInvite = { true },
            ),
        )
        assertNull(
            validatedRawNfcViewInvitation(
                value = fileInvite,
                validateRoomInvite = { true },
                validateTransferInvite = { false },
            ),
        )
        assertNull(
            validatedRawNfcViewInvitation(
                value = malformedRoomInvite,
                validateRoomInvite = { false },
                validateTransferInvite = {
                    throw AssertionError("room URI entered the InviteV2 validator")
                },
            ),
        )
        assertNull(
            validatedRawNfcViewInvitation(
                value = "envoix://room/",
                validateRoomInvite = { true },
                validateTransferInvite = { true },
            ),
        )
        assertNull(
            validatedRawNfcViewInvitation(
                value =
                    requireNotNull(NfcInvitationContract.encode(INVITATION))
                        .toString(Charsets.US_ASCII),
                validateRoomInvite = { true },
                validateTransferInvite = { true },
            ),
        )
    }

    private fun hostingWorkflow(inviteRevealed: Boolean): ConnectionWorkflowUiState =
        ConnectionWorkflowUiState(
            screen = WorkflowScreen.Hub,
            control =
                RoomControlUiState(
                    phase = RoomControlPhase.Hosting,
                    invite =
                        RoomControlInvite(
                            code = "123456-a1b2-c3d4",
                            payload = INVITATION,
                            endpoint = RoomControlEndpoint("broker.example", ""),
                            expiresAtEpochMs = Long.MAX_VALUE,
                        ),
                    inviteRevealed = inviteRevealed,
                ),
        )

    private companion object {
        const val INVITATION =
            "envoix://room/123456-a1b2-c3d4" +
                "?broker=broker.example&expires=18446744073709551615"
    }
}
