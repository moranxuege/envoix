package dev.envoix.app.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class RememberedRoomConnectionPolicyTest {
    @Test
    fun `file picker lease keeps links alive while activity is stopped`() {
        assertTrue(
            shouldKeepRememberedRoomLinks(
                foreground = false,
                externalActivityLeases = 1,
            ),
        )
    }

    @Test
    fun `links stop only after both foreground and picker leases end`() {
        assertTrue(
            shouldKeepRememberedRoomLinks(
                foreground = true,
                externalActivityLeases = 0,
            ),
        )
        assertFalse(
            shouldKeepRememberedRoomLinks(
                foreground = false,
                externalActivityLeases = 0,
            ),
        )
    }

    @Test
    fun `negative picker leases are rejected`() {
        assertThrows(IllegalArgumentException::class.java) {
            shouldKeepRememberedRoomLinks(
                foreground = false,
                externalActivityLeases = -1,
            )
        }
    }
}
