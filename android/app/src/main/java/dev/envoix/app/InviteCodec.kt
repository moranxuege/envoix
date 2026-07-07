package dev.envoix.app

import org.json.JSONObject

/** A parsed pairing invite (from a typed code or a scanned QR). */
data class ParsedInvite(
    val code: String,
    val broker: String?,
    val relay: String?,
    val role: String?, // "send" / "receive" / null
)

/** Kotlin wrapper over the JNI [Native.generateInvite]/[Native.parseInvite]. */
object InviteCodec {
    /** Generate a room invite for [role]; returns (code, qrPayload) or null on error. */
    fun generate(role: String, broker: String, relay: String): Pair<String, String>? {
        val o = json(Native.generateInvite(role, broker, relay)) ?: return null
        if (o.has("error")) return null
        return o.getString("code") to o.getString("payload")
    }

    /** Parse a typed code or scanned `envoix://` payload; null on error. */
    fun parse(input: String): ParsedInvite? {
        val o = json(Native.parseInvite(input)) ?: return null
        if (o.has("error")) return null
        return ParsedInvite(
            code = o.getString("code"),
            broker = o.strOrNull("broker"),
            relay = o.strOrNull("relay"),
            role = o.strOrNull("role"),
        )
    }

    /** The role a joiner should take, given a scanned invite's role. */
    fun oppositeRole(scanned: String?): String? = when (scanned) {
        "send" -> "receive"
        "receive" -> "send"
        else -> null
    }

    private fun json(s: String) = runCatching { JSONObject(s) }.getOrNull()
    private fun JSONObject.strOrNull(k: String) = if (isNull(k)) null else optString(k).ifEmpty { null }
}
