package dev.envoix.app.discovery

import java.security.SecureRandom
import java.util.UUID

/**
 * Secret-free hint that an Android presenter has armed its short NFC lease.
 *
 * The random low 64 bits are only a replay/deduplication handle. They are not
 * an invitation, identity, or authorization token.
 */
internal object NfcReadinessUuid {
    val FILTER_BASE_UUID: UUID = UUID.fromString("d5f3a2d8-8f4a-4b34-0000-000000000000")
    val FILTER_MASK_UUID: UUID = UUID.fromString("ffffffff-ffff-ffff-0000-000000000000")

    private val random = SecureRandom()

    fun newOfferId(): String {
        var value: Long
        do {
            value = random.nextLong()
        } while (value == 0L)
        return format(value)
    }

    fun encode(offerId: String): UUID? {
        val normalized = normalizeOfferId(offerId) ?: return null
        val leastSignificantBits =
            runCatching { java.lang.Long.parseUnsignedLong(normalized, 16) }
                .getOrNull()
                ?: return null
        return UUID(FILTER_BASE_UUID.mostSignificantBits, leastSignificantBits)
    }

    fun decode(uuid: UUID?): String? {
        if (uuid == null ||
            uuid.mostSignificantBits != FILTER_BASE_UUID.mostSignificantBits ||
            uuid.leastSignificantBits == 0L
        ) {
            return null
        }
        return format(uuid.leastSignificantBits)
    }

    fun normalizeOfferId(value: String?): String? =
        value
            ?.takeIf { OFFER_ID.matches(it) && it != ZERO_OFFER_ID }

    private fun format(value: Long): String = "%016x".format(value)

    private const val ZERO_OFFER_ID = "0000000000000000"
    private val OFFER_ID = Regex("^[0-9a-f]{16}$")
}
