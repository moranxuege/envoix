package dev.envoix.app

import dev.envoix.app.ui.AppText

internal enum class ConnectionPathKind(
    val wire: String,
) {
    Direct("direct"),
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
        ConnectionPathKind.Relay -> AppText.value("Relay", "中继", language)
        ConnectionPathKind.WifiAware -> AppText.value("Wi-Fi Aware", "Wi-Fi Aware", language)
        ConnectionPathKind.Other -> AppText.value("Other", "其他", language)
        null -> null
    }
