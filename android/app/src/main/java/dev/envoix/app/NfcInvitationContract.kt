package dev.envoix.app

import java.nio.charset.StandardCharsets
import java.util.Base64

internal object NfcInvitationContract {
    const val MAX_INVITATION_BYTES = 8_211
    const val CARRIER_PREFIX = "https://ece4410j-nuub.github.io/nfc/v1/#"

    private const val MAX_ENCODED_INVITATION_BYTES =
        (MAX_INVITATION_BYTES * 4 + 2) / 3
    val maxCarrierBytes = CARRIER_PREFIX.length + MAX_ENCODED_INVITATION_BYTES
    private val encoder = Base64.getUrlEncoder().withoutPadding()
    private val decoder = Base64.getUrlDecoder()

    private val prefixes =
        listOf(
            "envoix://invite/v2/",
            "envoix://room/",
        )

    fun encode(invitation: String): ByteArray? {
        val invitationBytes = canonicalBytes(invitation) ?: return null
        return (
            CARRIER_PREFIX +
                encoder.encodeToString(invitationBytes)
        ).toByteArray(StandardCharsets.US_ASCII)
    }

    fun isCanonicalInvitation(invitation: String): Boolean =
        invitation.length in 1..MAX_INVITATION_BYTES &&
            invitation.all { character -> character.code in 0x21..0x7e } &&
            hasSupportedPrefix(invitation)

    fun decode(bytes: ByteArray): String? {
        if (bytes.isEmpty() ||
            bytes.size > maxCarrierBytes ||
            !bytes.all(::isPrintableAscii)
        ) {
            return null
        }
        val value = String(bytes, StandardCharsets.US_ASCII)
        if (!value.startsWith(CARRIER_PREFIX)) {
            return value.takeIf { canonicalBytes(it) != null }
        }

        val token = value.substring(CARRIER_PREFIX.length)
        if (token.isEmpty() ||
            token.length > MAX_ENCODED_INVITATION_BYTES ||
            !token.all(::isBase64UrlCharacter)
        ) {
            return null
        }
        val decoded =
            try {
                decoder.decode(token)
            } catch (_: IllegalArgumentException) {
                return null
            }
        if (decoded.size > MAX_INVITATION_BYTES ||
            encoder.encodeToString(decoded) != token
        ) {
            return null
        }
        return canonicalInvitation(decoded)
    }

    private fun canonicalBytes(invitation: String): ByteArray? {
        if (!isCanonicalInvitation(invitation)) return null
        return invitation.toByteArray(StandardCharsets.US_ASCII)
    }

    private fun canonicalInvitation(bytes: ByteArray): String? {
        if (bytes.isEmpty() ||
            bytes.size > MAX_INVITATION_BYTES ||
            !bytes.all(::isPrintableAscii)
        ) {
            return null
        }
        return String(bytes, StandardCharsets.US_ASCII).takeIf(::hasSupportedPrefix)
    }

    private fun hasSupportedPrefix(invitation: String): Boolean =
        prefixes.any { prefix ->
            invitation.length > prefix.length && invitation.startsWith(prefix)
        }

    private fun isPrintableAscii(byte: Byte): Boolean = (byte.toInt() and 0xff) in 0x21..0x7e

    private fun isBase64UrlCharacter(character: Char): Boolean =
        character in 'A'..'Z' ||
            character in 'a'..'z' ||
            character in '0'..'9' ||
            character == '-' ||
            character == '_'
}
