package dev.envoix.app.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RoomControlInviteFormatTest {
    @Test
    fun `accepts canonical room control inputs`() {
        val supported =
            listOf(
                "123456-a1b2-c3d4",
                "123456A1B2C3D4",
                "123456-A1B2-C3D4",
                " envoix://room/123456-a1b2-c3d4?broker=example.test ",
                "envoix://room/123456-A1B2-C3D4",
            )

        supported.forEach { assertTrue(RoomControlInviteFormat.looksLikeRoomInvite(it)) }
    }

    @Test
    fun `rejects retired and malformed room control inputs`() {
        val unsupported =
            listOf(
                "R123456-a1b2-c3d4",
                "r123456-a1b2-c3d4",
                "envoix://room/R123456-a1b2-c3d4",
                "envoix://room/123456-alpha-bravo",
                "envoix://pair/123456-a1b2-c3d4",
                "envoix://invite/v2/test-payload",
                "123456-a1b2c3d4",
                "123456a1b2-c3d4",
            )

        unsupported.forEach { assertFalse(RoomControlInviteFormat.looksLikeRoomInvite(it)) }
    }
}
