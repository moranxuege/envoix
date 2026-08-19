package dev.envoix.app.ui

import android.app.Application
import android.os.Build
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.AndroidWifiAwareCapabilityProbe
import dev.envoix.app.AndroidWifiAwareDiagnosticController
import dev.envoix.app.WifiAwareAvailability
import dev.envoix.app.WifiAwareCapabilitySnapshot
import dev.envoix.app.WifiAwareProbeRole
import dev.envoix.app.WifiAwareProbeSnapshot
import dev.envoix.app.isRunning
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

internal data class SettingsDiagnosticsUiState(
    val capability: WifiAwareCapabilitySnapshot? = null,
    val probe: WifiAwareProbeSnapshot = WifiAwareProbeSnapshot(),
    val nearbyPermissionRequestSupported: Boolean = false,
) {
    val canRequestNearbyPermission: Boolean
        get() =
            nearbyPermissionRequestSupported &&
                capability?.availability == WifiAwareAvailability.PERMISSION_REQUIRED

    val probeRunning: Boolean
        get() = probe.phase.isRunning

    val canStartProbe: Boolean
        get() =
            !probeRunning &&
                capability?.availability in
                setOf(
                    WifiAwareAvailability.READY,
                    WifiAwareAvailability.PAIRING_REQUIRED,
                )
}

/** Owns the platform Wi-Fi Aware diagnostic lifetime for the Settings feature. */
internal class SettingsDiagnosticsViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private val capability = MutableStateFlow<WifiAwareCapabilitySnapshot?>(null)
    private val controller = AndroidWifiAwareDiagnosticController(application)
    private var enabled = false
    private var refreshGeneration = 0L

    val uiState: StateFlow<SettingsDiagnosticsUiState> =
        combine(capability, controller.snapshot) { currentCapability, probe ->
            SettingsDiagnosticsUiState(
                capability = currentCapability,
                probe = probe,
                nearbyPermissionRequestSupported =
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU,
            )
        }.stateIn(
            scope = viewModelScope,
            started = SharingStarted.Eagerly,
            initialValue = SettingsDiagnosticsUiState(),
        )

    fun setEnabled(value: Boolean) {
        if (enabled == value) return
        enabled = value
        refreshGeneration += 1
        if (value) {
            refresh()
        } else {
            capability.value = null
            controller.stop()
        }
    }

    fun refresh() {
        if (!enabled) return
        val generation = ++refreshGeneration
        viewModelScope.launch {
            val snapshot = AndroidWifiAwareCapabilityProbe.read(getApplication())
            if (enabled && generation == refreshGeneration) {
                capability.value = snapshot
            }
        }
        controller.refresh()
    }

    fun startProbe(role: WifiAwareProbeRole) {
        if (enabled && uiState.value.canStartProbe) {
            controller.start(role)
        }
    }

    fun stopProbe() {
        controller.stop()
    }

    override fun onCleared() {
        controller.close()
        super.onCleared()
    }
}
