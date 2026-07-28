package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DiscoveryPeerRegistryTest {
    @Test
    fun `merges BLE and mDNS observations for the same peer`() {
        val registry = DiscoveryPeerRegistry(observationTtlMs = 10_000)

        assertTrue(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = "0011223344556677",
                    source = DiscoverySource.Bluetooth,
                    seenAtMs = 1_000,
                    rssi = -42,
                ),
            ),
        )
        assertTrue(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = "0011223344556677",
                    source = DiscoverySource.Mdns,
                    seenAtMs = 1_200,
                    displayName = "  Test   Phone  ",
                    nearbyInviteRoute = route(directAddresses = listOf("192.0.2.1:45454")),
                ),
            ),
        )

        val peer = registry.peers(nowMs = 1_500).single()
        assertEquals("Test Phone", peer.displayName)
        assertEquals(setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns), peer.sources)
        assertEquals(-42, peer.rssi)
        assertEquals(route(directAddresses = listOf("192.0.2.1:45454")), peer.nearbyInviteRoute)
        assertEquals(1_200, peer.lastSeenAtMs)
    }

    @Test
    fun `rejects invalid peer keys and timestamps`() {
        val registry = DiscoveryPeerRegistry()

        assertFalse(
            registry.upsert(
                DiscoveryObservation("not-a-key", DiscoverySource.Bluetooth, seenAtMs = 1),
            ),
        )
        assertFalse(
            registry.upsert(
                DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = -1),
            ),
        )
        assertTrue(registry.peers(nowMs = 1).isEmpty())
    }

    @Test
    fun `expires each transport independently after the ttl`() {
        val registry = DiscoveryPeerRegistry(observationTtlMs = 1_000)
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 0, rssi = -60),
        )
        registry.upsert(
            DiscoveryObservation(
                "0011223344556677",
                DiscoverySource.Mdns,
                seenAtMs = 500,
                nearbyInviteRoute = route(directAddresses = listOf("192.0.2.2:1234")),
            ),
        )

        val boundary = registry.peers(nowMs = 1_000).single()
        assertEquals(setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns), boundary.sources)

        val mdnsOnly = registry.peers(nowMs = 1_001).single()
        assertEquals(setOf(DiscoverySource.Mdns), mdnsOnly.sources)
        assertNull(mdnsOnly.rssi)
        assertTrue(registry.peers(nowMs = 1_501).isEmpty())
    }

    @Test
    fun `keeps a peer through source loss and merges the source when it returns`() {
        val registry = DiscoveryPeerRegistry(observationTtlMs = 1_000)
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 0, rssi = -60),
        )
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Mdns, seenAtMs = 500),
        )

        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Mdns, seenAtMs = 1_001),
        )
        val mdnsOnly = registry.peers(nowMs = 1_001).single()
        assertEquals(setOf(DiscoverySource.Mdns), mdnsOnly.sources)

        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 1_002, rssi = -48),
        )
        val mergedAgain = registry.peers(nowMs = 1_002).single()
        assertEquals(setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns), mergedAgain.sources)
        assertEquals(-48, mergedAgain.rssi)
    }

    @Test
    fun `normalizes key and caps display name`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation(
                " AABBCCDDEEFF0011 ",
                DiscoverySource.Mdns,
                seenAtMs = 1,
                displayName = "x".repeat(DiscoveryPeerRegistry.MAX_DISPLAY_NAME_LENGTH + 20),
            ),
        )

        val peer = registry.peers(nowMs = 1).single()
        assertEquals("aabbccddeeff0011", peer.peerKey)
        assertEquals(DiscoveryPeerRegistry.MAX_DISPLAY_NAME_LENGTH, peer.displayName?.length)
    }

    @Test
    fun `does not let an out of order callback replace newer transport data`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 2_000, rssi = -40),
        )

        assertFalse(
            registry.upsert(
                DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 1_000, rssi = -90),
            ),
        )
        val peer = registry.peers(nowMs = 2_000).single()
        assertEquals(-40, peer.rssi)
        assertEquals(2_000, peer.lastSeenAtMs)
    }

    @Test
    fun `clear removes observations from the previous presence session`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 1),
        )

        registry.clear()

        assertTrue(registry.peers(nowMs = 1).isEmpty())
    }

    @Test
    fun `pairing selection binds delivery to the endpoint shown to the user`() {
        val selection =
            NearbyPairingSelection.from(
                DiscoveredPeer(
                    peerKey = "0011223344556677",
                    displayName = "Nearby phone",
                    sources = setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns),
                    lastSeenAtMs = 42,
                    rssi = -36,
                    nearbyInviteRoute =
                        route(
                            relayUrl = "https://relay.example",
                            directAddresses = listOf("192.0.2.4:443"),
                        ),
                ),
            )

        assertEquals("0011223344556677", selection.discoveryPeerKey)
        assertEquals("Nearby phone", selection.displayName)
        assertEquals(setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns), selection.sources)
        assertEquals(
            "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst",
            selection.nearbyInviteRoute?.endpointId,
        )
        assertEquals("https://relay.example", selection.nearbyInviteRoute?.relayUrl)
        assertEquals(listOf("192.0.2.4:443"), selection.nearbyInviteRoute?.directAddresses)
    }

    @Test
    fun `registry and pairing selection freeze all nearby route fields`() {
        val addresses = mutableListOf("192.0.2.8:8443")
        val discoveredRoute =
            NearbyInviteRoute.normalized(
                endpointId = ENDPOINT_ID,
                relayUrl = "https://relay.example",
                directAddresses = addresses,
            )!!
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation(
                peerKey = "0011223344556677",
                source = DiscoverySource.Mdns,
                seenAtMs = 1,
                nearbyInviteRoute = discoveredRoute,
            ),
        )

        addresses += "198.51.100.9:8443"
        val peer = registry.peers(nowMs = 1).single()
        val selection = NearbyPairingSelection.from(peer)

        assertEquals(listOf("192.0.2.8:8443"), peer.nearbyInviteRoute?.directAddresses)
        assertEquals(peer.nearbyInviteRoute, selection.nearbyInviteRoute)
    }

    private fun route(
        relayUrl: String? = null,
        directAddresses: List<String> = emptyList(),
    ): NearbyInviteRoute =
        NearbyInviteRoute.normalized(
            endpointId = ENDPOINT_ID,
            relayUrl = relayUrl,
            directAddresses = directAddresses,
        )!!

    private companion object {
        const val ENDPOINT_ID = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst"
    }
}
