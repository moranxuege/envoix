package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BleRendezvousProtocolTest {
    private val identity = LocalDiscoveryIdentity("0011223344556677", "Android phone")
    private val invite = "envoix://pair/123456-alpha-bravo?broker=https%3A%2F%2Fexample.test&role=send"

    @Test
    fun `round trips a fragmented invite`() {
        val frames =
            requireNotNull(
                BleRendezvousProtocol.encodeInvite(identity, invite, REQUEST_ID, maximumFrameBytes = 31),
            )
        val assembler = BleRendezvousProtocol.Assembler()

        val decoded = frames.mapNotNull(assembler::accept).single()

        assertTrue(frames.size > 1)
        assertEquals("0102030405060708", decoded.requestId)
        assertEquals(identity.peerKey, decoded.senderPeerKey)
        assertEquals(identity.displayName, decoded.senderDisplayName)
        assertEquals(invite, decoded.invite)
    }

    @Test
    fun `rejects an out of order continuation and resets`() {
        val frames =
            requireNotNull(
                BleRendezvousProtocol.encodeInvite(identity, invite, REQUEST_ID, maximumFrameBytes = 31),
            )
        val assembler = BleRendezvousProtocol.Assembler()

        assertNull(assembler.accept(frames[1]))
        assertNull(assembler.accept(frames[0]))
        assertNull(assembler.accept(frames[2]))

        val decoded = frames.mapNotNull(assembler::accept).single()
        assertEquals(invite, decoded.invite)
    }

    @Test
    fun `rejects invalid invites and too small frames`() {
        assertNull(BleRendezvousProtocol.encodeInvite(identity, "123456-alpha-bravo", REQUEST_ID, 64))
        assertNull(
            BleRendezvousProtocol.encodeInvite(
                identity,
                "envoix://pair/" + "x".repeat(BleRendezvousProtocol.MAX_INVITE_BYTES),
                REQUEST_ID,
                64,
            ),
        )
        assertNull(
            BleRendezvousProtocol.encodeInvite(
                identity,
                invite,
                REQUEST_ID,
                BleRendezvousProtocol.FRAME_HEADER_SIZE,
            ),
        )
    }

    @Test
    fun `rejects a mismatched security mode`() {
        val frames =
            requireNotNull(
                BleRendezvousProtocol.encodeInvite(identity, invite, REQUEST_ID, maximumFrameBytes = 128),
            )
        frames.first()[BleRendezvousProtocol.FRAME_HEADER_SIZE] = 1

        assertNull(BleRendezvousProtocol.Assembler().accept(frames.single()))
    }

    companion object {
        private const val REQUEST_ID = 0x0102030405060708L
    }
}
