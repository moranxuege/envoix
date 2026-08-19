package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class ConnectionPathPresentationTest {
    @Test
    fun `classifies structured and legacy paths without presenting endpoints`() {
        val cases =
            mapOf(
                "direct" to ConnectionPathKind.Direct,
                "direct (198.51.100.42:4242)" to ConnectionPathKind.Direct,
                "direct_ipv4" to ConnectionPathKind.DirectIpv4,
                "direct_ipv6" to ConnectionPathKind.DirectIpv6,
                "relay" to ConnectionPathKind.Relay,
                "relay (https://private-relay.example)" to ConnectionPathKind.Relay,
                "wifi_aware" to ConnectionPathKind.WifiAware,
                "mdns" to ConnectionPathKind.Other,
                "custom transport details" to ConnectionPathKind.Other,
            )

        cases.forEach { (raw, kind) ->
            assertEquals(kind, ConnectionPathKind.fromWireOrLegacy(raw))
            assertFalse(kind.wire.contains("198.51.100.42"))
            assertFalse(kind.wire.contains("private-relay.example"))
            assertFalse(kind.wire.contains("custom transport details"))
        }
        assertNull(ConnectionPathKind.fromWireOrLegacy(null))
        assertNull(ConnectionPathKind.fromWireOrLegacy(""))
    }
}
