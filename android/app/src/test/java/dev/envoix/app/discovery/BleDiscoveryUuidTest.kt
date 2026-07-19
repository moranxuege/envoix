package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.util.UUID

class BleDiscoveryUuidTest {
    @Test
    fun `round trips a peer key`() {
        val encoded = BleDiscoveryUuid.encode("0011223344556677")

        assertEquals(UUID.fromString("d5f3a2d8-8f4a-4b33-0011-223344556677"), encoded)
        assertEquals("0011223344556677", BleDiscoveryUuid.decode(encoded))
    }

    @Test
    fun `round trips a key whose unsigned value exceeds Long max`() {
        val encoded = BleDiscoveryUuid.encode("ffeeddccbbaa9988")

        assertEquals("ffeeddccbbaa9988", BleDiscoveryUuid.decode(encoded))
    }

    @Test
    fun `rejects malformed keys and unrelated service UUIDs`() {
        assertNull(BleDiscoveryUuid.encode("short"))
        assertNull(BleDiscoveryUuid.decode(null))
        assertNull(BleDiscoveryUuid.decode(UUID.fromString("00000000-0000-0000-0011-223344556677")))
    }
}
