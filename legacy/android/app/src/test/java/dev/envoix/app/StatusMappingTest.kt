package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pins the Kotlin [Status] enum against the core `State` wire strings. Mirrors
 * the Rust `every_state_serializes_to_its_wire_string` test — the two string
 * lists must stay identical, or a snapshot state fails to map and (previously)
 * silently froze the card.
 */
class StatusMappingTest {
    // The 11 wire strings the core `State` serializes to (serde snake_case).
    private val coreStates =
        listOf(
            "preparing",
            "waiting",
            "connecting",
            "verifying",
            "transferring",
            "confirming",
            "paused",
            "unconfirmed",
            "completed",
            "failed",
            "cancelled",
        )

    @Test
    fun every_core_wire_string_maps_to_a_status() {
        for (w in coreStates) assertNotNull("'$w' must map to a Status", Status.fromWire(w))
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
    fun kotlin_covers_exactly_the_core_states() {
        assertEquals(coreStates.toSet(), Status.entries.map { it.wire }.toSet())
    }
}
