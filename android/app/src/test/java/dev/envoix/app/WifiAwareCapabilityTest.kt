package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

class WifiAwareCapabilityTest {
    @Test
    fun availabilityPolicyCoversEveryStructuredState() {
        val cases =
            listOf(
                readyFacts(apiLevel = 33) to WifiAwareAvailability.UNSUPPORTED_OS,
                readyFacts(featurePresent = false) to WifiAwareAvailability.UNSUPPORTED_HARDWARE,
                readyFacts(pairingSupported = false) to WifiAwareAvailability.UNSUPPORTED_HARDWARE,
                readyFacts(entitlementPresent = false) to WifiAwareAvailability.ENTITLEMENT_MISSING,
                readyFacts(permissionState = WifiAwarePermissionState.REQUIRED) to WifiAwareAvailability.PERMISSION_REQUIRED,
                readyFacts(permissionState = WifiAwarePermissionState.DENIED) to WifiAwareAvailability.PERMISSION_DENIED,
                readyFacts(wifiEnabled = false) to WifiAwareAvailability.WIFI_DISABLED,
                readyFacts(
                    wifiEnabled = false,
                    temporarilyAvailable = false,
                    pairingSupported = null,
                ) to WifiAwareAvailability.WIFI_DISABLED,
                readyFacts(servicePresent = false) to WifiAwareAvailability.TEMPORARILY_UNAVAILABLE,
                readyFacts(temporarilyAvailable = false) to WifiAwareAvailability.TEMPORARILY_UNAVAILABLE,
                readyFacts(pairingSupported = null) to WifiAwareAvailability.TEMPORARILY_UNAVAILABLE,
                readyFacts(pairedDeviceCount = null) to WifiAwareAvailability.TEMPORARILY_UNAVAILABLE,
                readyFacts(pairedDeviceCount = 0) to WifiAwareAvailability.PAIRING_REQUIRED,
                readyFacts() to WifiAwareAvailability.READY,
            )

        cases.forEach { (facts, expected) ->
            assertEquals(expected, WifiAwareCapabilityPolicy.evaluate(facts).availability)
        }
    }

    @Test
    fun wireNamesAndServiceIdentifierAreStable() {
        assertEquals(
            listOf(
                "unsupported_os",
                "unsupported_hardware",
                "entitlement_missing",
                "permission_required",
                "permission_denied",
                "wifi_disabled",
                "temporarily_unavailable",
                "pairing_required",
                "ready",
            ),
            WifiAwareAvailability.entries.map { it.wireName },
        )
        assertEquals("_envoix._udp", ENVOIX_WIFI_AWARE_SERVICE)
    }

    private fun readyFacts(
        apiLevel: Int = WIFI_AWARE_PAIRING_MIN_API,
        featurePresent: Boolean = true,
        entitlementPresent: Boolean = true,
        permissionState: WifiAwarePermissionState = WifiAwarePermissionState.GRANTED,
        wifiEnabled: Boolean = true,
        servicePresent: Boolean = true,
        temporarilyAvailable: Boolean = true,
        pairingSupported: Boolean? = true,
        pairedDeviceCount: Int? = 1,
    ) = WifiAwareCapabilityFacts(
        apiLevel = apiLevel,
        featurePresent = featurePresent,
        entitlementPresent = entitlementPresent,
        permissionState = permissionState,
        wifiEnabled = wifiEnabled,
        servicePresent = servicePresent,
        temporarilyAvailable = temporarilyAvailable,
        pairingSupported = pairingSupported,
        pairedDeviceCount = pairedDeviceCount,
    )
}
