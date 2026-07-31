package dev.envoix.app

import dev.envoix.app.ui.ConnectionWorkflowUiState
import dev.envoix.app.ui.RoomControlEndpoint
import dev.envoix.app.ui.RoomControlInvite
import dev.envoix.app.ui.RoomControlPhase
import dev.envoix.app.ui.RoomControlUiState
import dev.envoix.app.ui.WorkflowScreen
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NfcInvitationHostingLifecycleTest {
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

        assertEquals(
            INVITATION,
            validatedRawNfcViewInvitation(INVITATION) {
                throw AssertionError("room URI entered the InviteV2 validator")
            },
        )
        assertEquals(
            fileInvite,
            validatedRawNfcViewInvitation(fileInvite) { true },
        )
        assertNull(validatedRawNfcViewInvitation(fileInvite) { false })
        assertNull(validatedRawNfcViewInvitation("envoix://room/") { true })
        assertNull(
            validatedRawNfcViewInvitation(
                requireNotNull(NfcInvitationContract.encode(INVITATION))
                    .toString(Charsets.US_ASCII),
            ) { true },
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
                            code = "R123456-a1b2-c3d4",
                            payload = INVITATION,
                            endpoint = RoomControlEndpoint("broker.example", ""),
                            expiresAtEpochMs = Long.MAX_VALUE,
                        ),
                    inviteRevealed = inviteRevealed,
                ),
        )

    private companion object {
        const val INVITATION =
            "envoix://room/R123456-a1b2-c3d4" +
                "?broker=broker.example&expires=18446744073709551615"
    }
}
