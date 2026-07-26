package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NearbyRendezvousOfferQueueTest {
    @Test
    fun `deduplicates repeated invitations without extending their lifetime`() {
        val queue = NearbyRendezvousOfferQueue(maxSize = 4, ttlMs = 100)
        val first = offer("request-1", "peer-a", "invite-a")
        val duplicate = offer("request-2", "peer-a", "invite-a")

        assertTrue(queue.add(first, nowMs = 0))
        assertFalse(queue.add(duplicate, nowMs = 90))
        assertEquals(listOf(first), queue.snapshot(nowMs = 99))
        assertTrue(queue.snapshot(nowMs = 100).isEmpty())
    }

    @Test
    fun `keeps a bounded fifo inbox and supports explicit removal`() {
        val queue = NearbyRendezvousOfferQueue(maxSize = 2, ttlMs = 1_000)
        val first = offer("request-1", "peer-a", "invite-a")
        val second = offer("request-2", "peer-b", "invite-b")
        val third = offer("request-3", "peer-c", "invite-c")

        queue.add(first, nowMs = 0)
        queue.add(second, nowMs = 1)
        queue.add(third, nowMs = 2)

        assertEquals(listOf(second, third), queue.snapshot(nowMs = 2))
        assertFalse(queue.remove(first.requestId))
        assertTrue(queue.remove(second.requestId))
        assertEquals(listOf(third), queue.snapshot(nowMs = 2))

        queue.add(second, nowMs = 3)
        queue.retainSender(third.senderPeerKey)
        assertEquals(listOf(third), queue.snapshot(nowMs = 3))
    }

    private fun offer(
        requestId: String,
        peerKey: String,
        invite: String,
    ) = NearbyRendezvousOffer(
        requestId = requestId,
        senderPeerKey = peerKey,
        senderDisplayName = null,
        invite = invite,
    )
}
