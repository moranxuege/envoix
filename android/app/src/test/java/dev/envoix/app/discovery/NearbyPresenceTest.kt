package dev.envoix.app.discovery

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NearbyPresenceTest {
    @Test
    fun `temporary visibility ends exactly at its deadline`() {
        assertTrue(
            nearbyAdvertisingAllowed(
                NearbyVisibility.EveryoneTenMinutes,
                expiresAtEpochMs = 1_000L,
                nowEpochMs = 999L,
            ),
        )
        assertFalse(
            nearbyAdvertisingAllowed(
                NearbyVisibility.EveryoneTenMinutes,
                expiresAtEpochMs = 1_000L,
                nowEpochMs = 1_000L,
            ),
        )
    }

    @Test
    fun `hidden never advertises and foreground visibility does`() {
        assertFalse(nearbyAdvertisingAllowed(NearbyVisibility.Hidden, 0L, 0L))
        assertTrue(nearbyAdvertisingAllowed(NearbyVisibility.Foreground, 0L, Long.MAX_VALUE))
    }
}
