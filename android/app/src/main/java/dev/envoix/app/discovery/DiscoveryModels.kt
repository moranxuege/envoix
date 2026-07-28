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
    val nearbyInviteRoute: NearbyInviteRoute? = null,
)

data class DiscoveredPeer(
    val peerKey: String,
    val displayName: String?,
    val sources: Set<DiscoverySource>,
    val lastSeenAtMs: Long,
    val rssi: Int?,
    val nearbyInviteRoute: NearbyInviteRoute?,
)

/**
 * Untrusted UI context carried into the existing authenticated pairing flow.
 * [nearbyInviteRoute] is a frozen routing snapshot captured from discovery,
 * not a verified person or remembered-device identity. Iroh authenticates that
 * the invitation reaches the selected endpoint; the room still authenticates
 * its own invitation independently.
 */
data class NearbyPairingSelection(
    val discoveryPeerKey: String,
    val displayName: String?,
    val sources: Set<DiscoverySource>,
    val nearbyInviteRoute: NearbyInviteRoute? = null,
) {
    companion object {
        fun from(peer: DiscoveredPeer) =
            NearbyPairingSelection(
                discoveryPeerKey = peer.peerKey,
                displayName = peer.displayName,
                sources = peer.sources,
                nearbyInviteRoute = peer.nearbyInviteRoute,
            )
    }
}

/**
 * Immutable native-Bonjour route for one foreground nearby-invitation inbox.
 * Direct addresses are opaque canonical `SocketAddr::to_string()` values from
 * the Rust endpoint. Native platforms agree on four bounded TXT slots.
 */
class NearbyInviteRoute private constructor(
    val endpointId: String,
    val relayUrl: String?,
    directAddresses: List<String>,
) {
    val directAddresses: List<String> = directAddresses.toList()

    override fun equals(other: Any?): Boolean =
        other is NearbyInviteRoute &&
            endpointId == other.endpointId &&
            relayUrl == other.relayUrl &&
            directAddresses == other.directAddresses

    override fun hashCode(): Int {
        var result = endpointId.hashCode()
        result = 31 * result + (relayUrl?.hashCode() ?: 0)
        result = 31 * result + directAddresses.hashCode()
        return result
    }

    companion object {
        fun normalized(
            endpointId: String?,
            relayUrl: String?,
            directAddresses: Iterable<String>,
        ): NearbyInviteRoute? {
            val normalizedEndpoint = normalizeNearbyInboxEndpointId(endpointId) ?: return null
            val normalizedRelay =
                normalizeRouteComponent(
                    relayUrl,
                    MAX_NEARBY_RELAY_URL_BYTES,
                )
            val normalizedAddresses =
                directAddresses
                    .mapNotNull { value ->
                        normalizeRouteComponent(
                            value,
                            MAX_NEARBY_DIRECT_ADDRESS_BYTES,
                        )
                    }.distinct()
                    .take(MAX_NEARBY_DIRECT_ADDRESSES)
                    .toList()
            if (normalizedRelay == null && normalizedAddresses.isEmpty()) return null
            return NearbyInviteRoute(
                endpointId = normalizedEndpoint,
                relayUrl = normalizedRelay,
                directAddresses = normalizedAddresses,
            )
        }
    }
}

data class NearbyRendezvousOffer(
    val requestId: String,
    val senderPeerKey: String,
    val senderDisplayName: String?,
    val invite: String,
    val senderEndpointId: String? = null,
    val source: DiscoverySource = DiscoverySource.Bluetooth,
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
    val source: DiscoverySource

    fun offerInvite(
        selection: NearbyPairingSelection,
        invite: String,
        completion: (error: String?) -> Unit,
    )
}

internal fun normalizeNearbyInboxEndpointId(value: String?): String? =
    value
        ?.trim()
        ?.lowercase()
        ?.takeIf { NEARBY_INBOX_ENDPOINT_ID.matches(it) }

private fun normalizeRouteComponent(
    value: String?,
    maxUtf8Bytes: Int,
): String? =
    value
        ?.trim()
        ?.takeIf(String::isNotEmpty)
        ?.takeIf { candidate ->
            candidate.toByteArray(Charsets.UTF_8).size <= maxUtf8Bytes &&
                candidate.none { character -> character.isISOControl() || character.isWhitespace() }
        }

internal const val MAX_NEARBY_DIRECT_ADDRESSES = 4
internal const val MAX_NEARBY_DIRECT_ADDRESS_BYTES = 128

// Android requires TXT key bytes + value bytes to be strictly below 255.
// `irelay` is six ASCII bytes, leaving 248 bytes for its UTF-8 value.
internal const val MAX_NEARBY_RELAY_URL_BYTES = 248

private val NEARBY_INBOX_ENDPOINT_ID = Regex("^[a-z2-7]{52}$")
