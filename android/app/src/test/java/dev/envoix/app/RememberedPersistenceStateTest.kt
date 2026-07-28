package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RememberedPersistenceStateTest {
    @Test
    fun `create then retry rotates the existing relationship`() {
        val pending =
            PendingRememberedPeer(
                relationshipId = "relationship",
                credentialReference = "credential",
                label = "peer",
                broker = "broker",
                relay = "relay",
            )
        val actions = mutableListOf<String>()
        val initial = RememberedPersistenceState(pending, pending.relationshipId)

        assertTrue(
            initial.persist(
                create = {
                    actions += "create:${it.relationshipId}"
                    true
                },
                rotate = {
                    actions += "rotate:$it"
                    true
                },
            ),
        )
        assertTrue(
            initial.persist(
                create = { error("created relationship must not be created again") },
                rotate = {
                    actions += "rotate:$it"
                    true
                },
            ),
        )

        val resumed = RememberedPersistenceState(null, pending.relationshipId)
        assertTrue(
            resumed.persist(
                create = { error("resumed relationship must not be created again") },
                rotate = {
                    actions += "resume:$it"
                    true
                },
            ),
        )
        assertEquals(
            listOf(
                "create:relationship",
                "rotate:relationship",
                "resume:relationship",
            ),
            actions,
        )
    }
}
