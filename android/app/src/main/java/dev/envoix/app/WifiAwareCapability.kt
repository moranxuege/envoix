package dev.envoix.app

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.WifiAwareManager
import android.net.wifi.aware.WifiAwareSession
import android.os.Build
import androidx.annotation.RequiresApi
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

internal const val ENVOIX_WIFI_AWARE_SERVICE = "_envoix._udp"
internal const val WIFI_AWARE_PAIRING_MIN_API = 34
private const val AWARE_ATTACH_TIMEOUT_MS = 5_000L
private const val PAIRED_DEVICE_PROBE_TIMEOUT_MS = 2_000L

internal enum class WifiAwareAvailability(
    val wireName: String,
) {
    UNSUPPORTED_OS("unsupported_os"),
    UNSUPPORTED_HARDWARE("unsupported_hardware"),
    ENTITLEMENT_MISSING("entitlement_missing"),
    PERMISSION_REQUIRED("permission_required"),
    PERMISSION_DENIED("permission_denied"),
    WIFI_DISABLED("wifi_disabled"),
    TEMPORARILY_UNAVAILABLE("temporarily_unavailable"),
    PAIRING_REQUIRED("pairing_required"),
    READY("ready"),
}

internal enum class WifiAwarePermissionState {
    GRANTED,
    REQUIRED,
    DENIED,
}

internal data class WifiAwareCapabilityFacts(
    val apiLevel: Int,
    val featurePresent: Boolean,
    val entitlementPresent: Boolean,
    val permissionState: WifiAwarePermissionState,
    val wifiEnabled: Boolean,
    val servicePresent: Boolean,
    val temporarilyAvailable: Boolean,
    val pairingSupported: Boolean?,
    val pairedDeviceCount: Int?,
)

internal data class WifiAwareCapabilitySnapshot(
    val availability: WifiAwareAvailability,
    val pairingSupported: Boolean?,
    val pairedDeviceCount: Int?,
) {
    val diagnosticSummary: String
        get() =
            buildString {
                append(availability.wireName)
                append(" · pairing=")
                append(pairingSupported?.toString() ?: "unknown")
                append(" · paired_devices=")
                append(pairedDeviceCount?.toString() ?: "unknown")
            }
}

internal object WifiAwareCapabilityPolicy {
    fun evaluate(facts: WifiAwareCapabilityFacts): WifiAwareCapabilitySnapshot {
        val availability =
            when {
                facts.apiLevel < WIFI_AWARE_PAIRING_MIN_API -> WifiAwareAvailability.UNSUPPORTED_OS
                !facts.featurePresent || facts.pairingSupported == false -> WifiAwareAvailability.UNSUPPORTED_HARDWARE
                !facts.entitlementPresent -> WifiAwareAvailability.ENTITLEMENT_MISSING
                facts.permissionState == WifiAwarePermissionState.REQUIRED -> WifiAwareAvailability.PERMISSION_REQUIRED
                facts.permissionState == WifiAwarePermissionState.DENIED -> WifiAwareAvailability.PERMISSION_DENIED
                !facts.wifiEnabled -> WifiAwareAvailability.WIFI_DISABLED
                !facts.servicePresent || !facts.temporarilyAvailable || facts.pairingSupported == null ->
                    WifiAwareAvailability.TEMPORARILY_UNAVAILABLE
                facts.pairedDeviceCount == null -> WifiAwareAvailability.TEMPORARILY_UNAVAILABLE
                facts.pairedDeviceCount == 0 -> WifiAwareAvailability.PAIRING_REQUIRED
                else -> WifiAwareAvailability.READY
            }

        return WifiAwareCapabilitySnapshot(
            availability = availability,
            pairingSupported = facts.pairingSupported,
            pairedDeviceCount = facts.pairedDeviceCount,
        )
    }
}

