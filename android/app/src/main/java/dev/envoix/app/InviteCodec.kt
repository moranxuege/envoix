package dev.envoix.app

import org.json.JSONObject

class CreatedInvite(
    val roomCode: String,
    val payload: String,
    val reference: String,
    val broker: String,
    val relay: String?,
    val creatorRole: String,
    val joinerRole: String,
    val expiresAt: Long,
) {
    override fun toString() = "CreatedInvite(<redacted>)"
}

data class ParsedInvite(
    val reference: String?,
    val broker: String,
    val relay: String?,
    val creatorRole: String,
    val joinerRole: String,
    val expiresAt: Long,
)

/** All invitation parsing and Room-Code normalization is delegated to Rust. */
object InviteCodec {
    fun generate(
        creatorRole: String,
        broker: String,
        relay: String,
    ): CreatedInvite? {
        val value = json(Native.generateInvite(creatorRole, broker, relay)) ?: return null
        if (value.has("error")) return null
        return CreatedInvite(
            roomCode = value.getString("code"),
            payload = value.getString("payload"),
            reference = value.getString("reference"),
            broker = value.getString("broker"),
            relay = value.strOrNull("relay"),
            creatorRole = value.getString("creatorRole"),
            joinerRole = value.getString("joinerRole"),
            expiresAt = value.getLong("expiresAt"),
        )
    }

    /** Parse for deep-link routing. The credential itself is not returned. */
    fun parseForRouting(input: String): ParsedInvite? = parsed(Native.parseInvite(input))

    /** Parse against the role fixed by an existing Send or Receive flow. */
    fun parseForRole(
        input: String,
        localRole: String,
    ): ParsedInvite? = parsed(Native.parseInviteForRole(input, localRole))

    fun normalizeRoomCode(input: String): String? {
        val value = json(Native.normalizeRoomCode(input)) ?: return null
        return value.optString("code").ifEmpty { null }
    }

    /** UI-only formatter; Rust remains authoritative when the transfer starts. */
    fun formatRoomCode(input: String): String {
        val compact = input.filterNot { it == '-' }.take(14)
        if (!compact.all { it.isLetterOrDigit() && it.code < 128 }) return input
        return buildString {
            compact.forEachIndexed { index, character ->
                if (index == 6 || index == 10) append('-')
                append(character.lowercaseChar())
            }
        }
    }

    private fun parsed(raw: String): ParsedInvite? {
        val value = json(raw) ?: return null
        if (value.has("error")) return null
        return ParsedInvite(
            reference = value.strOrNull("reference"),
            broker = value.getString("broker"),
            relay = value.strOrNull("relay"),
            creatorRole = value.getString("creatorRole"),
            joinerRole = value.getString("joinerRole"),
            expiresAt = value.getLong("expiresAt"),
        )
    }

    private fun json(value: String) = runCatching { JSONObject(value) }.getOrNull()

    private fun JSONObject.strOrNull(key: String) = if (isNull(key)) null else optString(key).ifEmpty { null }
}
