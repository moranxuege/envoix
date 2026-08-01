package dev.envoix.app.ui

import androidx.compose.ui.unit.dp
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.ProviderAvailability
import dev.envoix.app.discovery.ProviderStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionHubPresentationTest {
    @Test
    fun hiddenRoomQrRetainsCanonicalSize() {
        assertEquals(
            MainRoomInviteQrLayout(side = 148.dp, stackActions = false),
            resolveMainRoomInviteQrLayout(maxWidth = 320.dp, revealed = false),
        )
    }

    @Test
    fun revealedRoomQrUsesLargerSizeWhenSpaceAllows() {
        assertEquals(
            MainRoomInviteQrLayout(side = 168.dp, stackActions = false),
            resolveMainRoomInviteQrLayout(maxWidth = 320.dp, revealed = true),
        )
    }

    @Test
    fun roomQrStacksActionsWhenTheLargerRevealWouldCrowdThem() {
        assertEquals(
            MainRoomInviteQrLayout(side = 148.dp, stackActions = false),
            resolveMainRoomInviteQrLayout(maxWidth = 280.dp, revealed = false),
        )
        assertEquals(
            MainRoomInviteQrLayout(side = 168.dp, stackActions = true),
            resolveMainRoomInviteQrLayout(maxWidth = 280.dp, revealed = true),
        )
    }

    @Test
    fun roomQrShrinksOnlyWhenTheContainerIsNarrowerThanThePlaceholder() {
        assertEquals(
            MainRoomInviteQrLayout(side = 140.dp, stackActions = true),
            resolveMainRoomInviteQrLayout(maxWidth = 140.dp, revealed = false),
        )
        assertEquals(
            MainRoomInviteQrLayout(side = 140.dp, stackActions = true),
            resolveMainRoomInviteQrLayout(maxWidth = 140.dp, revealed = true),
        )
    }

    @Test
    fun roomCodeActionsStackWhenWidthOrFontScaleWouldCrowdTheCode() {
        assertFalse(shouldStackRoomCodeActions(maxWidth = 280.dp, fontScale = 1f))
        assertTrue(shouldStackRoomCodeActions(maxWidth = 240.dp, fontScale = 1f))
        assertTrue(shouldStackRoomCodeActions(maxWidth = 320.dp, fontScale = 1.5f))
    }

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

    @Test
    fun wifiAwareOnlyAppearsActiveWhenTheProviderIsActuallyReady() {
        assertEquals(
            WifiAwareDiscoveryUiState.Active,
            wifiAwareDiscoveryUiState(status(ProviderAvailability.Ready)),
        )
        assertEquals(
            WifiAwareDiscoveryUiState.Starting,
            wifiAwareDiscoveryUiState(status(ProviderAvailability.Starting)),
        )
        assertEquals(
            WifiAwareDiscoveryUiState.Unavailable,
            wifiAwareDiscoveryUiState(status(ProviderAvailability.Reserved)),
        )
        assertEquals(
            WifiAwareDiscoveryUiState.Unavailable,
            wifiAwareDiscoveryUiState(null),
        )
    }

    @Test
    fun nfcSharingOnlyStartsFromAReplaceableRoomState() {
        assertTrue(canShareRoomViaNfc(RoomControlPhase.None))
        assertTrue(canShareRoomViaNfc(RoomControlPhase.Hosting))
        assertTrue(canShareRoomViaNfc(RoomControlPhase.Closed))
        assertTrue(canShareRoomViaNfc(RoomControlPhase.Failed))
        assertFalse(canShareRoomViaNfc(RoomControlPhase.Joining))
        assertFalse(canShareRoomViaNfc(RoomControlPhase.Connected))
        assertFalse(canShareRoomViaNfc(RoomControlPhase.Legacy))
    }

    private fun status(availability: ProviderAvailability) =
        ProviderStatus(
            source = DiscoverySource.WifiAware,
            availability = availability,
            detail = "test-only",
        )
}