internal object AndroidWifiAwareCapabilityProbe {
    suspend fun read(context: Context): WifiAwareCapabilitySnapshot {
        val appContext = context.applicationContext
        val apiLevel = Build.VERSION.SDK_INT
        val featurePresent =
            appContext.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)
        val permissionState = permissionState(appContext, apiLevel)
        val wifiManager = appContext.getSystemService(WifiManager::class.java)
        val awareManager = appContext.getSystemService(WifiAwareManager::class.java)

        fun snapshot(
            temporarilyAvailable: Boolean = false,
            pairingSupported: Boolean? = null,
            pairedDeviceCount: Int? = null,
            effectivePermissionState: WifiAwarePermissionState = permissionState,
        ): WifiAwareCapabilitySnapshot =
            WifiAwareCapabilityPolicy.evaluate(
                WifiAwareCapabilityFacts(
                    apiLevel = apiLevel,
                    featurePresent = featurePresent,
                    entitlementPresent = true,
                    permissionState = effectivePermissionState,
                    wifiEnabled = wifiManager?.isWifiEnabled ?: true,
                    servicePresent = wifiManager != null && awareManager != null,
                    temporarilyAvailable = temporarilyAvailable,
                    pairingSupported = pairingSupported,
                    pairedDeviceCount = pairedDeviceCount,
                ),
            )

        if (
            Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE ||
            !featurePresent ||
            permissionState != WifiAwarePermissionState.GRANTED ||
            wifiManager == null ||
            awareManager == null ||
            !wifiManager.isWifiEnabled
        ) {
            return snapshot()
        }

        return try {
            if (!awareManager.isAvailable) return snapshot()
            val initialCharacteristics = awareManager.characteristics
            val probeSession =
                if (initialCharacteristics == null) {
                    withTimeoutOrNull(AWARE_ATTACH_TIMEOUT_MS) { attachForProbe(awareManager) }
                } else {
                    null
                }
            try {
                val pairingSupported =
                    (initialCharacteristics ?: awareManager.characteristics)?.isAwarePairingSupported
                val pairedDeviceCount =
                    if (pairingSupported == true) {
                        withTimeoutOrNull(PAIRED_DEVICE_PROBE_TIMEOUT_MS) {
                            pairedDeviceCount(appContext, awareManager)
                        }
                    } else {
                        null
                    }
                snapshot(
                    temporarilyAvailable = true,
                    pairingSupported = pairingSupported,
                    pairedDeviceCount = pairedDeviceCount,
                )
            } finally {
                probeSession?.close()
            }
        } catch (_: SecurityException) {
            snapshot(effectivePermissionState = WifiAwarePermissionState.DENIED)
        } catch (_: RuntimeException) {
            snapshot()
        }
    }

    private fun permissionState(
        context: Context,
        apiLevel: Int,
    ): WifiAwarePermissionState {
        if (apiLevel < Build.VERSION_CODES.TIRAMISU) return WifiAwarePermissionState.GRANTED
        return if (
            context.checkSelfPermission(Manifest.permission.NEARBY_WIFI_DEVICES) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            WifiAwarePermissionState.GRANTED
        } else {
            WifiAwarePermissionState.REQUIRED
        }
    }

    private suspend fun attachForProbe(manager: WifiAwareManager): WifiAwareSession? =
        suspendCancellableCoroutine { continuation ->
            manager.attach(
                object : AttachCallback() {
                    override fun onAttached(session: WifiAwareSession) {
                        if (!continuation.isActive) {
                            session.close()
                            return
                        }
                        continuation.invokeOnCancellation { session.close() }
                        continuation.resume(session)
                    }

                    override fun onAttachFailed() {
                        if (continuation.isActive) continuation.resume(null)
                    }
                },
                null,
            )
        }

    @RequiresApi(WIFI_AWARE_PAIRING_MIN_API)
    private suspend fun pairedDeviceCount(
        context: Context,
        manager: WifiAwareManager,
    ): Int =
        suspendCancellableCoroutine { continuation ->
            manager.getPairedDevices(context.mainExecutor) { aliases ->
                if (continuation.isActive) continuation.resume(aliases.size)
            }
        }
}
