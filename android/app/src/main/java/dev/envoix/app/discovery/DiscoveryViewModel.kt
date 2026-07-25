package dev.envoix.app.discovery

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.InviteCodec
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
    val mode: DiscoveryMode = DiscoveryMode.Off,
    val nowMs: Long = 0,
    val peers: List<DiscoveredPeer> = emptyList(),
    val statuses: Map<DiscoverySource, ProviderStatus> = emptyMap(),
    val incomingRendezvousOffers: List<NearbyRendezvousOffer> = emptyList(),
)

internal enum class DiscoveryMode {
    Off,
    BrowseNearby,
    SelectedPeer,
}

internal class DiscoveryViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private val identity = DiscoveryIdentityFactory.create()
    private val registry = DiscoveryPeerRegistry()
    private val offerQueue = NearbyRendezvousOfferQueue()
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

    private var foreground = false
    private var requestedMode = DiscoveryMode.Off
    private var selectedPeerKey: String? = null
    private var started = false
    private var generation = 0
    private var ticker: Job? = null

    fun setForeground(value: Boolean) {
        if (foreground == value) return
        foreground = value
        reconcile()
    }

    fun setMode(
        mode: DiscoveryMode,
        peerKey: String? = null,
    ) {
        require(mode != DiscoveryMode.SelectedPeer || !peerKey.isNullOrBlank()) {
            "Selected-peer discovery requires a peer key"
        }
        if (requestedMode == mode && selectedPeerKey == peerKey) return
        requestedMode = mode
        selectedPeerKey = peerKey.takeIf { mode == DiscoveryMode.SelectedPeer }
        _uiState.value = _uiState.value.copy(mode = mode)
        if (mode == DiscoveryMode.Off) {
            offerQueue.clear()
        } else if (mode == DiscoveryMode.SelectedPeer) {
            offerQueue.retainSender(requireNotNull(peerKey))
        }
        reconcile()
        publishPeers()
    }

    /** Compatibility for the retired standalone discovery screen. */
    fun start() {
        setMode(DiscoveryMode.BrowseNearby)
        setForeground(true)
    }

    /** Compatibility for the retired standalone discovery screen. */
    fun stop() {
        setForeground(false)
        setMode(DiscoveryMode.Off)
    }

    private fun reconcile() {
        val shouldRun = foreground && requestedMode != DiscoveryMode.Off
        if (shouldRun) {
            startProviders()
        } else {
            stopProviders()
        }
    }

    private fun startProviders() {
        if (started) return
        started = true
        generation += 1
        val activeGeneration = generation
        registry.clear()
        _uiState.value =
            _uiState.value.copy(
                localName = identity.displayName,
                active = true,
                peers = emptyList(),
                mode = requestedMode,
                incomingRendezvousOffers = offerQueue.snapshot(SystemClock.elapsedRealtime()),
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

    private fun stopProviders() {
        if (!started) return
        started = false
        generation += 1
        ticker?.cancel()
        ticker = null
        providers.forEach(DiscoveryProvider::stop)
        providers = emptyList()
        registry.clear()
        offerQueue.clear()
        _uiState.value =
            _uiState.value.copy(
                active = false,
                peers = emptyList(),
                statuses = stoppedStatuses(),
                incomingRendezvousOffers = emptyList(),
            )
    }

    fun restart() {
        stopProviders()
        reconcile()
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
        if (offerQueue.remove(requestId)) {
            _uiState.value =
                _uiState.value.copy(
                    incomingRendezvousOffers = offerQueue.snapshot(SystemClock.elapsedRealtime()),
                )
        }
    }

    override fun onCleared() {
        stop()
    }

    private fun publishPeers() {
        val now = SystemClock.elapsedRealtime()
        val peers =
            registry.peers(now).let { discovered ->
                if (requestedMode == DiscoveryMode.SelectedPeer) {
                    discovered.filter { it.peerKey == selectedPeerKey }
                } else {
                    discovered
                }
            }
        _uiState.value =
            _uiState.value.copy(
                nowMs = now,
                peers = peers,
                incomingRendezvousOffers = offerQueue.snapshot(now),
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
                    if (InviteCodec.parse(offer.invite) == null) {
                        OpLog.add("DISCOVERY provider=bluetooth state=invalid_offer")
                        return@launch
                    }
                    if (requestedMode == DiscoveryMode.SelectedPeer && offer.senderPeerKey != selectedPeerKey) {
                        return@launch
                    }
                    if (offerQueue.add(offer, SystemClock.elapsedRealtime())) publishPeers()
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
