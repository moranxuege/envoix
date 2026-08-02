package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NearbyRendezvousRoutingTest {
    @Test
    fun `secure local-network inbox is preferred when both carriers are visible`() {
        assertEquals(
            DiscoverySource.Mdns,
            preferredRendezvousSource(
                selection(
                    sources = setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns),
                    route = route(relayUrl = RELAY_URL),
                ),
            ),
        )
    }

    @Test
    fun `bluetooth is selected only when no secure inbox capability was advertised`() {
        assertEquals(
            DiscoverySource.Bluetooth,
            preferredRendezvousSource(
                selection(
                    sources = setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns),
                    route = null,
                ),
            ),
        )
    }

    @Test
    fun `transfer invites stay on the carrier that supports them`() {
        assertEquals(
            DiscoverySource.Bluetooth,
            preferredRendezvousSource(
                selection(
                    sources = setOf(DiscoverySource.Bluetooth, DiscoverySource.Mdns),
                    route = route(directAddresses = listOf(DIRECT_ADDRESS)),
                ),
                roomInvitation = false,
            ),
        )
    }

    @Test
    fun `mdns presence without a valid inbox endpoint is discovery only`() {
        assertNull(
            preferredRendezvousSource(
                selection(
                    sources = setOf(DiscoverySource.Mdns),
                    route = null,
                ),
            ),
        )
    }

    @Test
    fun `room action is available only when an invitation carrier can deliver`() {
        assertFalse(
            canOfferNearbyRoom(
                selection(sources = setOf(DiscoverySource.Mdns), route = null),
            ),
        )
        assertTrue(
            canOfferNearbyRoom(
                selection(sources = setOf(DiscoverySource.Bluetooth), route = null),
            ),
        )
        assertTrue(
            canOfferNearbyRoom(
                selection(sources = setOf(DiscoverySource.Mdns), route = route(relayUrl = RELAY_URL)),
            ),
        )
    }

    @Test
    fun `endpoint ids are normalized before native routing`() {
        assertEquals(
            ENDPOINT_ID,
            normalizeNearbyInboxEndpointId("  ${ENDPOINT_ID.uppercase()}  "),
        )
        assertNull(normalizeNearbyInboxEndpointId(ENDPOINT_ID.dropLast(1)))
        assertNull(normalizeNearbyInboxEndpointId("1" + ENDPOINT_ID.drop(1)))
    }

    @Test
    fun `endpoint without relay or direct address is discovery only`() {
        assertNull(
            NearbyInviteRoute.normalized(
                endpointId = ENDPOINT_ID,
                relayUrl = null,
                directAddresses = emptyList(),
            ),
        )
    }

    @Test
    fun `bonjour TXT round trip preserves the exact bounded route`() {
        val route =
            route(
                relayUrl = RELAY_URL,
                directAddresses =
                    listOf(
                        DIRECT_ADDRESS,
                        "[2001:db8::1]:8443",
                    ),
            )

        val attributes = nearbyInviteTxtAttributes(route)
        val parsed = parseNearbyInviteTxtAttributes(attributes::get)

        assertEquals(
            mapOf(
                "ibox" to ENDPOINT_ID,
                "irelay" to RELAY_URL,
                "iaddr0" to DIRECT_ADDRESS,
                "iaddr1" to "[2001:db8::1]:8443",
            ),
            attributes,
        )
        assertEquals(route, parsed)
    }

    @Test
    fun `bonjour route rejects whitespace and caps addresses at four`() {
        val parsed =
            NearbyInviteRoute.normalized(
                endpointId = ENDPOINT_ID,
                relayUrl = "https://relay .example",
                directAddresses =
                    listOf(
                        "192.0.2.1:1",
                        "192.0.2.2:2",
                        "192.0.2.3:3",
                        "192.0.2.4:4",
                        "192.0.2.5:5",
                        "192.0.2.1:1",
                    ),
            )

        assertEquals(null, parsed?.relayUrl)
        assertEquals(
            listOf(
                "192.0.2.1:1",
                "192.0.2.2:2",
                "192.0.2.3:3",
                "192.0.2.4:4",
            ),
            parsed?.directAddresses,
        )
    }

    private fun selection(
        sources: Set<DiscoverySource>,
        route: NearbyInviteRoute?,
    ) = NearbyPairingSelection(
        discoveryPeerKey = "0011223344556677",
        displayName = "Nearby phone",
        sources = sources,
        nearbyInviteRoute = route,
    )

    private fun route(
        relayUrl: String? = null,
        directAddresses: List<String> = emptyList(),
    ) = NearbyInviteRoute.normalized(
        endpointId = ENDPOINT_ID,
        relayUrl = relayUrl,
        directAddresses = directAddresses,
    )!!

    private companion object {
        const val ENDPOINT_ID = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst"
        const val RELAY_URL = "https://relay.example"
        const val DIRECT_ADDRESS = "192.0.2.44:8443"
    }
}
