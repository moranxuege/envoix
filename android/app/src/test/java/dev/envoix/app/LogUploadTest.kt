package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LogUploadTest {
    @Test
    fun `diagnostic upload accepts only https endpoints`() {
        assertNull(LogUpload.uploadUrl("http://logs.example", "room1", "send"))
        assertNull(LogUpload.uploadUrl("not a URL", "room1", "send"))

        assertEquals(
            "https://logs.example/logs/room1?side=send",
            LogUpload.uploadUrl("https://logs.example", "room1", "send")?.toString(),
        )
    }

    @Test
    fun `diagnostic upload validates correlation fields`() {
        assertNull(LogUpload.uploadUrl("https://logs.example", "room/secret", "send"))
        assertNull(LogUpload.uploadUrl("https://logs.example", "room1", "send&admin=true"))
        assertNull(LogUpload.uploadUrl("https://logs.example", "r".repeat(65), "send"))
    }

    @Test
    fun `diagnostic upload requires a bounded bearer token`() {
        assertNull(LogUpload.bearerHeader(null))
        assertNull(LogUpload.bearerHeader("  "))
        assertNull(LogUpload.bearerHeader("token\nforged"))
        assertNull(LogUpload.bearerHeader("x".repeat(1025)))
        assertEquals("Bearer upload-token", LogUpload.bearerHeader(" upload-token "))
    }

    @Test
    fun `diagnostic upload body uses the shared byte limit`() {
        assertTrue(LogUpload.boundedBody("x".repeat(LogUpload.BODY_MAX_BYTES)) != null)
        assertNull(LogUpload.boundedBody("x".repeat(LogUpload.BODY_MAX_BYTES + 1)))
        assertNull(LogUpload.boundedBody("界".repeat(LogUpload.BODY_MAX_BYTES / 2)))
    }
}
