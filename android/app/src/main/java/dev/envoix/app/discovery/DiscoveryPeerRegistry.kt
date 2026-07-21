package dev.envoix.app.discovery

internal class DiscoveryPeerRegistry(
    private val observationTtlMs: Long = DEFAULT_OBSERVATION_TTL_MS,
) {
    private val observations = mutableMapOf<String, MutableMap<DiscoverySource, DiscoveryObservation>>()

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
                endpoint = sanitizeText(observation.endpoint, MAX_ENDPOINT_LENGTH),
            )
        val bySource = observations.getOrPut(peerKey, ::mutableMapOf)
        val previous = bySource[observation.source]
        if (previous != null && previous.seenAtMs > normalized.seenAtMs) return false
        bySource[observation.source] = normalized
        return true
    }

    fun clear() {
        observations.clear()
    }

    fun peers(nowMs: Long): List<DiscoveredPeer> {
        require(nowMs >= 0) { "nowMs must not be negative" }
        val peerIterator = observations.iterator()
        while (peerIterator.hasNext()) {
            val bySource = peerIterator.next().value
            val sourceIterator = bySource.iterator()
            while (sourceIterator.hasNext()) {
                val observation = sourceIterator.next().value
                if (nowMs - observation.seenAtMs > observationTtlMs) sourceIterator.remove()
            }
            if (bySource.isEmpty()) peerIterator.remove()
        }

        return observations
            .map { (peerKey, bySource) ->
                val values = bySource.values
                DiscoveredPeer(
                    peerKey = peerKey,
                    displayName = values.latestNonBlank { it.displayName },
                    sources = bySource.keys.toSet(),
                    lastSeenAtMs = values.maxOf { it.seenAtMs },
                    rssi = values.latestValue { it.rssi },
                    endpoint = values.latestNonBlank { it.endpoint },
                )
            }.sortedWith(compareByDescending<DiscoveredPeer> { it.lastSeenAtMs }.thenBy { it.peerKey })
    }

    companion object {
        const val DEFAULT_OBSERVATION_TTL_MS = 20_000L
        const val MAX_DISPLAY_NAME_LENGTH = 48
        const val MAX_ENDPOINT_LENGTH = 96
        const val PEER_KEY_HEX_LENGTH = 16

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
