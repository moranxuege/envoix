package dev.envoix.app

import dev.envoix.app.discovery.NfcReadinessOffer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NfcInvitationReaderTest {
    @Test
    fun `private reader selects the Envoix AID and reads a fragmented NDEF file once`() {
        val message = ByteArray(600) { index -> (index * 31).toByte() }
        var completions = 0
        val protocol =
            NfcType4TagProtocol(
                snapshot = { HostedNdefSnapshot(7, message.copyOf()) },
                isCurrent = { generation -> generation == 7L },
                onMessageRead = { completions += 1 },
            )
        val commands = mutableListOf<ByteArray>()

        val read =
            NfcPrivateInvitationReader.readNdefMessage(
                object : NfcIsoDepTransceiver {
                    override fun transceive(command: ByteArray): ByteArray {
                        commands += command.copyOf()
                        return protocol.process(command)
                    }
                },
            )

        assertArrayEquals(message, read)
        assertTrue(commands.first().containsSubsequence(NfcType4TagProtocol.ENVOIX_APPLICATION_AID))
        assertEquals(1, completions)
    }

    @Test
    fun `private reader fails closed on a non-Envoix peer`() {
        var commands = 0

        val read =
            NfcPrivateInvitationReader.readNdefMessage(
                object : NfcIsoDepTransceiver {
                    override fun transceive(command: ByteArray): ByteArray {
                        commands += 1
                        return NfcType4TagProtocol.FILE_NOT_FOUND
                    }
                },
            )

        assertNull(read)
        assertEquals(1, commands)
    }

    @Test
    fun `fresh BLE offer opens one bounded reader lease and duplicates do not reopen it`() {
        val events = mutableListOf<String>()
        val platform = FakeReaderPlatform(events)
        var timeout: (() -> Unit)? = null
        val invitations = mutableListOf<String?>()
        val session =
            NfcReaderLeaseSession(
                platform = platform,
                scheduleTimeout = { delay, action ->
                    events += "timeout:$delay"
                    timeout = action
                },
                cancelTimeout = {
                    events += "cancel-timeout"
                    timeout = null
                },
                onInvitation = invitations::add,
            )
        val offer = NfcReadinessOffer("0123456789abcdef", seenAtMs = 100)

        session.onResume()
        session.enterConnect()
        assertTrue(session.startAutomatic(offer, nowMs = 101))
        assertEquals(
            listOf(
                "idle-listen-only",
                "reset",
                "enable-reader",
                "timeout:${NfcReaderLeaseSession.READER_LEASE_MS}",
            ),
            events,
        )
        assertTrue(session.state.value.scanning)
        assertTrue(session.state.value.automatic)

        platform.complete("envoix://room/redacted")

        assertEquals("envoix://room/redacted", invitations.single())
        assertFalse(session.state.value.scanning)
        assertEquals(
            listOf("disable-reader", "cancel-timeout", "idle-listen-only"),
            events.takeLast(3),
        )
        assertFalse(session.startAutomatic(offer, nowMs = 102))
        assertFalse(
            session.startAutomatic(
                NfcReadinessOffer("2222222222222222", seenAtMs = 102),
                nowMs = 103,
            ),
        )
        assertNull(timeout)

        session.resetAutomaticGate()
        assertTrue(
            session.startAutomatic(
                NfcReadinessOffer("1111111111111111", seenAtMs = 103),
                nowMs = 104,
            ),
        )
    }

    @Test
    fun `stale BLE offer is ignored while manual NFC remains available and times out once`() {
        val events = mutableListOf<String>()
        val platform = FakeReaderPlatform(events)
        var timeout: (() -> Unit)? = null
        val session =
            NfcReaderLeaseSession(
                platform = platform,
                scheduleTimeout = { _, action -> timeout = action },
                cancelTimeout = { timeout = null },
                onInvitation = { error("timeout must not produce an invitation") },
            )
        session.onResume()
        assertTrue(events.isEmpty())
        session.enterConnect()

        assertFalse(
            session.startAutomatic(
                NfcReadinessOffer("fedcba9876543210", seenAtMs = 1),
                nowMs = NfcReaderLeaseSession.MAX_READINESS_AGE_MS + 2,
            ),
        )
        assertTrue(session.startManual())

        requireNotNull(timeout).invoke()

        assertFalse(session.state.value.scanning)
        assertEquals(
            listOf("disable-reader", "idle-listen-only"),
            events.takeLast(2),
        )
    }

    @Test
    fun `leaving Connect ends ReaderMode and restores Android discovery defaults`() {
        val events = mutableListOf<String>()
        val platform = FakeReaderPlatform(events)
        val session =
            NfcReaderLeaseSession(
                platform = platform,
                scheduleTimeout = { _, _ -> },
                cancelTimeout = { events += "cancel-timeout" },
                onInvitation = {},
            )
        session.onResume()
        session.enterConnect()
        assertTrue(session.startManual())
        events.clear()

        session.leaveConnect()

        assertEquals(
            listOf("disable-reader", "cancel-timeout", "reset"),
            events,
        )
        assertFalse(session.state.value.scanning)
    }

    private class FakeReaderPlatform(
        private val events: MutableList<String>,
    ) : NfcReaderLeasePlatform {
        private var completion: ((String?) -> Unit)? = null

        override fun unavailableStatus(): NfcPhoneReaderStatus? = null

        override fun resetDiscoveryTechnology() {
            events += "reset"
        }

        override fun enterIdleListenOnly() {
            events += "idle-listen-only"
        }

        override fun enableReader(onInvitation: (String?) -> Unit): Boolean {
            events += "enable-reader"
            completion = onInvitation
            return true
        }

        override fun disableReader() {
            events += "disable-reader"
        }

        fun complete(invitation: String?) {
            requireNotNull(completion).invoke(invitation)
        }
    }

    private fun ByteArray.containsSubsequence(expected: ByteArray): Boolean =
        indices.any { offset ->
            offset + expected.size <= size &&
                copyOfRange(offset, offset + expected.size).contentEquals(expected)
        }
}
