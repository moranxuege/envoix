package dev.envoix.app

import dev.envoix.app.ui.AppText
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
                "relay" to ConnectionPathKind.Relay,
                "relay (https://private-relay.example)" to ConnectionPathKind.Relay,
                "custom transport details" to ConnectionPathKind.Other,
            )

        cases.forEach { (raw, kind) ->
            assertEquals(kind, ConnectionPathKind.fromWireOrLegacy(raw))
            val label = connectionPathLabel(raw, AppText.ENGLISH).orEmpty()
            assertFalse(label.contains("198.51.100.42"))
            assertFalse(label.contains("private-relay.example"))
            assertFalse(label.contains("custom transport details"))
        }
        assertNull(ConnectionPathKind.fromWireOrLegacy(null))
        assertNull(connectionPathLabel("", AppText.ENGLISH))
    }
}
