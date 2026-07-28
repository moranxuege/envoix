package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pins the Kotlin [Status] enum against the canonical UI lifecycle strings.
 */
class StatusMappingTest {
    private val canonicalStates =
        listOf(
            "preparing",
            "waiting_for_peer",
            "pairing",
            "connecting",
            "awaiting_decision",
            "transferring",
            "verifying",
            "saving",
            "waiting_for_receiver_save",
            "finalizing_delivery",
            "paused",
            "delivered",
            "failed",
            "canceled",
        )

    @Test
    fun every_canonical_wire_string_maps_to_a_status() {
        for (wire in canonicalStates) {
            assertNotNull("'$wire' must map to a Status", Status.fromWire(wire))
        }
    }

    @Test
    fun every_status_round_trips_through_its_wire() {
        for (s in Status.entries) assertEquals(s, Status.fromWire(s.wire))
    }

    @Test
    fun unknown_wire_returns_null_not_a_default() {
        assertNull(Status.fromWire("bogus"))
        assertNull(Status.fromWire(""))
    }

    @Test
    fun kotlin_covers_exactly_the_canonical_states() {
        assertEquals(canonicalStates.toSet(), Status.entries.map { it.wire }.toSet())
    }
}
