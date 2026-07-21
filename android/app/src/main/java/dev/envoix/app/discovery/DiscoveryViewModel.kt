package dev.envoix.app.discovery

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.OpLog
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

internal data class DiscoveryUiState(
    val localName: String,
    val active: Boolean = false,
    val nowMs: Long = 0,
    val peers: List<DiscoveredPeer> = emptyList(),
    val statuses: Map<DiscoverySource, ProviderStatus> = emptyMap(),
    val incomingRendezvousOffer: NearbyRendezvousOffer? = null,
)

internal class DiscoveryViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private var identity = DiscoveryIdentityFactory.create()
    private val registry = DiscoveryPeerRegistry()
    private var providers: List<DiscoveryProvider> = emptyList()
    private val lastLoggedAvailability = mutableMapOf<DiscoverySource, ProviderAvailability>()

    private val _uiState =
        MutableStateFlow(
            DiscoveryUiState(
                localName = identity.displayName,
                nowMs = SystemClock.elapsedRealtime(),
                statuses = stoppedStatuses(),
            ),
        )
    val uiState: StateFlow<DiscoveryUiState> = _uiState.asStateFlow()

    private var started = false
    private var hasStarted = false
    private var generation = 0
    private var ticker: Job? = null

    fun start() {
        if (started) return
        started = true
        if (hasStarted) {
            identity = DiscoveryIdentityFactory.create()
        } else {
            hasStarted = true
        }
        generation += 1
        val activeGeneration = generation
        registry.clear()
        _uiState.value =
            _uiState.value.copy(
                localName = identity.displayName,
                active = true,
                peers = emptyList(),
                incomingRendezvousOffer = null,
            )
        providers = createProviders()
        val listener = listener(activeGeneration)
        providers.forEach { provider -> provider.start(listener) }
        ticker =
            viewModelScope.launch {
                while (isActive) {
                    publishPeers()
                    delay(PEER_REFRESH_MS)
                }
            }
    }

    fun stop() {
        if (!started) return
        started = false
        generation += 1
        ticker?.cancel()
        ticker = null
        providers.forEach(DiscoveryProvider::stop)
        providers = emptyList()
        registry.clear()
        _uiState.value =
            _uiState.value.copy(
                active = false,
                peers = emptyList(),
                statuses = stoppedStatuses(),
                incomingRendezvousOffer = null,
            )
    }

    fun restart() {
        stop()
        start()
    }

    fun offerInvite(
        peerKey: String,
        invite: String,
        completion: (error: String?) -> Unit,
    ) {
        val provider = providers.filterIsInstance<NearbyRendezvousProvider>().firstOrNull()
        if (!started || provider == null) {
            completion("Experimental Bluetooth pairing is not available")
            return
        }
        val activeGeneration = generation
        provider.offerInvite(peerKey, invite) { error ->
            viewModelScope.launch {
                if (!started || generation != activeGeneration) {
                    completion("Bluetooth discovery stopped")
                } else {
                    completion(error)
                }
            }
        }
    }

    fun consumeRendezvousOffer(requestId: String) {
        if (_uiState.value.incomingRendezvousOffer?.requestId == requestId) {
            _uiState.value = _uiState.value.copy(incomingRendezvousOffer = null)
        }
    }

    override fun onCleared() {
        stop()
    }

    private fun publishPeers() {
        val now = SystemClock.elapsedRealtime()
        _uiState.value =
            _uiState.value.copy(
                nowMs = now,
                peers = registry.peers(now),
            )
    }

    private fun listener(activeGeneration: Int): DiscoveryListener =
        object : DiscoveryListener {
            override fun onObservation(observation: DiscoveryObservation) {
                viewModelScope.launch {
                    if (!started || generation != activeGeneration || observation.peerKey == identity.peerKey) {
                        return@launch
                    }
                    if (registry.upsert(observation)) publishPeers()
                }
            }

            override fun onStatus(status: ProviderStatus) {
                viewModelScope.launch {
                    if (!started || generation != activeGeneration) return@launch
                    val statuses = _uiState.value.statuses + (status.source to status)
                    _uiState.value = _uiState.value.copy(statuses = statuses)
                    if (lastLoggedAvailability[status.source] != status.availability) {
                        lastLoggedAvailability[status.source] = status.availability
                        OpLog.add(
                            "DISCOVERY provider=${status.source.logName()} state=${status.availability.logName()}",
                        )
                    }
                }
            }

            override fun onRendezvousOffer(offer: NearbyRendezvousOffer) {
                viewModelScope.launch {
                    if (!started || generation != activeGeneration || offer.senderPeerKey == identity.peerKey) {
                        return@launch
                    }
                    _uiState.value = _uiState.value.copy(incomingRendezvousOffer = offer)
                }
            }
        }

    private fun createProviders(): List<DiscoveryProvider> =
        listOf(
            BluetoothDiscoveryProvider(getApplication(), identity),
            MdnsDiscoveryProvider(getApplication(), identity),
            WifiAwareDiscoveryProvider(),
        )

    private fun stoppedStatuses(): Map<DiscoverySource, ProviderStatus> =
        DiscoverySource.entries.associateWith { source ->
            ProviderStatus(source, ProviderAvailability.Stopped, "Discovery is stopped")
        }

    private fun Enum<*>.logName(): String = name.replace(UPPERCASE_BOUNDARY, "_").lowercase()

    companion object {
        private const val PEER_REFRESH_MS = 1_000L
        private val UPPERCASE_BOUNDARY = Regex("(?<=[a-z])(?=[A-Z])")
    }
}
