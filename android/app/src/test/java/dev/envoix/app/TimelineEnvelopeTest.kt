package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pins the Kotlin timeline builder against the Rust one. The golden line below
 * MUST be byte-identical to the Rust `golden_line_matches_the_kotlin_builder`
 * test (crates/envoix-ffi/src/logging_tests.rs) — same inputs, same column order, same
 * escaping — so a Rust line and a Kotlin line are indistinguishable to a reader.
 */
class TimelineEnvelopeTest {
    // Same fixed inputs as the Rust golden test.
    private val golden =
        "1\t1720000000000\t42\t7\t1\tsender\tmachine\ttransition\tok\tcause=a%25b%09c%0Ad"

    @Test
    fun golden_line_matches_the_rust_builder() {
        val line =
            TransferTimeline.buildLine(
                schema = 1,
                epochMs = 1_720_000_000_000L,
                pid = 42,
                id = 7L,
                attempt = "1",
                side = "sender",
                layer = "machine",
                event = "transition",
                outcome = "ok",
                fields = linkedMapOf("cause" to "a%b\tc\nd"),
            )
        assertEquals(golden, line)
    }

    @Test
    fun typed_sink_rejects_session_ids_outside_the_android_record_range() {
        assertEquals(0L, timelineSessionId(0uL))
        assertEquals(Long.MAX_VALUE, timelineSessionId(Long.MAX_VALUE.toULong()))
        assertEquals(null, timelineSessionId(Long.MAX_VALUE.toULong() + 1uL))
        assertEquals(null, timelineSessionId(ULong.MAX_VALUE))
    }
}
