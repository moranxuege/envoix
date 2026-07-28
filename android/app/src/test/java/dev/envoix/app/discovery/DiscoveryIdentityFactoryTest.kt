package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class DiscoveryIdentityFactoryTest {
    @Test
    fun `creates a fresh normalized presence identity for each discovery session`() {
        val first =
            DiscoveryIdentityFactory.create("  Android   phone  ") { bytes ->
                bytes.fill(0x11)
            }
        val second =
            DiscoveryIdentityFactory.create("Android phone") { bytes ->
                bytes.fill(0x22)
            }

        assertEquals("1111111111111111", first.peerKey)
        assertEquals("Android phone", first.displayName)
        assertEquals("2222222222222222", second.peerKey)
        assertNotEquals(first.peerKey, second.peerKey)
    }
}
