package dev.envoix.app

import dev.envoix.app.ffi.EnvoixException
import dev.envoix.app.ffi.FfiInviteRole
import dev.envoix.app.ffi.FfiPairingInvite
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiRendezvousPlan
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.makePairingInvite
import dev.envoix.app.ffi.parsePairingInvite
import dev.envoix.app.ffi.parsePairingInviteForRole
import dev.envoix.app.ffi.transferInvitationRoomId

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
    private const val INVALID_ACTIVITY_REFERENCE = "invalid-invitation"

    fun generate(
        creatorRole: String,
        broker: String,
        relay: String,
    ): CreatedInvite? {
        val role = creatorRole.inviteRole() ?: return null
        return try {
            makePairingInvite(role, broker, relay).toCreatedInvite()
        } catch (_: EnvoixException) {
            null
        }
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
    ): ParsedInvite? {
        val role = localRole.inviteRole() ?: return null
        return try {
            parsePairingInviteForRole(input, role).toParsedInvite(input)
        } catch (_: EnvoixException) {
            null
        }
    }

    /** Secret-free activity identity shared by both sides of one typed InviteV2 send. */
    fun activityReference(
        invitationReference: String,
        localRole: String,
        creator: Boolean,
    ): String =
        try {
            transferInvitationRoomId(
                invitationTransferRequest(invitationReference, localRole, creator),
            )
        } catch (_: EnvoixException) {
            INVALID_ACTIVITY_REFERENCE
        }

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

    private fun FfiPairingInvite.toCreatedInvite(): CreatedInvite? {
        val expiresAt = expiresAt.checkedLong() ?: return null
        return CreatedInvite(
            roomCode = roomCode,
            payload = payload,
            reference = roomCode,
            broker = broker,
            relay = relayUrls.firstOrNull(),
            creatorRole = creatorRole.wireName(),
            joinerRole = joinerRole.wireName(),
            expiresAt = expiresAt,
        )
    }

    private fun FfiPairingInvite.toParsedInvite(reference: String? = null): ParsedInvite? {
        val expiresAt = expiresAt.checkedLong() ?: return null
        return ParsedInvite(
            reference = reference,
            broker = broker,
            relay = relayUrls.firstOrNull(),
            creatorRole = creatorRole.wireName(),
            joinerRole = joinerRole.wireName(),
            expiresAt = expiresAt,
        )
    }

    private fun ULong.checkedLong(): Long? = takeIf { it <= Long.MAX_VALUE.toULong() }?.toLong()

    private fun invitationTransferRequest(
        invitationReference: String,
        localRole: String,
        creator: Boolean,
    ) = FfiTransferRequest(
        direction =
            when (localRole) {
                "send" -> FfiTransferDirection.SEND
                "receive" -> FfiTransferDirection.RECEIVE
                else -> error("Transfer invitation role must be send or receive")
            },
        mode = if (creator) FfiTransferMode.ROOM else FfiTransferMode.INVITE,
        peerDescriptor = "",
        invite = if (creator) "" else invitationReference,
        code = if (creator) invitationReference else "",
        token = "",
        rememberConsent = false,
        rememberedCredentialRef = "",
        rememberedGeneration = 0uL,
        rememberedPreviousGeneration = null,
        broker = "",
        relay = "",
        configPath = "",
        pathPolicy = FfiPathPolicy.AUTO,
        rendezvous =
            FfiRendezvousPlan(
                useRoom = true,
                useMdns = false,
                internetAvailable = true,
            ),
    )

    private fun FfiInviteRole.wireName() =
        when (this) {
            FfiInviteRole.SEND -> "send"
            FfiInviteRole.RECEIVE -> "receive"
        }

    private fun String.inviteRole(): FfiInviteRole? =
        when (this) {
            "send" -> FfiInviteRole.SEND
            "receive" -> FfiInviteRole.RECEIVE
            else -> null
        }
}
