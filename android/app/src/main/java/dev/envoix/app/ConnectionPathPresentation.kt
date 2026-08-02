package dev.envoix.app

import dev.envoix.app.ui.AppText

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

internal fun connectionPathLabel(
    value: String?,
    language: String,
): String? =
    when (ConnectionPathKind.fromWireOrLegacy(value)) {
        ConnectionPathKind.Direct -> AppText.value("Direct", "直连", language)
        ConnectionPathKind.DirectIpv4 -> AppText.value("Direct · IPv4", "直连 · IPv4", language)
        ConnectionPathKind.DirectIpv6 -> AppText.value("Direct · IPv6", "直连 · IPv6", language)
        ConnectionPathKind.Relay -> AppText.value("Relay", "中继", language)
        ConnectionPathKind.WifiAware -> AppText.value("Wi-Fi Aware", "Wi-Fi Aware", language)
        ConnectionPathKind.Other -> AppText.value("Other", "其他", language)
        null -> null
    }
