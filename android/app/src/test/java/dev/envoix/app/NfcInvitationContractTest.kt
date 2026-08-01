package dev.envoix.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

class NfcInvitationContractTest {
    @Test
    fun `canonical invitations encode into the exact HTTPS carrier`() {
        assertArrayEquals(
            (
                NfcInvitationContract.CARRIER_PREFIX +
                    "ZW52b2l4Oi8vaW52aXRlL3YyL2FiY19ERUYtMTIz"
            ).toByteArray(Charsets.US_ASCII),
            NfcInvitationContract.encode(INVITE),
        )

        for (invitation in listOf(INVITE, ROOM)) {
            val encoded = NfcInvitationContract.encode(invitation)
            assertEquals(invitation, NfcInvitationContract.decode(requireNotNull(encoded)))
        }
    }

    @Test
    fun `direct supported invitations remain readable`() {
        for (invitation in listOf(INVITE, ROOM)) {
            assertEquals(
                invitation,
                NfcInvitationContract.decode(invitation.toByteArray(Charsets.US_ASCII)),
            )
        }
    }

    @Test
    fun `non canonical invitations are rejected before encoding`() {
        for (invitation in listOf(
            "envoix://invite/v2/",
            "envoix://room/",
            "envoix://pair/legacy",
            "ENVOIX://invite/v2/value",
            "https://example.test",
            " $INVITE",
            "$INVITE ",
            "envoix://invite/v2/has space",
            "envoix://invite/v2/line\nbreak",
            "envoix://invite/v2/café",
        )) {
            assertNull(
                "accepted $invitation",
                NfcInvitationContract.encode(invitation),
            )
        }
    }

    @Test
    fun `malformed or non canonical HTTPS carriers are rejected`() {
        val invalidCanonicalValues =
            listOf(
                "envoix://invite/v2/",
                "envoix://room/",
                "envoix://pair/legacy",
                "ENVOIX://invite/v2/value",
                "envoix://invite/v2/has space",
            )
        val invalidCarriers =
            buildList {
                add(NfcInvitationContract.CARRIER_PREFIX)
                add(NfcInvitationContract.CARRIER_PREFIX + "A")
                add(NfcInvitationContract.CARRIER_PREFIX + "abcd=")
                add(NfcInvitationContract.CARRIER_PREFIX + "abcd+")
                add(NfcInvitationContract.CARRIER_PREFIX + "abcd/")
                add(NfcInvitationContract.CARRIER_PREFIX + "YR")
                add("https://ece4410j-nuub.github.io/nfc/v1/$INVITE")
                add("https://ECE4410J-NUUB.github.io/nfc/v1/#abcd")
                invalidCanonicalValues.forEach { value ->
                    add(
                        NfcInvitationContract.CARRIER_PREFIX +
                            Base64
                                .getUrlEncoder()
                                .withoutPadding()
                                .encodeToString(value.toByteArray(Charsets.UTF_8)),
                    )
                }
            }

        invalidCarriers.forEach { carrier ->
            assertNull(
                "accepted $carrier",
                NfcInvitationContract.decode(carrier.toByteArray(Charsets.UTF_8)),
            )
        }
        assertNull(NfcInvitationContract.decode(byteArrayOf(0x00)))
        assertNull(NfcInvitationContract.decode(byteArrayOf(0x7f)))
        assertNull(NfcInvitationContract.decode("é".toByteArray(Charsets.UTF_8)))
    }

    @Test
    fun `maximum canonical and carrier bounds are exact`() {
        val prefix = "envoix://invite/v2/"
        val maximum =
            prefix +
                "a".repeat(NfcInvitationContract.MAX_INVITATION_BYTES - prefix.length)
        val encoded = requireNotNull(NfcInvitationContract.encode(maximum))

        assertEquals(NfcInvitationContract.maxCarrierBytes, encoded.size)
        assertEquals(maximum, NfcInvitationContract.decode(encoded))
        assertNull(NfcInvitationContract.encode(maximum + "a"))

        val oversizedToken =
            Base64
                .getUrlEncoder()
                .withoutPadding()
                .encodeToString((maximum + "a").toByteArray(Charsets.US_ASCII))
        assertNull(
            NfcInvitationContract.decode(
                (NfcInvitationContract.CARRIER_PREFIX + oversizedToken)
                    .toByteArray(Charsets.US_ASCII),
            ),
        )
        assertTrue(encoded.size > NfcInvitationContract.MAX_INVITATION_BYTES)
    }

    private companion object {
        const val INVITE = "envoix://invite/v2/abc_DEF-123"
        const val ROOM =
            "envoix://room/123456-a1b2-c3d4?broker=example.test&expires=9999999999"
    }
}
