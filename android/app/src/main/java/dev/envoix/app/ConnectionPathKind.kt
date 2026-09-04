package dev.envoix.app

internal enum class ConnectionPathKind(
    val wire: String,
) {
    Direct("direct"),
    DirectIpv4("direct_ipv4"),
    DirectIpv6("direct_ipv6"),
    Relay("relay"),
    WifiAware("wifi_aware"),
    Other("other"),
    ;

    companion object {
        /**
         * Accepts the structured wire kind and classifies legacy endpoint-rich
         * values without retaining or presenting their details.
         */
        fun fromWireOrLegacy(value: String?): ConnectionPathKind? {
            val normalized = value?.trim()?.lowercase().orEmpty()
            if (normalized.isEmpty()) return null
            return when {
                normalized == Direct.wire ||
                    normalized.startsWith("direct ") ||
                    normalized.startsWith("direct(") -> Direct
                normalized == DirectIpv4.wire -> DirectIpv4
                normalized == DirectIpv6.wire -> DirectIpv6
                normalized == Relay.wire ||
                    normalized.startsWith("relay ") ||
                    normalized.startsWith("relay(") -> Relay
                normalized == WifiAware.wire ||
                    normalized == "wifi aware" ||
                    normalized == "wifi-aware" -> WifiAware
                else -> Other
            }
        }
    }
}
