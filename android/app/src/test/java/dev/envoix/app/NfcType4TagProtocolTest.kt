package dev.envoix.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class NfcType4TagProtocolTest {
    @Test
    fun `standard Type 4 selection exposes capability container and NDEF file`() {
        val message = byteArrayOf(0xd1.toByte(), 0x01, 0x01, 0x55, 0x00)
        val harness = Harness(message)

        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.CAPABILITY_CONTAINER_FILE_ID)),
        )
        assertArrayEquals(
            byteArrayOf(
                0x00,
                0x0f,
                0x20,
                0x00,
                0xff.toByte(),
                0x00,
                0xff.toByte(),
                0x04,
                0x06,
                0xe1.toByte(),
                0x04,
                0x7f,
                0xff.toByte(),
                0x00,
                0xff.toByte(),
                0x90.toByte(),
                0x00,
            ),
            harness.protocol.process(readBinary(offset = 0, length = 15)),
        )

        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )
        assertArrayEquals(
            byteArrayOf(0x00, message.size.toByte(), 0x90.toByte(), 0x00),
            harness.protocol.process(readBinary(offset = 0, length = 2)),
        )
        assertArrayEquals(
            message.copyOfRange(0, 3) + NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(readBinary(offset = 2, length = 3)),
        )
        assertArrayEquals(
            message.copyOfRange(3, message.size) + NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(readBinary(offset = 5, length = 0)),
        )
    }

    @Test
    fun `optional select response length is accepted`() {
        val harness = Harness(byteArrayOf(0xd0.toByte()))
        val withoutLe = selectAid().copyOf(selectAid().size - 1)

        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(withoutLe))
    }

    @Test
    fun `private Envoix AID exposes the same Type 4 NDEF file`() {
        val message = byteArrayOf(0xd1.toByte(), 0x01, 0x01, 0x55, 0x00)
        val harness = Harness(message)

        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectAid(NfcType4TagProtocol.ENVOIX_APPLICATION_AID)),
        )
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )
        assertArrayEquals(
            byteArrayOf(0x00, message.size.toByte()) +
                message +
                NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(readBinary(offset = 0, length = 0)),
        )
    }

    @Test
    fun `last chunk alone does not report a completed invitation read`() {
        var completions = 0
        val harness =
            Harness(
                message = byteArrayOf(1, 2, 3, 4),
                onMessageRead = { completions += 1 },
            )
        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )

        harness.protocol.process(readBinary(offset = 4, length = 2))
        assertEquals(0, completions)

        harness.protocol.process(readBinary(offset = 0, length = 4))
        assertEquals(0, completions)

        harness.protocol.process(readBinary(offset = 4, length = 2))
        assertEquals(1, completions)
    }

    @Test
    fun `a hidden or replaced invitation invalidates an active read`() {
        val harness = Harness(byteArrayOf(1, 2, 3))
        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )

        harness.generation += 1
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            harness.protocol.process(readBinary(offset = 0, length = 2)),
        )

        harness.message = byteArrayOf(4, 5)
        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )
        assertArrayEquals(
            byteArrayOf(0x00, 0x02, 0x04, 0x05, 0x90.toByte(), 0x00),
            harness.protocol.process(readBinary(offset = 0, length = 8)),
        )
    }

    @Test
    fun `deactivation wipes the session selection`() {
        val harness = Harness(byteArrayOf(1))
        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )

        harness.protocol.deactivate()

        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            harness.protocol.process(readBinary(offset = 0, length = 1)),
        )
    }

    @Test
    fun `missing invitation and unknown application fail closed`() {
        val harness = Harness(null)
        assertStatus(NfcType4TagProtocol.FILE_NOT_FOUND, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )

        harness.message = byteArrayOf(1)
        val unknownAid = selectAid().also { it[11] = 0x02 }
        assertStatus(NfcType4TagProtocol.FILE_NOT_FOUND, harness.protocol.process(unknownAid))
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )
    }

    @Test
    fun `malformed and unsupported APDUs return bounded ISO 7816 errors`() {
        val harness = Harness(byteArrayOf(1))

        assertStatus(NfcType4TagProtocol.WRONG_LENGTH, harness.protocol.process(byteArrayOf()))
        assertStatus(
            NfcType4TagProtocol.CLASS_NOT_SUPPORTED,
            harness.protocol.process(byteArrayOf(0x01, 0xa4.toByte(), 0x04, 0x00)),
        )
        assertStatus(
            NfcType4TagProtocol.INSTRUCTION_NOT_SUPPORTED,
            harness.protocol.process(byteArrayOf(0x00, 0xca.toByte(), 0x00, 0x00)),
        )
        assertStatus(
            NfcType4TagProtocol.WRONG_LENGTH,
            harness.protocol.process(byteArrayOf(0x00, 0xa4.toByte(), 0x04, 0x00)),
        )
        assertStatus(
            NfcType4TagProtocol.INCORRECT_PARAMETERS,
            harness.protocol.process(
                selectAid().also {
                    it[3] = 0x0c
                },
            ),
        )

        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.FILE_NOT_FOUND,
            harness.protocol.process(selectFile(byteArrayOf(0xe1.toByte(), 0x05))),
        )
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            harness.protocol.process(readBinary(offset = 0, length = 1)),
        )
        assertStatus(
            NfcType4TagProtocol.WRONG_LENGTH,
            harness.protocol.process(readBinary(offset = 0, length = 1) + byteArrayOf(0x00)),
        )
    }

    @Test
    fun `NDEF file length and offsets enforce the advertised maximum`() {
        val harness = Harness(ByteArray(NfcType4TagProtocol.MAX_NDEF_MESSAGE_BYTES) { 0x5a })
        assertStatus(NfcType4TagProtocol.SUCCESS, harness.protocol.process(selectAid()))
        assertStatus(
            NfcType4TagProtocol.SUCCESS,
            harness.protocol.process(selectFile(NfcType4TagProtocol.NDEF_FILE_ID)),
        )
        assertArrayEquals(
            byteArrayOf(0x7f, 0xfd.toByte(), 0x90.toByte(), 0x00),
            harness.protocol.process(readBinary(offset = 0, length = 2)),
        )
        assertArrayEquals(
            byteArrayOf(0x5a, 0x90.toByte(), 0x00),
            harness.protocol.process(
                readBinary(
                    offset = NfcType4TagProtocol.MAX_NDEF_FILE_BYTES - 1,
                    length = 1,
                ),
            ),
        )
        assertStatus(
            NfcType4TagProtocol.WRONG_PARAMETERS,
            harness.protocol.process(
                readBinary(
                    offset = NfcType4TagProtocol.MAX_NDEF_FILE_BYTES,
                    length = 1,
                ),
            ),
        )

        val oversized =
            Harness(ByteArray(NfcType4TagProtocol.MAX_NDEF_MESSAGE_BYTES + 1) { 0x5a })
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            oversized.protocol.process(selectAid()),
        )
        val empty = Harness(ByteArray(0))
        assertStatus(
            NfcType4TagProtocol.CONDITIONS_NOT_SATISFIED,
            empty.protocol.process(selectAid()),
        )
    }

    @Test
    fun `debug APDU trace reports structure without command data`() {
        assertEquals(
            "select-ndef-application",
            NfcType4TagProtocol.traceCommandShape(selectAid()),
        )
        assertEquals(
            "select-envoix-application",
            NfcType4TagProtocol.traceCommandShape(
                selectAid(NfcType4TagProtocol.ENVOIX_APPLICATION_AID),
            ),
        )
        assertEquals(
            "select-capability-container",
            NfcType4TagProtocol.traceCommandShape(
                selectFile(NfcType4TagProtocol.CAPABILITY_CONTAINER_FILE_ID),
            ),
        )
        assertEquals(
            "select-ndef-file",
            NfcType4TagProtocol.traceCommandShape(
                selectFile(NfcType4TagProtocol.NDEF_FILE_ID),
            ),
        )
        assertEquals(
            "read-binary-short",
            NfcType4TagProtocol.traceCommandShape(readBinary(offset = 0x1234, length = 0x56)),
        )

        val invitation = "envoix://invite/v2/private-value"
        val untrustedSelect =
            byteArrayOf(0x00, 0xa4.toByte(), 0x04, 0x00, invitation.length.toByte()) +
                invitation.toByteArray()
        val trace = NfcType4TagProtocol.traceCommandShape(untrustedSelect)
        assertEquals("select-other-application", trace)
        assertFalse(trace.contains(invitation))
        assertFalse(trace.contains("private"))
    }

    @Test
    fun `debug response trace reports only the terminal status word`() {
        val privateBody = "envoix://invite/v2/private-value".toByteArray()
        val trace =
            NfcType4TagProtocol.traceResponseStatus(
                privateBody + NfcType4TagProtocol.SUCCESS,
            )

        assertEquals("sw=9000", trace)
        assertFalse(trace.contains("envoix"))
        assertFalse(trace.contains("private"))
        assertEquals(
            "malformed-response",
            NfcType4TagProtocol.traceResponseStatus(byteArrayOf(0x00)),
        )
    }

    private class Harness(
        var message: ByteArray?,
        onMessageRead: (Long) -> Unit = {},
    ) {
        var generation = 1L
        val protocol =
            NfcType4TagProtocol(
                snapshot = {
                    message?.let {
                        HostedNdefSnapshot(
                            generation = generation,
                            message = it.copyOf(),
                        )
                    }
                },
                isCurrent = { it == generation && message != null },
                onMessageRead = onMessageRead,
            )
    }

    private companion object {
        fun selectAid(aid: ByteArray = NfcType4TagProtocol.NDEF_APPLICATION_AID): ByteArray =
            byteArrayOf(
                0x00,
                0xa4.toByte(),
                0x04,
                0x00,
                aid.size.toByte(),
            ) + aid + byteArrayOf(0x00)

        fun selectFile(fileId: ByteArray): ByteArray =
            byteArrayOf(
                0x00,
                0xa4.toByte(),
                0x00,
                0x0c,
                0x02,
            ) + fileId

        fun readBinary(
            offset: Int,
            length: Int,
        ): ByteArray =
            byteArrayOf(
                0x00,
                0xb0.toByte(),
                (offset ushr 8).toByte(),
                offset.toByte(),
                length.toByte(),
            )

        fun assertStatus(
            expected: ByteArray,
            actual: ByteArray,
        ) {
            assertArrayEquals(expected, actual)
        }
    }
}
