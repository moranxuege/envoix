package dev.envoix.app

import android.Manifest
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.rule.GrantPermissionRule
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = WIFI_AWARE_PAIRING_MIN_API)
class WifiAwareCapabilityInstrumentedTest {
    @get:Rule
    val nearbyWifiPermission: GrantPermissionRule =
        GrantPermissionRule.grant(Manifest.permission.NEARBY_WIFI_DEVICES)

    @Test
    fun reportsPairingCapableWifiAwareHardware() =
        runBlocking {
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val snapshot = AndroidWifiAwareCapabilityProbe.read(context)
            val evidence =
                "availability=${snapshot.availability.wireName} " +
                    "pairing_supported=${snapshot.pairingSupported ?: "unknown"} " +
                    "paired_device_count=${snapshot.pairedDeviceCount ?: "unknown"}"
            Log.i(LOG_TAG, evidence)

            assertEquals("Wi-Fi Aware pairing is unavailable: $evidence", true, snapshot.pairingSupported)
            assertNotNull("Paired-device query did not complete: $evidence", snapshot.pairedDeviceCount)
            assertTrue(
                "Unexpected Wi-Fi Aware gate state: $evidence",
                snapshot.availability == WifiAwareAvailability.PAIRING_REQUIRED ||
                    snapshot.availability == WifiAwareAvailability.READY,
            )
        }

    private companion object {
        const val LOG_TAG = "EnvoixWifiAwareGate"
    }
}
