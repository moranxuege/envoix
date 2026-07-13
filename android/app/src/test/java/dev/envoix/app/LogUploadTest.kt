package dev.envoix.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LogUploadTest {
    @Test
    fun defaultServerPrefersHttpsThenHttp() {
        assertEquals(
            listOf(
                "https://rdz.chkxwlyh.us:8460",
                "http://rdz.chkxwlyh.us:8460",
            ),
            LogUpload.uploadServers(Endpoints.LOG_SERVER),
        )
    }

    @Test
    fun deprecatedIpMigratesToNamedHttpsEndpoint() {
        assertEquals(
            listOf(
                "https://rdz.chkxwlyh.us:8460",
                "http://rdz.chkxwlyh.us:8460",
            ),
            LogUpload.uploadServers("${Endpoints.DEPRECATED_LOG_SERVER}/"),
        )
    }

    @Test
    fun invalidSchemeHasNoUploadCandidate() {
        assertTrue(LogUpload.uploadServers("file:///tmp/log").isEmpty())
    }
}
