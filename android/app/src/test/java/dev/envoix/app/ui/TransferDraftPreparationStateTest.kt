package dev.envoix.app.ui

import dev.envoix.app.CreatedInvite
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferDraftPreparationStateTest {
    @Test
    fun `discard schedules cleanup exactly once and prevents a late start`() {
        var cleanupCalls = 0
        val state =
            TransferDraftPreparationState(
                onDiscard = { cleanupCalls += 1 },
            )

        assertTrue(state.discard())
        assertFalse(state.discard())
        assertFalse(state.transferOwnership())
        assertFalse(state.acceptsPreparationChanges())
        assertEquals(1, cleanupCalls)
    }

    @Test
    fun `started draft claims ownership exactly once and is never cleaned as abandoned`() {
        var cleanupCalls = 0
        var startCalls = 0
        val state =
            TransferDraftPreparationState(
                onDiscard = { cleanupCalls += 1 },
            )

        repeat(2) {
            if (state.transferOwnership()) startCalls += 1
        }

        assertEquals(1, startCalls)
        assertTrue(state.ownershipWasTransferred())
        assertFalse(state.discard())
        assertFalse(state.acceptsPreparationChanges())
        assertEquals(0, cleanupCalls)
    }

    @Test
    fun `failed room acceptance can release a claimed receiver for retry`() {
        val state = TransferDraftPreparationState(onDiscard = {})

        assertTrue(state.transferOwnership())
        assertTrue(state.rollbackTransferredOwnership())
        assertFalse(state.rollbackTransferredOwnership())
        assertTrue(state.acceptsPreparationChanges())
        assertTrue(state.transferOwnership())
    }

    @Test
    fun `connection fields remain attached to the draft state`() {
        val state =
            TransferDraftPreparationState(
                initialRole = "receive",
                showQrInitially = true,
                onDiscard = {},
            )
        state.typedCode.value = "1234-river-stone"
        state.invitationInput.value = "envoix://invite/v2/incoming"
        state.generatedInvite.value =
            CreatedInvite(
                roomCode = "abcd12-ef34-5678",
                payload = "envoix://invite/v2/fixture",
                reference = "invite:fixture",
                broker = "broker",
                relay = null,
                creatorRole = "receive",
                joinerRole = "send",
                expiresAt = 1L,
            )
        state.generatedInviteRole.value = "receive"

        assertEquals("receive", state.role.value)
        assertEquals("show", state.topMode.value)
        assertEquals("1234-river-stone", state.typedCode.value)
        assertEquals("envoix://invite/v2/incoming", state.invitationInput.value)
        assertEquals("abcd12-ef34-5678", state.generatedInvite.value?.roomCode)
    }
}
