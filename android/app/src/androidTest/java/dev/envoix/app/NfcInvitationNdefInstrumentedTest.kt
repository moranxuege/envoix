package dev.envoix.app

import android.nfc.NdefMessage
import android.nfc.NdefRecord
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NfcInvitationNdefInstrumentedTest {
    @Test
    fun uriRecordUsesTheExactHttpsCarrier() {
        val message = requireNotNull(NfcInvitationNdefCodec.messageFor(INVITE))
        val record = message.records.single()
        val expectedCarrier =
            NfcInvitationContract.CARRIER_PREFIX +
                "ZW52b2l4Oi8vaW52aXRlL3YyL2FiY19ERUYtMTIz"

        assertEquals(NdefRecord.TNF_WELL_KNOWN, record.tnf)
        assertArrayEquals(NdefRecord.RTD_URI, record.type)
        assertArrayEquals(ByteArray(0), record.id)
        assertEquals(0, record.payload.first().toInt())
        assertArrayEquals(
            expectedCarrier.toByteArray(Charsets.US_ASCII),
            record.payload.drop(1).toByteArray(),
        )
        assertEquals(expectedCarrier, record.toUri().toString())
        assertEquals(
            INVITE,
            NfcInvitationNdefCodec.invitationFrom(listOf(message)),
        )
    }

    @Test
    fun directEnvoixUriRecordRemainsReadable() {
        val direct =
            NdefMessage(
                arrayOf(
                    uriRecord(INVITE),
                ),
            )

        assertEquals(
            INVITE,
            NfcInvitationNdefCodec.invitationFrom(listOf(direct)),
        )
    }

    @Test
    fun canonicalRoomFallbackUsesTheRoomControlParser() {
        val room =
            "envoix://room/123456-a1b2-c3d4" +
                "?broker=test&relay=https%3A%2F%2Frelay.test&expires=9999999999"
        val parsedRoom =
            JSONObject(
                Native.parseRoomControlInvite(
                    room,
                    "fallback",
                    "",
                ),
            )
        val parsedAsTransfer = JSONObject(Native.parseInvite(room))

        assertFalse(parsedRoom.toString(), parsedRoom.has("error"))
        assertEquals(room, parsedRoom.getString("payload"))
        assertTrue(parsedAsTransfer.toString(), parsedAsTransfer.has("error"))
    }

    @Test
    fun malformedRoomQueriesAreRejectedBeforeConfirmation() {
        val malformedInvitations =
            listOf(
                "envoix://room/123456-a1b2-c3d4" +
                    "?broker=test&relay=https%3A%2F%2Frelay.test&expires=not-a-number",
                "envoix://room/123456-a1b2-c3d4?broker=%ZZ&expires=9999999999",
                "envoix://room/123456-a1b2-c3d4" +
                    "?broker=test&relay=https%3A%2F%2Frelay.test" +
                    "&expires=9999999999&unknown=value",
                "envoix://room/123456-a1b2-c3d4" +
                    "?broker=test&relay=https%3A%2F%2Frelay.test",
                "envoix://room/123456-a1b2-c3d4" +
                    "?broker=test&relay=https%3A%2F%2Frelay.test&expires=1",
            )

        malformedInvitations.forEach { invitation ->
            assertFalse(
                invitation,
                isStrictNativeRoomNfcInvitation(
                    value = invitation,
                    fallbackBroker = "fallback",
                    fallbackRelay = "",
                    nowEpochMs = 2_000L,
                ),
            )
        }
        assertTrue(
            isStrictNativeRoomNfcInvitation(
                value =
                    "envoix://room/123456-a1b2-c3d4" +
                        "?broker=test&relay=https%3A%2F%2Frelay.test&expires=9999999999",
                fallbackBroker = "fallback",
                fallbackRelay = "",
                nowEpochMs = 1L,
            ),
        )
    }

    @Test
    fun nonCanonicalMessagesAreRejected() {
        val valid = requireNotNull(NfcInvitationNdefCodec.messageFor(INVITE)).records.single()
        val second = NdefRecord.createTextRecord("en", "extra")
        val exactPrefixOnly =
            NfcInvitationContract.CARRIER_PREFIX +
                "ZW52b2l4Oi8vaW52aXRlL3YyLw"

        val invalid =
            listOf(
                emptyList(),
                listOf(NdefMessage(arrayOf(valid)), NdefMessage(arrayOf(valid))),
                listOf(NdefMessage(arrayOf(valid, second))),
                listOf(
                    NdefMessage(
                        arrayOf(
                            NdefRecord(
                                NdefRecord.TNF_WELL_KNOWN,
                                NdefRecord.RTD_URI,
                                byteArrayOf(1),
                                valid.payload,
                            ),
                        ),
                    ),
                ),
                listOf(
                    NdefMessage(
                        arrayOf(
                            NdefRecord(
                                NdefRecord.TNF_WELL_KNOWN,
                                NdefRecord.RTD_URI,
                                ByteArray(0),
                                byteArrayOf(1) + INVITE.toByteArray(Charsets.US_ASCII),
                            ),
                        ),
                    ),
                ),
                listOf(NdefMessage(arrayOf(second))),
                listOf(NdefMessage(arrayOf(uriRecord(exactPrefixOnly)))),
                listOf(
                    NdefMessage(
                        arrayOf(
                            uriRecord(
                                NfcInvitationContract.CARRIER_PREFIX +
                                    "ZW52b2l4Oi8vaW52aXRlL3YyL2FiY19ERUYtMTIz=",
                            ),
                        ),
                    ),
                ),
            )

        invalid.forEach { messages ->
            assertNull(NfcInvitationNdefCodec.invitationFrom(messages))
        }
    }

    private fun uriRecord(value: String): NdefRecord =
        NdefRecord(
            NdefRecord.TNF_WELL_KNOWN,
            NdefRecord.RTD_URI,
            ByteArray(0),
            byteArrayOf(0) + value.toByteArray(Charsets.US_ASCII),
        )

    private companion object {
        const val INVITE = "envoix://invite/v2/abc_DEF-123"
    }
}
