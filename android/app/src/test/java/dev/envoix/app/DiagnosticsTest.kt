package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-logic tests for the report clipping. These lock in the two bugs the
 * bug-class hunt fixed: the byte-budget overshoot (the marker's own length must
 * be reserved, so the result never exceeds `maxBytes`) and head+tail retention
 * (the connection setup at the head survives, not just the failure at the tail).
 */
class DiagnosticsTest {
    private fun bytes(s: String) = s.toByteArray(Charsets.UTF_8).size

    @Test
    fun tailFitsBudgetAndKeepsTheEnd() {
        val text = "x".repeat(10_000) + "TAIL_MARKER"
        val out = Diagnostics.tail(text, 1000)
        assertTrue("result must fit the byte budget", bytes(out) <= 1000)
        assertTrue("keeps the tail", out.endsWith("TAIL_MARKER"))
        assertTrue("marks the clip", out.contains("trimmed"))
    }

    @Test
    fun tailReturnsInputWhenItFits() {
        val text = "short enough"
        assertEquals(text, Diagnostics.tail(text, 1000))
    }

    @Test
    fun headAndTailFitsBudgetAndKeepsBothEnds() {
        val text = "HEAD_MARKER" + "x".repeat(10_000) + "TAIL_MARKER"
        val out = Diagnostics.headAndTail(text, 1000)
        assertTrue("result must fit the byte budget", bytes(out) <= 1000)
        assertTrue("keeps the head (connection setup)", out.startsWith("HEAD_MARKER"))
        assertTrue("keeps the tail (the failure)", out.endsWith("TAIL_MARKER"))
        assertTrue("marks the clip", out.contains("trimmed"))
    }

    @Test
    fun headAndTailReturnsInputWhenItFits() {
        val text = "fits within budget"
        assertEquals(text, Diagnostics.headAndTail(text, 1000))
    }

    /** The byte-budget property at the caps actually used (all far larger than
     *  the marker): the clipped result never exceeds the cap. */
    @Test
    fun clippingFitsAtRealisticCaps() {
        val text = "y".repeat(100_000)
        for (cap in listOf(1024, 32 * 1024, 256 * 1024)) {
            assertTrue("tail cap=$cap fits", bytes(Diagnostics.tail(text, cap)) <= cap)
            assertTrue("headAndTail cap=$cap fits", bytes(Diagnostics.headAndTail(text, cap)) <= cap)
        }
    }
}
