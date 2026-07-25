package dev.envoix.app.ui

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
    fun `connection fields remain attached to the draft state`() {
        val state =
            TransferDraftPreparationState(
                initialRole = "receive",
                showQrInitially = true,
                onDiscard = {},
            )
        state.typedCode.value = "1234-river-stone"
        state.generatedInvite.value = "4321-cloud-field" to "envoix://pair/fixture"
        state.generatedInviteRole.value = "receive"

        assertEquals("receive", state.role.value)
        assertEquals("show", state.topMode.value)
        assertEquals("1234-river-stone", state.typedCode.value)
        assertEquals("4321-cloud-field", state.generatedInvite.value?.first)
    }
}
