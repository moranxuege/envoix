package dev.envoix.app.discovery

internal enum class NearbyVisibility(
    val persistedValue: String,
) {
    Hidden("hidden"),
    EveryoneTenMinutes("everyone_10m"),
    Foreground("foreground"),
    ;

    companion object {
        fun fromPersisted(value: String): NearbyVisibility =
            entries.firstOrNull { it.persistedValue == value }
                ?: Hidden
    }
}

internal fun nearbyAdvertisingAllowed(
    visibility: NearbyVisibility,
    expiresAtEpochMs: Long,
    nowEpochMs: Long,
): Boolean =
    when (visibility) {
        NearbyVisibility.Hidden -> false
        NearbyVisibility.EveryoneTenMinutes -> nowEpochMs < expiresAtEpochMs
        NearbyVisibility.Foreground -> true
    }
