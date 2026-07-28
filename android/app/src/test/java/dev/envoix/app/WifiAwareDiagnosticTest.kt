package dev.envoix.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class WifiAwareDiagnosticTest {
    @Test
    fun probeProtocolRoundTripsAndRejectsCorruption() {
        val nonce = ByteArray(WifiAwareProbeWireProtocol.NONCE_LENGTH) { it.toByte() }
        val request = WifiAwareProbeWireProtocol.makeRequest(nonce)
        val response = WifiAwareProbeWireProtocol.makeResponse(request)

        assertEquals(WifiAwareProbeWireProtocol.FRAME_LENGTH, request.size)
        WifiAwareProbeWireProtocol.validateResponse(response, nonce)

        val corrupted = response.copyOf().also { it[it.lastIndex] = (it.last() + 1).toByte() }
        val error =
            assertThrows(WifiAwareProbeProtocolException::class.java) {
                WifiAwareProbeWireProtocol.validateResponse(corrupted, nonce)
            }
        assertEquals(WifiAwareProbeProtocolFailure.NONCE_MISMATCH, error.failure)
    }

    @Test
    fun probeProtocolRejectsInvalidInputLengthsAndMagic() {
        assertFailure(WifiAwareProbeProtocolFailure.INVALID_NONCE_LENGTH) {
            WifiAwareProbeWireProtocol.makeRequest(byteArrayOf())
        }
        assertFailure(WifiAwareProbeProtocolFailure.INVALID_FRAME_LENGTH) {
            WifiAwareProbeWireProtocol.makeResponse(byteArrayOf())
        }

        val nonce = ByteArray(WifiAwareProbeWireProtocol.NONCE_LENGTH)
        val request = WifiAwareProbeWireProtocol.makeRequest(nonce).also { it[0] = 0 }
        assertFailure(WifiAwareProbeProtocolFailure.INVALID_REQUEST_MAGIC) {
            WifiAwareProbeWireProtocol.makeResponse(request)
        }
    }

    @Test
    fun probeWireConstantsMatchAppleContract() {
        assertEquals("_envoix-probe._tcp", ENVOIX_WIFI_AWARE_PROBE_SERVICE)
        val nonce = ByteArray(WifiAwareProbeWireProtocol.NONCE_LENGTH) { it.toByte() }
        assertArrayEquals(
            "ENVXWA01".encodeToByteArray() + nonce,
            WifiAwareProbeWireProtocol.makeRequest(nonce),
        )
    }

    private fun assertFailure(
        expected: WifiAwareProbeProtocolFailure,
        block: () -> Unit,
    ) {
        val error = assertThrows(WifiAwareProbeProtocolException::class.java, block)
        assertEquals(expected, error.failure)
    }
}
