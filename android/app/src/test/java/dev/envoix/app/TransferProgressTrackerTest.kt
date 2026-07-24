package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferProgressTrackerTest {
    @Test
    fun `progress is monotonic and rate limited`() {
        val tracker = TransferProgressTracker()

        assertNotNull(tracker.update(bytes = 100, total = 1_000, nowNanos = 0))
        assertNull(tracker.update(bytes = 90, total = 1_000, nowNanos = 50_000_000))
        val sample = tracker.update(bytes = 300, total = 1_000, nowNanos = 200_000_000)

        assertNotNull(sample)
        assertEquals(300L, sample?.bytes)
        assertEquals(1_000L, sample?.total)
        assertTrue(checkNotNull(sample).speedBps > 0)
    }

    @Test
    fun `completion flushes without waiting for the publish interval`() {
        val tracker = TransferProgressTracker(initialBytes = 400)
        tracker.update(bytes = 400, total = 1_000, nowNanos = 0)

        val completed = tracker.update(bytes = 1_000, total = 1_000, nowNanos = 50_000_000)

        assertNotNull(completed)
        assertEquals(1_000L, completed?.bytes)
        assertEquals(1_000L, completed?.total)
    }
}
