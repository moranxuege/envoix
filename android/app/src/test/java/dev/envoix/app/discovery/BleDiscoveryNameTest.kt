package dev.envoix.app.discovery

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BleDiscoveryNameTest {
    @Test
    fun `round trips a bounded app display name`() {
        val encoded = BleDiscoveryName.encodeServiceData(" Nearby-Xiaomi ")

        assertArrayEquals("Nearby-Xiaomi".toByteArray(), encoded)
        assertEquals("Nearby-Xiaomi", BleDiscoveryName.decode(encoded, localName = null))
    }

    @Test
    fun `truncates on a UTF-8 code point boundary`() {
        val encoded = BleDiscoveryName.encodeServiceData("设备-AlphaXYZ")

        assertEquals("设备-AlphaX", encoded?.toString(Charsets.UTF_8))
        assertEquals(BleDiscoveryName.MAX_SERVICE_DATA_BYTES, encoded?.size)

        val emoji = BleDiscoveryName.encodeServiceData("Phone📱📱📱")
        assertEquals("Phone📱📱", emoji?.toString(Charsets.UTF_8))
        assertEquals(13, emoji?.size)
    }

    @Test
    fun `falls back to an Apple local name`() {
        assertEquals(
            "Nearby Mac",
            BleDiscoveryName.decode(serviceData = null, localName = "  Nearby   Mac "),
        )
    }

    @Test
    fun `prefers app service data over a platform local name`() {
        assertEquals(
            "Envoix Mac",
            BleDiscoveryName.decode(
                serviceData = "Envoix Mac".toByteArray(),
                localName = "MacBook Pro",
            ),
        )
    }

    @Test
    fun `rejects blank control malformed and oversized metadata`() {
        assertNull(BleDiscoveryName.encodeServiceData(" \n\t "))
        assertNull(BleDiscoveryName.encodeServiceData("Nearby\u0000Phone"))
        assertNull(BleDiscoveryName.encodeServiceData(String(charArrayOf('\uD800'))))
        assertNull(BleDiscoveryName.decode(byteArrayOf(0xC3.toByte(), 0x28), localName = null))
        assertEquals(
            "Nearby Mac",
            BleDiscoveryName.decode(
                serviceData = byteArrayOf(0xC3.toByte(), 0x28),
                localName = "Nearby Mac",
            ),
        )
        assertNull(
            BleDiscoveryName.decode(
                ByteArray(BleDiscoveryName.MAX_SERVICE_DATA_BYTES + 1) { 'a'.code.toByte() },
                localName = null,
            ),
        )
    }
}
