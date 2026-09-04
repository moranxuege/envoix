package dev.envoix.app.ui

import dev.envoix.app.WifiAwareAvailability
import dev.envoix.app.WifiAwareCapabilitySnapshot
import dev.envoix.app.WifiAwareProbePhase
import dev.envoix.app.WifiAwareProbeSnapshot
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsDiagnosticsUiStateTest {
    @Test
    fun `permission action requires platform support and a missing permission`() {
        val permissionRequired = state(WifiAwareAvailability.PERMISSION_REQUIRED)

        assertTrue(
            permissionRequired
                .copy(nearbyPermissionRequestSupported = true)
                .canRequestNearbyPermission,
        )
        assertFalse(permissionRequired.canRequestNearbyPermission)
        assertFalse(
            state(WifiAwareAvailability.READY)
                .copy(nearbyPermissionRequestSupported = true)
                .canRequestNearbyPermission,
        )
    }

    @Test
    fun `probe starts only for usable idle capability`() {
        assertTrue(state(WifiAwareAvailability.READY).canStartProbe)
        assertTrue(state(WifiAwareAvailability.PAIRING_REQUIRED).canStartProbe)
        assertFalse(state(WifiAwareAvailability.TEMPORARILY_UNAVAILABLE).canStartProbe)
        assertFalse(
            state(WifiAwareAvailability.READY)
                .copy(probe = WifiAwareProbeSnapshot(phase = WifiAwareProbePhase.CONNECTING))
                .canStartProbe,
        )
    }

    private fun state(availability: WifiAwareAvailability) =
        SettingsDiagnosticsUiState(
            capability =
                WifiAwareCapabilitySnapshot(
                    availability = availability,
                    pairingSupported = null,
                    pairedDeviceCount = null,
                ),
        )
}
