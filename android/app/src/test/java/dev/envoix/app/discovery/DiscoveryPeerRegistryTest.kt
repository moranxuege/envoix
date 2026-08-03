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
    fun `out of order callback cannot replace a newer name or route`() {
        val registry = DiscoveryPeerRegistry()
        val currentRoute = route(directAddresses = listOf("192.0.2.10:8443"))
        registry.upsert(
            DiscoveryObservation(
                peerKey = "0011223344556677",
                source = DiscoverySource.Mdns,
                seenAtMs = 2_000,
                displayName = "Current phone",
                nearbyInviteRoute = currentRoute,
            ),
        )

        assertFalse(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = "0011223344556677",
                    source = DiscoverySource.Mdns,
                    seenAtMs = 1_000,
                    displayName = "Stale phone",
                    nearbyInviteRoute = route(directAddresses = listOf("192.0.2.99:8443")),
                ),
            ),
        )

        val peer = registry.peers(nowMs = 2_000).single()
        assertEquals("Current phone", peer.displayName)
        assertEquals(currentRoute, peer.nearbyInviteRoute)
        assertEquals(2_000, peer.lastSeenAtMs)
    }

    @Test
    fun `blank name refresh keeps the last known name from that source`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation(
                peerKey = "0011223344556677",
                source = DiscoverySource.Mdns,
                seenAtMs = 1_000,
                displayName = "Nearby phone",
            ),
        )

        assertTrue(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = "0011223344556677",
                    source = DiscoverySource.Mdns,
                    seenAtMs = 2_000,
                    displayName = "   ",
                ),
            ),
        )

        val peer = registry.peers(nowMs = 2_000).single()
        assertEquals("Nearby phone", peer.displayName)
        assertEquals(2_000, peer.lastSeenAtMs)
    }

    @Test
    fun `peers keep first seen order when names and last seen change`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Mdns,
                seenAtMs = 1_000,
                displayName = "beta",
            ),
        )
        registry.upsert(
            DiscoveryObservation(
                peerKey = "2222222222222222",
                source = DiscoverySource.Mdns,
                seenAtMs = 2_000,
                displayName = "Alpha",
            ),
        )

        assertEquals(
            listOf("1111111111111111", "2222222222222222"),
            registry.peers(nowMs = 2_000).map(DiscoveredPeer::peerKey),
        )

        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 3_000,
                displayName = "Zulu",
            ),
        )

        val updatedPeers = registry.peers(nowMs = 3_000)
        assertEquals(
            listOf("1111111111111111", "2222222222222222"),
            updatedPeers.map(DiscoveredPeer::peerKey),
        )
        assertEquals("beta", updatedPeers.first().displayName)
        assertEquals(
            setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns),
            updatedPeers.first().sources,
        )
    }

    @Test
    fun `BLE name is used until a complete mDNS name is available`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 1_000,
                displayName = "Nearby Xi",
            ),
        )

        assertEquals("Nearby Xi", registry.peers(nowMs = 1_000).single().displayName)

        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Mdns,
                seenAtMs = 2_000,
                displayName = "Nearby Xiaomi Phone",
            ),
        )
        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 3_000,
                displayName = "Nearby Xi",
            ),
        )

        assertEquals("Nearby Xiaomi Phone", registry.peers(nowMs = 3_000).single().displayName)
    }

    @Test
    fun `duplicate and missing names remain independent in first seen order`() {
        val registry = DiscoveryPeerRegistry()
        listOf(
            DiscoveryObservation(
                peerKey = "4444444444444444",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 1_000,
            ),
            DiscoveryObservation(
                peerKey = "3333333333333333",
                source = DiscoverySource.Mdns,
                seenAtMs = 4_000,
                displayName = "Phone",
            ),
            DiscoveryObservation(
                peerKey = "2222222222222222",
                source = DiscoverySource.Mdns,
                seenAtMs = 3_000,
                displayName = "phone",
            ),
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 2_000,
            ),
        ).forEach(registry::upsert)

        val peers = registry.peers(nowMs = 4_000)

        assertEquals(4, peers.size)
        assertEquals(
            listOf(
                "4444444444444444",
                "3333333333333333",
                "2222222222222222",
                "1111111111111111",
            ),
            peers.map(DiscoveredPeer::peerKey),
        )
        assertEquals(listOf(null, "Phone", "phone", null), peers.map(DiscoveredPeer::displayName))
    }

    @Test
    fun `peer removed by ttl is appended when it reappears`() {
        val registry = DiscoveryPeerRegistry(observationTtlMs = 1_000)
        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 0,
            ),
        )
        registry.upsert(
            DiscoveryObservation(
                peerKey = "2222222222222222",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 500,
            ),
        )

        assertEquals(
            listOf("2222222222222222"),
            registry.peers(nowMs = 1_001).map(DiscoveredPeer::peerKey),
        )

        registry.upsert(
            DiscoveryObservation(
                peerKey = "1111111111111111",
                source = DiscoverySource.Bluetooth,
                seenAtMs = 1_002,
            ),
        )

        assertEquals(
            listOf("2222222222222222", "1111111111111111"),
            registry.peers(nowMs = 1_002).map(DiscoveredPeer::peerKey),
        )
    }

    @Test
    fun `rejects a new peer at capacity but still updates an existing peer`() {
        val registry = DiscoveryPeerRegistry()
        repeat(DiscoveryPeerRegistry.MAX_PEERS) { index ->
            assertTrue(
                registry.upsert(
                    DiscoveryObservation(
                        peerKey = peerKey(index),
                        source = DiscoverySource.Bluetooth,
                        seenAtMs = 1_000,
                    ),
                ),
            )
        }

        assertFalse(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = peerKey(DiscoveryPeerRegistry.MAX_PEERS),
                    source = DiscoverySource.Bluetooth,
                    seenAtMs = 1_001,
                ),
            ),
        )
        assertTrue(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = peerKey(0),
                    source = DiscoverySource.Bluetooth,
                    seenAtMs = 1_002,
                    rssi = -31,
                ),
            ),
        )

        val peers = registry.peers(nowMs = 1_002)
        assertEquals(DiscoveryPeerRegistry.MAX_PEERS, peers.size)
        assertEquals(-31, peers.first { it.peerKey == peerKey(0) }.rssi)
    }

    @Test
    fun `accepts a new peer after peers removes expired capacity`() {
        val registry = DiscoveryPeerRegistry(observationTtlMs = 1_000)
        repeat(DiscoveryPeerRegistry.MAX_PEERS) { index ->
            assertTrue(
                registry.upsert(
                    DiscoveryObservation(
                        peerKey = peerKey(index),
                        source = DiscoverySource.Bluetooth,
                        seenAtMs = 0,
                    ),
                ),
            )
        }

        assertTrue(registry.peers(nowMs = 1_001).isEmpty())
        assertTrue(
            registry.upsert(
                DiscoveryObservation(
                    peerKey = peerKey(DiscoveryPeerRegistry.MAX_PEERS),
                    source = DiscoverySource.Bluetooth,
                    seenAtMs = 1_001,
                ),
            ),
        )
        assertEquals(
            peerKey(DiscoveryPeerRegistry.MAX_PEERS),
            registry.peers(nowMs = 1_001).single().peerKey,
        )
    }

    @Test
    fun `clear removes observations from the previous presence session`() {
        val registry = DiscoveryPeerRegistry()
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 1),
        )

        registry.clear()

        assertTrue(registry.peers(nowMs = 1).isEmpty())

        registry.upsert(
            DiscoveryObservation("8899aabbccddeeff", DiscoverySource.Bluetooth, seenAtMs = 2),
        )
        registry.upsert(
            DiscoveryObservation("0011223344556677", DiscoverySource.Bluetooth, seenAtMs = 2),
        )
        assertEquals(
            listOf("8899aabbccddeeff", "0011223344556677"),
            registry.peers(nowMs = 2).map(DiscoveredPeer::peerKey),
        )
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

    private fun peerKey(index: Int): String = index.toString(16).padStart(DiscoveryPeerRegistry.PEER_KEY_HEX_LENGTH, '0')

    private companion object {
        const val ENDPOINT_ID = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst"
    }
}
