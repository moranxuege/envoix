package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-logic tests for the converging-publish helpers. The collision retry itself
 * and the positive `isUniqueViolation` case need a real MediaStore /
 * `SQLiteConstraintException` (not constructible in a plain JVM test), so those
 * are covered by the on-emulator thrice-receive verification. Here we lock the
 * candidate sequence and — importantly — that a plain "UNIQUE" message is NOT
 * mistaken for a name collision.
 */
class MediaStoreSaverTest {
    @Test
    fun nameSequenceStartsWithTheNameThenBumps() {
        assertEquals(
            listOf("photo.jpg", "photo (1).jpg", "photo (2).jpg"),
            MediaStoreSaver.nameSequence("photo.jpg").take(3).toList(),
        )
    }

    @Test
    fun nameSequenceHandlesNoExtension() {
        assertEquals(
            listOf("report", "report (1)", "report (2)"),
            MediaStoreSaver.nameSequence("report").take(3).toList(),
        )
    }

    @Test
    fun nameSequenceKeepsLeadingDotAsPartOfTheName() {
        // ".gitignore" has no extension to split (dot at index 0), so it bumps whole.
        assertEquals(
            listOf(".gitignore", ".gitignore (1)"),
            MediaStoreSaver.nameSequence(".gitignore").take(2).toList(),
        )
    }

    @Test
    fun nameSequenceHasNinetyNineNumberedThenAWellFormedRandomTail() {
        val first100 = MediaStoreSaver.nameSequence("f.bin").take(100).toList()
        assertEquals("f.bin", first100[0])
        assertEquals("f (1).bin", first100[1])
        assertEquals("f (99).bin", first100[99])
        // Element 101 is the first random-suffixed candidate: still well-formed,
        // extension preserved, and distinct from every numbered candidate.
        val tail = MediaStoreSaver.nameSequence("f.bin").take(101).last()
        assertTrue("keeps the base", tail.startsWith("f ("))
        assertTrue("keeps the extension", tail.endsWith(").bin"))
        assertFalse("differs from the numbered candidates", first100.contains(tail))
    }

    @Test
    fun isUniqueViolationRejectsNullAndNonSqliteUniqueMessages() {
        assertFalse(MediaStoreSaver.isUniqueViolation(null))
        // A plain exception that merely mentions UNIQUE must NOT be treated as a
        // MediaStore name collision (would loop on an unrelated fault).
        assertFalse(MediaStoreSaver.isUniqueViolation(RuntimeException("UNIQUE constraint failed: files._data")))
        assertFalse(
            MediaStoreSaver.isUniqueViolation(
                IllegalStateException("wrap", RuntimeException("UNIQUE")),
            ),
        )
    }
}
