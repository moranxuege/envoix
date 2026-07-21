package dev.envoix.app.discovery

enum class DiscoverySource {
    Bluetooth,
    Mdns,
    WifiAware,
}

enum class ProviderAvailability {
    Stopped,
    Starting,
    Ready,
    Degraded,
    PermissionRequired,
    Disabled,
    Unsupported,
    TemporarilyUnavailable,
    Reserved,
    Error,
}

data class ProviderStatus(
    val source: DiscoverySource,
    val availability: ProviderAvailability,
    val detail: String,
)

data class DiscoveryObservation(
    val peerKey: String,
    val source: DiscoverySource,
    val seenAtMs: Long,
    val displayName: String? = null,
    val rssi: Int? = null,
    val endpoint: String? = null,
)

data class DiscoveredPeer(
    val peerKey: String,
    val displayName: String?,
    val sources: Set<DiscoverySource>,
    val lastSeenAtMs: Long,
    val rssi: Int?,
    val endpoint: String?,
)

/**
 * Untrusted UI context carried into the existing authenticated pairing flow.
 * Endpoint and credential material are deliberately absent: tapping a public
 * discovery result must never authorize a connection on its own.
 */
data class NearbyPairingSelection(
    val discoveryPeerKey: String,
    val displayName: String?,
    val sources: Set<DiscoverySource>,
) {
    companion object {
        fun from(peer: DiscoveredPeer) =
            NearbyPairingSelection(
                discoveryPeerKey = peer.peerKey,
                displayName = peer.displayName,
                sources = peer.sources,
            )
    }
}

data class NearbyRendezvousOffer(
    val requestId: String,
    val senderPeerKey: String,
    val senderDisplayName: String?,
    val invite: String,
)

interface DiscoveryListener {
    fun onObservation(observation: DiscoveryObservation)

    fun onStatus(status: ProviderStatus)

    fun onRendezvousOffer(offer: NearbyRendezvousOffer) = Unit
}

interface DiscoveryProvider {
    val source: DiscoverySource

    fun start(listener: DiscoveryListener)

    fun stop()
}

interface NearbyRendezvousProvider {
    fun offerInvite(
        peerKey: String,
        invite: String,
        completion: (error: String?) -> Unit,
    )
}
