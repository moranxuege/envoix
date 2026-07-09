package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class InviteCodecInstrumentedTest {
    @Test
    fun generatedInviteRoundTripsThroughUniffi() {
        val invite = InviteCodec.generate(
            role = "receive",
            broker = Endpoints.BROKER,
            relay = Endpoints.RELAY,
        )

        assertNotNull("UniFFI invite generation returned null", invite)
        val (code, payload) = invite!!
        assertTrue("invite code should contain the room separator", code.contains("-"))

        val parsedPayload = InviteCodec.parse(payload)
        assertNotNull("UniFFI invite payload parsing returned null", parsedPayload)
        assertEquals(code, parsedPayload!!.code)
        assertEquals(Endpoints.BROKER, parsedPayload.broker)
        assertEquals(Endpoints.RELAY, parsedPayload.relay)
        assertEquals("receive", parsedPayload.role)

        val parsedCode = InviteCodec.parse(code)
        assertNotNull("UniFFI invite code parsing returned null", parsedCode)
        assertEquals(code, parsedCode!!.code)
    }
}
