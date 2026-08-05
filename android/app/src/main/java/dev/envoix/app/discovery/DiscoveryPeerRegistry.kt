package dev.envoix.app.discovery

internal class DiscoveryPeerRegistry(
    private val observationTtlMs: Long = DEFAULT_OBSERVATION_TTL_MS,
) {
    private val observations = mutableMapOf<String, MutableMap<DiscoverySource, DiscoveryObservation>>()
    private val peerOrdinals = mutableMapOf<String, Long>()
    private var nextPeerOrdinal = 0L

    init {
        require(observationTtlMs > 0) { "observationTtlMs must be positive" }
    }

    fun upsert(observation: DiscoveryObservation): Boolean {
        val peerKey = normalizePeerKey(observation.peerKey) ?: return false
        if (observation.seenAtMs < 0) return false
        val normalized =
            observation.copy(
                peerKey = peerKey,
                displayName = sanitizeText(observation.displayName, MAX_DISPLAY_NAME_LENGTH),
                nearbyInviteRoute =
                    observation.nearbyInviteRoute?.let { route ->
                        NearbyInviteRoute.normalized(
                            endpointId = route.endpointId,
                            relayUrl = route.relayUrl,
                            directAddresses = route.directAddresses,
                        )
                    },
            )
        if (peerKey !in observations && observations.size >= MAX_PEERS) return false
        val bySource = observations.getOrPut(peerKey, ::mutableMapOf)
        val previous = bySource[observation.source]
        if (previous != null && previous.seenAtMs > normalized.seenAtMs) return false
        bySource[observation.source] =
            normalized.copy(displayName = normalized.displayName ?: previous?.displayName)
        peerOrdinals.getOrPut(peerKey) { nextPeerOrdinal++ }
        return true
    }

    fun clear() {
        observations.clear()
        peerOrdinals.clear()
        nextPeerOrdinal = 0L
    }

    fun peers(nowMs: Long): List<DiscoveredPeer> {
        require(nowMs >= 0) { "nowMs must not be negative" }
        val peerIterator = observations.iterator()
        while (peerIterator.hasNext()) {
            val (peerKey, bySource) = peerIterator.next()
            val sourceIterator = bySource.iterator()
            while (sourceIterator.hasNext()) {
                val observation = sourceIterator.next().value
                if (nowMs - observation.seenAtMs > observationTtlMs) sourceIterator.remove()
            }
            if (bySource.isEmpty()) {
                peerIterator.remove()
                peerOrdinals.remove(peerKey)
            }
        }

        return observations
            .map { (peerKey, bySource) ->
                val values = bySource.values
                DiscoveredPeer(
                    peerKey = peerKey,
                    displayName =
                        DISPLAY_NAME_SOURCE_PREFERENCE
                            .firstNotNullOfOrNull { source -> bySource[source]?.displayName }
                            ?: values.latestNonBlank { it.displayName },
                    sources = bySource.keys.toSet(),
                    lastSeenAtMs = values.maxOf { it.seenAtMs },
                    rssi = values.latestValue { it.rssi },
                    nearbyInviteRoute = values.latestValue { it.nearbyInviteRoute },
                )
            }.sortedBy { peerOrdinals.getValue(it.peerKey) }
    }

    companion object {
        const val DEFAULT_OBSERVATION_TTL_MS = 20_000L
        const val MAX_DISPLAY_NAME_LENGTH = 48
        const val MAX_PEERS = 64
        const val PEER_KEY_HEX_LENGTH = 16
        private val DISPLAY_NAME_SOURCE_PREFERENCE =
            listOf(DiscoverySource.Mdns, DiscoverySource.WifiAware, DiscoverySource.Bluetooth)

        fun normalizePeerKey(value: String): String? {
            val normalized = value.trim().lowercase()
            return normalized.takeIf {
                it.length == PEER_KEY_HEX_LENGTH && it.all { character -> character in '0'..'9' || character in 'a'..'f' }
            }
        }

        fun sanitizeDisplayName(value: String?): String? = sanitizeText(value, MAX_DISPLAY_NAME_LENGTH)

        private fun sanitizeText(
            value: String?,
            maxLength: Int,
        ): String? =
            value
                ?.trim()
                ?.replace(Regex("\\s+"), " ")
                ?.take(maxLength)
                ?.ifBlank { null }
    }
}

private fun Collection<DiscoveryObservation>.latestNonBlank(selector: (DiscoveryObservation) -> String?): String? =
    asSequence()
        .mapNotNull { item -> selector(item)?.let { item to it } }
        .maxByOrNull { (item, _) -> item.seenAtMs }
        ?.second

private fun <R> Collection<DiscoveryObservation>.latestValue(selector: (DiscoveryObservation) -> R?): R? =
    asSequence()
        .mapNotNull { item -> selector(item)?.let { item to it } }
        .maxByOrNull { (item, _) -> item.seenAtMs }
        ?.second
