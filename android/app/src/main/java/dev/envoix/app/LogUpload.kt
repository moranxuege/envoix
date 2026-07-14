package dev.envoix.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

/** Uploads a transfer's log to the rdz log-collection endpoint, keyed by room id
 *  so it lands alongside the peer's log (and the rdz's own). Dev-mode only. */
object LogUpload {
    /** POST [body] to `<server>/logs/<roomId>?side=<side>`; true on a 2xx reply. */
    suspend fun upload(
        server: String,
        roomId: String,
        side: String,
        body: String,
    ): Boolean =
        withContext(Dispatchers.IO) {
            if (!BuildConfig.DEBUG || !SettingsStore.settings.value.devMode) {
                return@withContext false
            }
            if (!validKey(roomId, 64) || !validKey(side, 32) || body.toByteArray().size > Diagnostics.UPLOAD_MAX) {
                return@withContext false
            }
            for (candidate in uploadServers(server)) {
                val status = uploadOnce(candidate, roomId, side, body)
                when {
                    status != null && status in 200..299 -> return@withContext true
                    status == null || status >= 500 -> continue
                    else -> return@withContext false
                }
            }
            false
        }

    private fun uploadOnce(
        server: String,
        roomId: String,
        side: String,
        body: String,
    ): Int? =
        runCatching {
            val url = URL("${server.trimEnd('/')}/logs/$roomId?side=$side")
            val connection = url.openConnection() as HttpURLConnection
            try {
                connection.requestMethod = "POST"
                connection.doOutput = true
                connection.connectTimeout = 8000
                connection.readTimeout = 8000
                connection.setRequestProperty("Content-Type", "text/plain; charset=utf-8")
                connection.outputStream.use { it.write(body.toByteArray()) }
                connection.responseCode
            } finally {
                connection.disconnect()
            }
        }.getOrNull()

    /** Prefer HTTPS, retaining HTTP only as a connection/5xx fallback. */
    internal fun uploadServers(server: String): List<String> {
        val configured = server.trim().trimEnd('/')
        val preferred =
            if (configured == Endpoints.LOG_SERVER_LEGACY) {
                Endpoints.LOG_SERVER
            } else {
                configured
            }
        val parsed = runCatching { URL(preferred) }.getOrNull() ?: return emptyList()
        if (parsed.protocol != "http" && parsed.protocol != "https") return emptyList()
        val authority = parsed.authority ?: return emptyList()
        val path = parsed.path.trimEnd('/')
        return listOf("https://$authority$path", "http://$authority$path").distinct()
    }

    private fun validKey(
        value: String,
        maxLength: Int,
    ): Boolean =
        value.isNotEmpty() &&
            value.length <= maxLength &&
            value.all { it.isLetterOrDigit() || it == '-' || it == '_' }

    /** POST raw [body] bytes to [url]; true on a 2xx reply. */
    suspend fun postBytes(
        url: String,
        body: ByteArray,
    ): Boolean =
        withContext(Dispatchers.IO) {
            runCatching {
                (URL(url).openConnection() as HttpURLConnection).run {
                    requestMethod = "POST"
                    doOutput = true
                    connectTimeout = 8000
                    readTimeout = 8000
                    setRequestProperty("Content-Type", "application/octet-stream")
                    outputStream.use { it.write(body) }
                    val ok = responseCode in 200..299
                    disconnect()
                    ok
                }
            }.getOrDefault(false)
        }

    /** GET [url]; the response bytes on 200, else null. */
    suspend fun getBytes(url: String): ByteArray? =
        withContext(Dispatchers.IO) {
            runCatching {
                (URL(url).openConnection() as HttpURLConnection).run {
                    connectTimeout = 8000
                    readTimeout = 8000
                    val bytes = if (responseCode == 200) inputStream.use { it.readBytes() } else null
                    disconnect()
                    bytes
                }
            }.getOrNull()
        }
}
