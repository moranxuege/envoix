package dev.envoix.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

/** Uploads a transfer's log to the rdz log-collection endpoint, keyed by room id
 *  so it lands alongside the peer's log (and the rdz's own). Dev-mode only. */
object LogUpload {
    /** POST [body] to `<server>/logs/<roomId>?side=<side>`; true on a 2xx reply. */
    suspend fun upload(server: String, roomId: String, side: String, body: String): Boolean =
        withContext(Dispatchers.IO) {
            runCatching {
                val url = URL("${server.trimEnd('/')}/logs/$roomId?side=$side")
                (url.openConnection() as HttpURLConnection).run {
                    requestMethod = "POST"
                    doOutput = true
                    connectTimeout = 8000
                    readTimeout = 8000
                    setRequestProperty("Content-Type", "text/plain; charset=utf-8")
                    outputStream.use { it.write(body.toByteArray()) }
                    val ok = responseCode in 200..299
                    disconnect()
                    ok
                }
            }.getOrDefault(false)
        }
}
