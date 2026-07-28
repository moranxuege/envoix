package dev.envoix.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class NativeRoomControlGatewayTest {
    private val filesDirectory = File("/data/user/0/dev.envoix.app/files")

    @Test
    fun `one-time room keeps the legacy transport identity`() {
        assertEquals(
            File(filesDirectory, "room-control/identity.json").absolutePath,
            roomControlIdentityPath(filesDirectory, rememberedRelationshipId = null),
        )
    }

    @Test
    fun `remembered rooms receive stable distinct transport identities`() {
        val first = roomControlIdentityPath(filesDirectory, "relationship-a")
        val firstAgain = roomControlIdentityPath(filesDirectory, "relationship-a")
        val second = roomControlIdentityPath(filesDirectory, "relationship-b")

        assertEquals(first, firstAgain)
        assertNotEquals(first, second)
        assertTrue(
            first.matches(
                Regex(".*/room-control/remembered/[0-9a-f]{64}/identity\\.json$"),
            ),
        )
        assertFalse(first.contains("relationship-a"))
    }

    @Test
    fun `remembered room identity rejects an empty relationship`() {
        assertThrows(IllegalArgumentException::class.java) {
            roomControlIdentityPath(filesDirectory, "")
        }
    }
}
