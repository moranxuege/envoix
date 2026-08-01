package dev.envoix.app.discovery

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class IdentityReadAttemptLimiterTest {
    @Test
    fun `backs off the same peer without blocking a different peer`() {
        val limiter =
            IdentityReadAttemptLimiter(
                maxAttempts = 16,
                windowMs = 30_000,
                peerBackoffMs = 5_000,
            )

        assertTrue(limiter.tryAcquire("peer-a", nowMs = 0))
        assertFalse(limiter.tryAcquire("peer-a", nowMs = 4_999))
        assertTrue(limiter.tryAcquire("peer-b", nowMs = 4_999))
        assertTrue(limiter.tryAcquire("peer-a", nowMs = 5_000))
    }

    @Test
    fun `limits actual starts in a rolling window and retries after expiry`() {
        val limiter =
            IdentityReadAttemptLimiter(
                maxAttempts = 16,
                windowMs = 30_000,
                peerBackoffMs = 5_000,
            )

        repeat(16) { index ->
            assertTrue(limiter.tryAcquire("peer-$index", nowMs = index.toLong()))
        }
        assertFalse(limiter.tryAcquire("overflow", nowMs = 29_999))
        assertTrue(limiter.tryAcquire("retry", nowMs = 30_000))
        assertFalse(limiter.tryAcquire("still-full", nowMs = 30_000))
        assertTrue(limiter.tryAcquire("next-slot", nowMs = 30_001))
    }

    @Test
    fun `old failed peer can retry across many bounded windows`() {
        val limiter =
            IdentityReadAttemptLimiter(
                maxAttempts = 1,
                windowMs = 30_000,
                peerBackoffMs = 5_000,
            )

        repeat(1_000) { window ->
            assertTrue(limiter.tryAcquire("failed-peer", nowMs = window * 30_000L))
        }
    }
}
