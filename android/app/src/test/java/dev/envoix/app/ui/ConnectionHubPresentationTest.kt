package dev.envoix.app.ui

import androidx.compose.ui.unit.dp
import dev.envoix.app.WifiAwareAvailability
import dev.envoix.app.WifiAwareCapabilitySnapshot
import dev.envoix.app.discovery.DiscoverySource
import dev.envoix.app.discovery.ProviderAvailability
import dev.envoix.app.discovery.ProviderStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectionHubPresentationTest {
    @Test
    fun roomActionsFillTheSameSquareAsTheRevealedQr() {
        assertEquals(
            MainRoomInviteQrLayout(
                side = 240.dp,
                viewportHeight = 240.dp,
                showsActions = true,
            ),
            resolveMainRoomInviteQrLayout(maxWidth = 320.dp, revealed = false),
        )
    }

    @Test
    fun revealedRoomQrUsesTheViewportAndHidesConflictingActions() {
        assertEquals(
            MainRoomInviteQrLayout(
                side = 240.dp,
                viewportHeight = 240.dp,
                showsActions = false,
            ),
            resolveMainRoomInviteQrLayout(maxWidth = 320.dp, revealed = true),
        )
    }

    @Test
    fun roomActionsAndQrAdaptToNarrowWidthTogether() {
        assertEquals(
            MainRoomInviteQrLayout(
                side = 220.dp,
                viewportHeight = 240.dp,
                showsActions = true,
            ),
            resolveMainRoomInviteQrLayout(maxWidth = 220.dp, revealed = false),
        )
        assertEquals(
            MainRoomInviteQrLayout(
                side = 220.dp,
                viewportHeight = 240.dp,
                showsActions = false,
            ),
            resolveMainRoomInviteQrLayout(maxWidth = 220.dp, revealed = true),
        )
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
    fun wifiAwarePresentationRequiresBothCapabilityAndAReadyProvider() {
        assertEquals(
            WifiAwareFeatureUiState.Checking,
            wifiAwareFeatureUiState(null, status(ProviderAvailability.Ready)),
        )
        assertEquals(
            WifiAwareFeatureUiState.Active,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.READY),
                status(ProviderAvailability.Ready),
            ),
        )
        assertEquals(
            WifiAwareFeatureUiState.Starting,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.READY),
                status(ProviderAvailability.Starting),
            ),
        )
        assertEquals(
            WifiAwareFeatureUiState.ExperimentalUnavailable,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.READY),
                status(ProviderAvailability.Reserved),
            ),
        )
    }

    @Test
    fun wifiAwarePresentationExplainsUnsupportedAndActionableCapabilityStates() {
        assertEquals(
            WifiAwareFeatureUiState.Unsupported,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.UNSUPPORTED_HARDWARE),
                status(ProviderAvailability.Reserved),
            ),
        )
        assertEquals(
            WifiAwareFeatureUiState.PermissionRequired,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.PERMISSION_REQUIRED),
                status(ProviderAvailability.Reserved),
            ),
        )
        assertEquals(
            WifiAwareFeatureUiState.WifiDisabled,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.WIFI_DISABLED),
                status(ProviderAvailability.Reserved),
            ),
        )
        assertEquals(
            WifiAwareFeatureUiState.PairingRequired,
            wifiAwareFeatureUiState(
                capability(WifiAwareAvailability.PAIRING_REQUIRED),
                status(ProviderAvailability.Reserved),
            ),
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

    @Test
    fun openingNfcPanelStartsTheAndroidPresenterForAnIphone() {
        assertTrue(
            shouldStartNfcPresentationWhenPanelOpens(
                phase = RoomControlPhase.None,
                hostingArmed = false,
                readerScanning = false,
            ),
        )
        assertTrue(
            shouldStartNfcPresentationWhenPanelOpens(
                phase = RoomControlPhase.Hosting,
                hostingArmed = false,
                readerScanning = false,
            ),
        )
        assertFalse(
            shouldStartNfcPresentationWhenPanelOpens(
                phase = RoomControlPhase.Hosting,
                hostingArmed = true,
                readerScanning = false,
            ),
        )
        assertFalse(
            shouldStartNfcPresentationWhenPanelOpens(
                phase = RoomControlPhase.None,
                hostingArmed = false,
                readerScanning = true,
            ),
        )
        assertFalse(
            shouldStartNfcPresentationWhenPanelOpens(
                phase = RoomControlPhase.Connected,
                hostingArmed = false,
                readerScanning = false,
            ),
        )
    }

    private fun status(availability: ProviderAvailability) =
        ProviderStatus(
            source = DiscoverySource.WifiAware,
            availability = availability,
            detail = "test-only",
        )

    private fun capability(availability: WifiAwareAvailability) =
        WifiAwareCapabilitySnapshot(
            availability = availability,
            pairingSupported = availability != WifiAwareAvailability.UNSUPPORTED_HARDWARE,
            pairedDeviceCount = if (availability == WifiAwareAvailability.READY) 1 else 0,
        )
}
