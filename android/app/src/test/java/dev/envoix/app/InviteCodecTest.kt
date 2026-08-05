package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Test

class InviteCodecTest {
    @Test
    fun `formats compact and partial room code input`() {
        assertEquals("123456-a1b2-c3d4", InviteCodec.formatRoomCode("123456A1B2C3D4"))
        assertEquals("123456-a", InviteCodec.formatRoomCode("123456A"))
        assertEquals("123456-a1b2-c", InviteCodec.formatRoomCode("123456-A1B2C"))
        assertEquals("123456-a1b2-", InviteCodec.formatRoomCode("123456-A1B2-"))
    }

    @Test
    fun `keeps canonical room code unchanged`() {
        val canonical = "123456-a1b2-c3d4"

        assertEquals(canonical, InviteCodec.formatRoomCode(canonical))
    }

    @Test
    fun `does not truncate or repair malformed room code input`() {
        val malformedInputs =
            listOf(
                "envoix://invite/v2/value",
                "123456a1b2c3d4x",
                "123456-a1b2-c3d4-extra",
                "123456--a1b2-c3d4",
                "12345-6a1b2c3d4",
                "123456-a1b2c3d4",
                "123456a1b2-c3d4",
                "123456-a1b2-c3dé",
                " 123456-a1b2-c3d4",
                "123456-a1b2-c3d4 ",
            )

        for (input in malformedInputs) {
            assertEquals(input, InviteCodec.formatRoomCode(input))
        }
    }
}
