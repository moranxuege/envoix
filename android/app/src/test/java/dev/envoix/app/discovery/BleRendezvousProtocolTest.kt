package dev.envoix.app.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BleRendezvousProtocolTest {
    private val identity = LocalDiscoveryIdentity("0011223344556677", "Android phone")

    private fun offer(): String = "envoix://ble/v1/123456?broker=example.test&expires=${System.currentTimeMillis() / 1_000L + 300}"

    @Test
    fun `round trips a full BLE discovery identity`() {
        val longIdentity =
            LocalDiscoveryIdentity(
                peerKey = identity.peerKey,
                displayName = "Nearby " + "📱".repeat(20),
            )

        val encoded = requireNotNull(BleRendezvousProtocol.encodeIdentity(longIdentity))
        val decoded = requireNotNull(BleRendezvousProtocol.decodeIdentity(encoded))

        assertEquals(longIdentity.peerKey, decoded.peerKey)
        assertEquals(longIdentity.displayName, decoded.displayName)
    }

    @Test
    fun `identity payload matches the cross-platform wire vector`() {
        val encoded =
            requireNotNull(
                BleRendezvousProtocol.encodeIdentity(
                    LocalDiscoveryIdentity("0011223344556677", "设备"),
                ),
            )

        assertEquals(
            "01303031313232333334343535363637370006e8aebee5a487",
            encoded.joinToString(separator = "") { byte -> "%02x".format(byte.toInt() and 0xff) },
        )
        assertEquals(
            BleDiscoveryIdentity("0011223344556677", "设备"),
            BleRendezvousProtocol.decodeIdentity(encoded),
        )
    }

    @Test
    fun `identity payload validates key version length and UTF-8`() {
        val encoded = requireNotNull(BleRendezvousProtocol.encodeIdentity(identity))

        assertNull(BleRendezvousProtocol.decodeIdentity(encoded.copyOf().also { it[0] = 2 }))
        assertNull(BleRendezvousProtocol.decodeIdentity(encoded.copyOf().also { it[1] = 'z'.code.toByte() }))
        assertNull(BleRendezvousProtocol.decodeIdentity(encoded.copyOf(encoded.size - 1)))
        assertNull(
            BleRendezvousProtocol.decodeIdentity(
                encoded.copyOf().also {
                    it[it.lastIndex] = 0xFF.toByte()
                },
            ),
        )
        assertNull(
            BleRendezvousProtocol.encodeIdentity(
                LocalDiscoveryIdentity(identity.peerKey, "Nearby\u0000phone"),
            ),
        )
    }

    @Test
    fun `round trips a fragmented invite`() {
        val invite = offer()
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
    fun `rejects plaintext room and transfer invitations`() {
        assertNull(
            BleRendezvousProtocol.encodeInvite(
                identity,
                "envoix://room/123456-a1b2-c3d4?broker=example.test",
                REQUEST_ID,
                128,
            ),
        )
        assertNull(BleRendezvousProtocol.encodeInvite(identity, "envoix://invite/v2/secret", REQUEST_ID, 128))
    }

    @Test
    fun `rejects an out of order continuation and resets`() {
        val invite = offer()
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
        val unsupported =
            listOf(
                "123456-a1b2-c3d4",
                "https://example.test/envoix-room",
                "envoix://pair/123456-alpha-bravo",
                "envoix://invite/v2/",
                "envoix://room/R123456-a1b2-c3d4",
                "envoix://room/123456-alpha-bravo",
                "envoix://invite/v2/secret",
            )
        unsupported.forEach {
            assertNull(BleRendezvousProtocol.encodeInvite(identity, it, REQUEST_ID, 64))
        }
        assertNull(
            BleRendezvousProtocol.encodeInvite(
                identity,
                "envoix://ble/v1/123456?broker=" + "x".repeat(BleRendezvousProtocol.MAX_INVITE_BYTES),
                REQUEST_ID,
                64,
            ),
        )
        assertNull(
            BleRendezvousProtocol.encodeInvite(
                identity,
                offer(),
                REQUEST_ID,
                BleRendezvousProtocol.FRAME_HEADER_SIZE,
            ),
        )
    }

    @Test
    fun `rejects a mismatched security mode`() {
        val invite = offer()
        val frames =
            requireNotNull(
                BleRendezvousProtocol.encodeInvite(identity, invite, REQUEST_ID, maximumFrameBytes = 128),
            )
        frames.first()[BleRendezvousProtocol.FRAME_HEADER_SIZE] = 1

        assertNull(BleRendezvousProtocol.Assembler().accept(frames.single()))
    }

    @Test
    fun `verification code reconstructs the private PAKE invitation`() {
        val now = 1_700_000_000_000L
        val publicOffer =
            "envoix://ble/v1/123456?broker=example.test&relay=https%3A%2F%2Frelay.test&expires=1700000300"

        val invitation = requireNotNull(BleVerificationInvitation.resolve(publicOffer, "654321", now))
        val query = java.net.URI(invitation).rawQuery

        assertEquals("/123456-db32-0129", java.net.URI(invitation).path)
        assertTrue(query.contains("broker=example.test"))
        assertTrue(BleVerificationInvitation.isPublicOffer(publicOffer, now))
        assertNull(BleVerificationInvitation.resolve(publicOffer, "65432", now))
        assertNull(BleVerificationInvitation.resolve(publicOffer, "654321", now + 300_000L))
        assertNull(BleVerificationInvitation.resolve("$publicOffer&relay", "654321", now))
        assertNull(BleVerificationInvitation.resolve("$publicOffer&relay=%", "654321", now))
        val tamperedOffer = publicOffer.replace("broker=example.test", "broker=other.test")
        assertNotEquals(
            java.net.URI(invitation).path,
            java.net.URI(requireNotNull(BleVerificationInvitation.resolve(tamperedOffer, "654321", now))).path,
        )
    }

    @Test
    fun `generated verification offer round trips without reusing the public locator as code`() {
        val now = 1_700_000_000_000L
        val verification = BleVerificationInvitation.create("example.test", "", now)

        assertEquals(
            verification.privateInvitation,
            BleVerificationInvitation.resolve(verification.publicOffer, verification.verificationCode, now),
        )
        assertNotEquals(
            verification.verificationCode,
            java.net
                .URI(verification.publicOffer)
                .path
                .substringAfterLast('/'),
        )
    }

    companion object {
        private const val REQUEST_ID = 0x0102030405060708L
    }
}
