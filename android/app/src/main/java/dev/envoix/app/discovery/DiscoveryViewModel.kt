package dev.envoix.app.discovery

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.InviteCodec
import dev.envoix.app.OpLog
import dev.envoix.app.SettingsStore
import dev.envoix.app.ui.RoomControlInviteFormat
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
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
    val nfcReadinessOffer: NfcReadinessOffer? = null,
)

internal enum class DiscoveryMode {
    Off,
    BrowseNearby,
    SelectedPeer,
}

internal class DiscoveryViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private var identity =
        DiscoveryIdentityFactory.create(
            displayName = SettingsStore.settings.value.nearbyDisplayName,
        )
    private val registry = DiscoveryPeerRegistry()
    private val offerQueue = NearbyRendezvousOfferQueue()
    private var providers: List<DiscoveryProvider> = emptyList()
    private val lastLoggedAvailability = mutableMapOf<DiscoverySource, ProviderAvailability>()
    private val seenNfcReadinessOfferIds = LinkedHashSet<String>()

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
    private var advertisedNfcReadinessOfferId: String? = null
    private var currentPresence =
        PresenceSettings(
            displayName = identity.displayName,
            visibility =
                NearbyVisibility.fromPersisted(
                    SettingsStore.settings.value.nearbyVisibility,
                ),
            expiresAtEpochMs = SettingsStore.settings.value.nearbyVisibilityExpiresAtEpochMs,
        )

    init {
        viewModelScope.launch {
            SettingsStore.settings
                .map {
                    PresenceSettings(
                        displayName = it.nearbyDisplayName,
                        visibility = NearbyVisibility.fromPersisted(it.nearbyVisibility),
                        expiresAtEpochMs = it.nearbyVisibilityExpiresAtEpochMs,
                    )
                }.distinctUntilChanged()
                .collect { presence ->
                    val normalizedName =
                        DiscoveryPeerRegistry.sanitizeDisplayName(presence.displayName)
                            ?: "Android device"
                    val changed =
                        normalizedName != identity.displayName ||
                            presence != currentPresence
                    currentPresence = presence.copy(displayName = normalizedName)
                    identity = identity.copy(displayName = normalizedName)
                    _uiState.value = _uiState.value.copy(localName = normalizedName)
                    if (changed && started) restart()
                }
        }
    }

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
        providers.forEach { provider ->
            (provider as? NfcReadinessProvider)
                ?.setNfcReadinessOffer(advertisedNfcReadinessOfferId)
            provider.start(listener)
        }
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
                nfcReadinessOffer = null,
            )
    }

    fun startNfcReadinessOffer(): String {
        val offerId = NfcReadinessUuid.newOfferId()
        advertisedNfcReadinessOfferId = offerId
        providers
            .filterIsInstance<NfcReadinessProvider>()
            .forEach { provider -> provider.setNfcReadinessOffer(offerId) }
        return offerId
    }

    fun stopNfcReadinessOffer(offerId: String? = null) {
        if (offerId != null && advertisedNfcReadinessOfferId != offerId) return
        advertisedNfcReadinessOfferId = null
        providers
            .filterIsInstance<NfcReadinessProvider>()
            .forEach { provider -> provider.setNfcReadinessOffer(null) }
    }

    fun restart() {
        stopProviders()
        reconcile()
    }

    fun offerInvite(
        selection: NearbyPairingSelection,
        invite: String,
        completion: (error: String?) -> Unit,
    ) {
        val preferredSource =
            preferredRendezvousSource(
                selection = selection,
                roomInvitation = RoomControlInviteFormat.looksLikeRoomInvite(invite),
            )
        val provider =
            providers
                .filterIsInstance<NearbyRendezvousProvider>()
                .firstOrNull { it.source == preferredSource }
        if (!started || provider == null) {
            completion("Nearby invitation delivery is not available for this device")
            return
        }
        val activeGeneration = generation
        provider.offerInvite(selection, invite) { error ->
            viewModelScope.launch {
                if (!started || generation != activeGeneration) {
                    completion("Nearby discovery stopped")
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

    fun consumeNfcReadinessOffer(offerId: String) {
        if (_uiState.value.nfcReadinessOffer?.offerId == offerId) {
            _uiState.value = _uiState.value.copy(nfcReadinessOffer = null)
        }
    }

    override fun onCleared() {
        stop()
    }

    private fun publishPeers() {
        expireTemporaryVisibilityIfNeeded()
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
                    if (InviteCodec.parseForRouting(offer.invite) == null &&
                        !RoomControlInviteFormat.looksLikeRoomInvite(offer.invite)
                    ) {
                        OpLog.add("DISCOVERY provider=${offer.source.logName()} state=invalid_offer")
                        return@launch
                    }
                    if (requestedMode == DiscoveryMode.SelectedPeer && offer.senderPeerKey != selectedPeerKey) {
                        return@launch
                    }
                    if (offerQueue.add(offer, SystemClock.elapsedRealtime())) publishPeers()
                }
            }

            override fun onNfcReadinessOffer(offer: NfcReadinessOffer) {
                viewModelScope.launch {
                    if (!started ||
                        generation != activeGeneration ||
                        requestedMode != DiscoveryMode.BrowseNearby ||
                        offer.offerId == advertisedNfcReadinessOfferId ||
                        offer.offerId in seenNfcReadinessOfferIds
                    ) {
                        return@launch
                    }
                    if (seenNfcReadinessOfferIds.size >= MAX_SEEN_NFC_READINESS_OFFERS) {
                        seenNfcReadinessOfferIds.remove(seenNfcReadinessOfferIds.first())
                    }
                    seenNfcReadinessOfferIds += offer.offerId
                    _uiState.value = _uiState.value.copy(nfcReadinessOffer = offer)
                }
            }
        }

    private fun createProviders(): List<DiscoveryProvider> =
        listOf(
            BluetoothDiscoveryProvider(getApplication(), identity, advertisingAllowed()),
            MdnsDiscoveryProvider(
                getApplication(),
                identity,
                advertisingAllowed(),
                SettingsStore.settings.value.relay,
            ),
            WifiAwareDiscoveryProvider(),
        )

    private fun advertisingAllowed(): Boolean =
        nearbyAdvertisingAllowed(
            visibility = currentPresence.visibility,
            expiresAtEpochMs = currentPresence.expiresAtEpochMs,
            nowEpochMs = System.currentTimeMillis(),
        )

    private fun expireTemporaryVisibilityIfNeeded() {
        if (currentPresence.visibility != NearbyVisibility.EveryoneTenMinutes ||
            System.currentTimeMillis() < currentPresence.expiresAtEpochMs
        ) {
            return
        }
        SettingsStore.setNearbyVisibility(NearbyVisibility.Hidden.persistedValue)
    }

    private fun stoppedStatuses(): Map<DiscoverySource, ProviderStatus> =
        DiscoverySource.entries.associateWith { source ->
            ProviderStatus(source, ProviderAvailability.Stopped, "Discovery is stopped")
        }

    private fun Enum<*>.logName(): String = name.replace(UPPERCASE_BOUNDARY, "_").lowercase()

    companion object {
        private const val PEER_REFRESH_MS = 1_000L
        private const val MAX_SEEN_NFC_READINESS_OFFERS = 64
        private val UPPERCASE_BOUNDARY = Regex("(?<=[a-z])(?=[A-Z])")
    }
}

private data class PresenceSettings(
    val displayName: String,
    val visibility: NearbyVisibility,
    val expiresAtEpochMs: Long,
)

internal fun preferredRendezvousSource(
    selection: NearbyPairingSelection,
    roomInvitation: Boolean = true,
): DiscoverySource? =
    when {
        roomInvitation &&
            selection.nearbyInviteRoute?.let {
                NearbyInviteRoute.normalized(
                    endpointId = it.endpointId,
                    relayUrl = it.relayUrl,
                    directAddresses = it.directAddresses,
                )
            } != null &&
            DiscoverySource.Mdns in selection.sources ->
            DiscoverySource.Mdns
        DiscoverySource.Bluetooth in selection.sources ->
            DiscoverySource.Bluetooth
        else -> null
    }

internal fun canOfferNearbyRoom(selection: NearbyPairingSelection): Boolean = preferredRendezvousSource(selection) != null
