package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class NfcReadinessUuidTest {
    @Test
    fun `readiness UUID round trips only a nonzero random offer id`() {
        val offerId = "0123456789abcdef"
        val encoded = requireNotNull(NfcReadinessUuid.encode(offerId))

        assertEquals(NfcReadinessUuid.FILTER_BASE_UUID.mostSignificantBits, encoded.mostSignificantBits)
        assertEquals(offerId, NfcReadinessUuid.decode(encoded))
        assertNotEquals(BleDiscoveryUuid.FILTER_BASE_UUID.mostSignificantBits, encoded.mostSignificantBits)
    }

    @Test
    fun `readiness UUID rejects malformed and foreign values`() {
        listOf(
            null,
            "",
            "0000000000000000",
            "1234",
            "0123456789abcdeg",
            " 0123456789abcdef",
        ).forEach { value ->
            assertNull(NfcReadinessUuid.encode(value ?: ""))
        }
        assertNull(NfcReadinessUuid.decode(UUID.fromString("d5f3a2d8-8f4a-4b33-0123-456789abcdef")))
        assertNull(NfcReadinessUuid.decode(NfcReadinessUuid.FILTER_BASE_UUID))
    }

    @Test
    fun `generated offer ids are canonical nonzero values`() {
        val ids = List(32) { NfcReadinessUuid.newOfferId() }

        assertEquals(ids.size, ids.toSet().size)
        assertTrue(ids.all { NfcReadinessUuid.normalizeOfferId(it) == it })
    }
}
