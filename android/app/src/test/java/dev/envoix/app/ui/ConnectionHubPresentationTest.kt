package dev.envoix.app.ui

import dev.envoix.app.discovery.ProviderAvailability
import org.junit.Assert.assertEquals
import org.junit.Test

class ConnectionHubPresentationTest {
    @Test
    fun inactiveDiscoveryIsPaused() {
        assertEquals(
            NearbyEmptyState.Paused,
            nearbyEmptyState(
                active = false,
                availabilities = listOf(ProviderAvailability.Stopped),
            ),
        )
    }

    @Test
    fun activeDiscoveryWithNoAvailableProviderIsUnavailable() {
        assertEquals(
            NearbyEmptyState.Unavailable,
            nearbyEmptyState(
                active = true,
                availabilities =
                    listOf(
                        ProviderAvailability.PermissionRequired,
                        ProviderAvailability.Unsupported,
                    ),
            ),
        )
    }

    @Test
    fun activeDiscoveryWithAStartingProviderIsLooking() {
        assertEquals(
            NearbyEmptyState.Looking,
            nearbyEmptyState(
                active = true,
                availabilities =
                    listOf(
                        ProviderAvailability.Starting,
                        ProviderAvailability.Unsupported,
                    ),
            ),
        )
    }
}
