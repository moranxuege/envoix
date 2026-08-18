package dev.envoix.app

import dev.envoix.app.ffi.EnvoixException
import dev.envoix.app.ffi.FfiInviteRole
import dev.envoix.app.ffi.FfiPairingInvite
import dev.envoix.app.ffi.parsePairingInvite
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

/** Invitation parsing is delegated to Rust; only foreground Room Code typing is formatted locally. */
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
    fun parseForRouting(input: String): ParsedInvite? =
        try {
            parsePairingInvite(input).toParsedInvite()
        } catch (_: EnvoixException) {
            null
        }

    /** Parse against the role fixed by an existing Send or Receive flow. */
    fun parseForRole(
        input: String,
        localRole: String,
    ): ParsedInvite? = parsed(Native.parseInviteForRole(input, localRole))

    /** UI-only formatter; Rust remains authoritative when the transfer starts. */
    fun formatRoomCode(input: String): String {
        val compact = StringBuilder(14)
        var separatorAfterSix = false
        var separatorAfterTen = false
        for (character in input) {
            when {
                character.isLetterOrDigit() && character.code < 128 -> {
                    if (compact.length == 14) return input
                    compact.append(character.lowercaseChar())
                }
                character == '-' && compact.length == 6 && !separatorAfterSix ->
                    separatorAfterSix = true
                character == '-' && compact.length == 10 && !separatorAfterTen ->
                    separatorAfterTen = true
                else -> return input
            }
        }
        if (compact.length == 14 && separatorAfterSix != separatorAfterTen) return input
        return buildString {
            compact.forEachIndexed { index, character ->
                if (index == 6 || index == 10) append('-')
                append(character)
            }
            if (compact.length == 6 && separatorAfterSix) append('-')
            if (compact.length == 10 && separatorAfterTen) append('-')
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

    private fun FfiPairingInvite.toParsedInvite(): ParsedInvite? {
        val expiresAt = expiresAt.takeIf { it <= Long.MAX_VALUE.toULong() }?.toLong() ?: return null
        return ParsedInvite(
            reference = null,
            broker = broker,
            relay = relayUrls.firstOrNull(),
            creatorRole = creatorRole.wireName(),
            joinerRole = joinerRole.wireName(),
            expiresAt = expiresAt,
        )
    }

    private fun FfiInviteRole.wireName() =
        when (this) {
            FfiInviteRole.SEND -> "send"
            FfiInviteRole.RECEIVE -> "receive"
        }

    private fun json(value: String) = runCatching { JSONObject(value) }.getOrNull()

    private fun JSONObject.strOrNull(key: String) = if (isNull(key)) null else optString(key).ifEmpty { null }
}
