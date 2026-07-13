package dev.envoix.app

import dev.envoix.app.ffi.FfiInviteRole
import dev.envoix.app.ffi.makePairingInvite
import dev.envoix.app.ffi.parsePairingInvite

/** A parsed pairing invite (from a typed code or a scanned QR). */
data class ParsedInvite(
    val code: String,
    val broker: String?,
    val relay: String?,
    val role: String?, // "send" / "receive" / null
)

/** Kotlin wrapper over the shared UniFFI invite codec. */
object InviteCodec {
    /** Direct transfer invites are produced by the receiver's live UniFFI session. */
    fun isTransferInvite(input: String): Boolean {
        val lower = input.trim().lowercase()
        return lower.startsWith("envoix:") && !lower.startsWith("envoix://pair/")
    }

    /** Generate a room invite for [role]; returns (code, qrPayload) or null on error. */
    fun generate(
        role: String,
        broker: String,
        relay: String,
    ): Pair<String, String>? {
        val ffiRole =
            when (role) {
                "send" -> FfiInviteRole.SEND
                "receive" -> FfiInviteRole.RECEIVE
                else -> FfiInviteRole.UNKNOWN
            }
        val invite = runCatching { makePairingInvite(ffiRole, broker, relay) }.getOrNull() ?: return null
        return invite.code to invite.payload
    }

    /** Parse a typed code or scanned `envoix://` payload; null on error. */
    fun parse(input: String): ParsedInvite? {
        val invite = runCatching { parsePairingInvite(input) }.getOrNull() ?: return null
        return ParsedInvite(
            code = invite.code,
            broker = invite.broker.ifBlank { null },
            relay = invite.relay.ifBlank { null },
            role = invite.role.toRoleString(),
        )
    }

    /** The role a joiner should take, given a scanned invite's role. */
    fun oppositeRole(scanned: String?): String? =
        when (scanned) {
            "send" -> "receive"
            "receive" -> "send"
            else -> null
        }

    private fun FfiInviteRole.toRoleString(): String? =
        when (this) {
            FfiInviteRole.SEND -> "send"
            FfiInviteRole.RECEIVE -> "receive"
            FfiInviteRole.UNKNOWN -> null
        }
}
