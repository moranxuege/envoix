package dev.envoix.app.ui

import dev.envoix.app.discovery.DiscoveredPeer
import dev.envoix.app.discovery.DiscoverySource
import org.junit.Assert.assertEquals
import org.junit.Test

class NearbyDevicePresentationTest {
    @Test
    fun `duplicate names gain a stable short identity while unique names stay clean`() {
        val first = peer("0011223344556677", "Phone")
        val second = peer("8899aabbccddeeff", " phone ")
        val unique = peer("0123456789abcdef", "Tablet")
        val peers = listOf(first, second, unique)

        assertEquals("Phone · 6677", nearbyPeerDisplayName(first, peers, "Nearby Envoix device"))
        assertEquals("phone · EEFF", nearbyPeerDisplayName(second, peers, "Nearby Envoix device"))
        assertEquals("Tablet", nearbyPeerDisplayName(unique, peers, "Nearby Envoix device"))
    }

    @Test
    fun `unnamed peers are also disambiguated`() {
        val first = peer("0011223344556677", null)
        val second = peer("8899aabbccddeeff", " ")
        val peers = listOf(first, second)

        assertEquals(
            "Nearby Envoix device · 6677",
            nearbyPeerDisplayName(first, peers, "Nearby Envoix device"),
        )
        assertEquals(
            "Nearby Envoix device · EEFF",
            nearbyPeerDisplayName(second, peers, "Nearby Envoix device"),
        )
    }

    @Test
    fun `device card lists only observed discovery sources in stable order`() {
        assertEquals(
            "BLE · Local network",
            nearbyDiscoverySourceLabel(
                setOf(DiscoverySource.Mdns, DiscoverySource.Bluetooth),
                AppText.ENGLISH,
            ),
        )
        assertEquals(
            "Wi-Fi Aware",
            nearbyDiscoverySourceLabel(
                setOf(DiscoverySource.WifiAware),
                AppText.ENGLISH,
            ),
        )
        assertEquals(
            "附近",
            nearbyDiscoverySourceLabel(emptySet(), AppText.SIMPLIFIED_CHINESE),
        )
    }

    private fun peer(
        key: String,
        name: String?,
    ): DiscoveredPeer =
        DiscoveredPeer(
            peerKey = key,
            displayName = name,
            sources = setOf(DiscoverySource.Bluetooth),
            lastSeenAtMs = 0,
            rssi = null,
            nearbyInviteRoute = null,
        )
}
