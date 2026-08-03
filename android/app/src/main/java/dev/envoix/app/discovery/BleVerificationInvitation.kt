package dev.envoix.app.discovery

import java.net.URI
import java.net.URLDecoder
import java.net.URLEncoder
import java.security.MessageDigest
import java.security.SecureRandom

/** Secret-free BLE locator. The six-digit PAKE input stays on the two screens. */
internal data class BleVerificationInvitation(
    val verificationCode: String,
    val privateInvitation: String,
    val publicOffer: String,
    val expiresAtEpochMs: Long,
) {
    companion object {
        const val URL_PREFIX = "envoix://ble/v1/"
        private const val LIFETIME_MS = 300_000L
        private val random = SecureRandom()

        fun create(
            broker: String,
            relay: String,
            nowEpochMs: Long = System.currentTimeMillis(),
        ): BleVerificationInvitation {
            val normalizedBroker = broker.trim()
            require(normalizedBroker.isNotEmpty()) { "A rendezvous broker is required" }
            val code = digits()
            var locator = digits()
            while (locator == code) locator = digits()
            val expiry = Math.addExact(nowEpochMs, LIFETIME_MS)
            val offer = url("ble", listOf("v1", locator), normalizedBroker, relay.trim(), expiry)
            return BleVerificationInvitation(
                code,
                privateUrl(locator, code, offer, normalizedBroker, relay.trim(), expiry),
                offer,
                expiry,
            )
        }

        fun resolve(
            publicOffer: String,
            verificationCode: String,
            nowEpochMs: Long = System.currentTimeMillis(),
        ): String? {
            if (!verificationCode.isDigits()) return null
            val offer = parse(publicOffer, nowEpochMs) ?: return null
            return privateUrl(
                offer.locator,
                verificationCode,
                publicOffer,
                offer.broker,
                offer.relay,
                offer.expiry,
            )
        }

        fun isPublicOffer(
            value: String,
            nowEpochMs: Long = System.currentTimeMillis(),
        ): Boolean = parse(value, nowEpochMs) != null

        private fun privateUrl(
            locator: String,
            code: String,
            publicOffer: String,
            broker: String,
            relay: String,
            expiry: Long,
        ): String {
            val secret =
                MessageDigest
                    .getInstance("SHA-256")
                    .digest("envoix BLE verification v1\u0000$publicOffer\u0000$code".toByteArray())
                    .take(4)
                    .joinToString("") { "%02x".format(it.toInt() and 0xff) }
            return url(
                "room",
                listOf("$locator-${secret.take(4)}-${secret.takeLast(4)}"),
                broker,
                relay,
                expiry,
            )
        }

        private fun url(
            host: String,
            path: List<String>,
            broker: String,
            relay: String,
            expiry: Long,
        ): String {
            val query =
                buildList {
                    add("broker=${broker.urlEncode()}")
                    if (relay.isNotEmpty()) add("relay=${relay.urlEncode()}")
                    add("expires=${expiry / 1_000L}")
                }.joinToString("&")
            return "envoix://$host/${path.joinToString("/")}?$query"
        }

        private fun parse(
            value: String,
            now: Long,
        ): Parsed? {
            if (value != value.trim() || value.toByteArray().size > 2_048) return null
            val uri = runCatching { URI(value) }.getOrNull() ?: return null
            val path = (uri.path ?: return null).split('/').filter(String::isNotEmpty)
            val rawFields = uri.rawQuery?.split('&') ?: return null
            if (rawFields.any { '=' !in it }) return null
            val fields =
                runCatching {
                    rawFields.map { field ->
                        val parts = field.split('=', limit = 2)
                        parts[0].urlDecode() to parts[1].urlDecode()
                    }
                }.getOrNull() ?: return null
            if (uri.scheme != "envoix" ||
                uri.host != "ble" ||
                uri.userInfo != null ||
                uri.port != -1 ||
                uri.fragment != null ||
                path.size != 2 ||
                path[0] != "v1" ||
                !path[1].isDigits() ||
                fields.any { it.first !in setOf("broker", "relay", "expires") }
            ) {
                return null
            }
            val broker = fields.one("broker")?.trim()?.takeIf(String::isNotEmpty) ?: return null
            val seconds = fields.one("expires")?.toLongOrNull() ?: return null
            if (fields.count { it.first == "relay" } > 1) return null
            val relay = fields.one("relay")?.trim().orEmpty()
            val expiry = runCatching { Math.multiplyExact(seconds, 1_000L) }.getOrNull() ?: return null
            if (broker.toByteArray().size > 1_024 ||
                relay.toByteArray().size > 1_024 ||
                expiry <= now ||
                expiry - now > LIFETIME_MS * 2
            ) {
                return null
            }
            return Parsed(path[1], broker, relay, expiry)
        }

        private fun List<Pair<String, String>>.one(name: String): String? =
            filter { it.first == name }.takeIf { it.size == 1 }?.single()?.second

        private fun String.urlEncode(): String = URLEncoder.encode(this, Charsets.UTF_8.name())

        private fun String.urlDecode(): String = URLDecoder.decode(this, Charsets.UTF_8.name())

        private fun digits(): String = random.nextInt(1_000_000).toString().padStart(6, '0')

        private fun String.isDigits(): Boolean = length == 6 && all { it in '0'..'9' }

        private data class Parsed(
            val locator: String,
            val broker: String,
            val relay: String,
            val expiry: Long,
        )
    }
}
