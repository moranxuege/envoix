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
        ConnectionPathKind.Direct -> AppText.value("Direct path", "直连路径", language)
        ConnectionPathKind.Relay -> AppText.value("Relay path", "中继路径", language)
        ConnectionPathKind.WifiAware -> AppText.value("Wi-Fi Aware path", "Wi-Fi Aware 路径", language)
        ConnectionPathKind.Other -> AppText.value("Other path", "其他路径", language)
        null -> null
    }
