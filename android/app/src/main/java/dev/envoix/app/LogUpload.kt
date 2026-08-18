package dev.envoix.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

/** Uploads a transfer's log to the rdz log-collection endpoint, keyed by room id
 *  so it lands alongside the peer's log (and the rdz's own). Dev-mode only. */
object LogUpload {
    /** Connect + read timeout for every log-courier HTTP call (ms). */
    private const val TIMEOUT_MS = 8000

    /** Developer-injected, process-local token. It is deliberately not persisted in app settings. */
    private const val UPLOAD_TOKEN_PROPERTY = "envoix.diagnosticUploadToken"
    internal const val BODY_MAX_BYTES = 480 * 1024
    private const val TOKEN_MAX_BYTES = 1024

    /** POST [body] to `<server>/logs/<roomId>?side=<side>`; true on a 2xx reply. */
    suspend fun upload(
        server: String,
        roomId: String,
        side: String,
        body: String,
    ): Boolean =
        withContext(Dispatchers.IO) {
            runCatching {
                val url = uploadUrl(server, roomId, side) ?: return@runCatching false
                val authorization =
                    bearerHeader(System.getProperty(UPLOAD_TOKEN_PROPERTY))
                        ?: return@runCatching false
                val bytes = boundedBody(body) ?: return@runCatching false
                (url.openConnection() as HttpURLConnection).run {
                    requestMethod = "POST"
                    doOutput = true
                    connectTimeout = TIMEOUT_MS
                    readTimeout = TIMEOUT_MS
                    setRequestProperty("Content-Type", "text/plain; charset=utf-8")
                    setRequestProperty("Authorization", authorization)
                    outputStream.use { it.write(bytes) }
                    val ok = responseCode in 200..299
                    disconnect()
                    ok
                }
            }.getOrDefault(false)
        }

    internal fun uploadUrl(
        server: String,
        roomId: String,
        side: String,
    ): URL? {
        if (!validCorrelationField(roomId, 64) || !validCorrelationField(side, 16)) {
            return null
        }
        val base = runCatching { URL(server.trim()) }.getOrNull() ?: return null
        if (
            base.protocol != "https" ||
            base.host.isBlank() ||
            base.userInfo != null ||
            base.query != null ||
            base.ref != null
        ) {
            return null
        }
        val path = base.path.trimEnd('/') + "/logs/$roomId?side=$side"
        return runCatching { URL(base.protocol, base.host, base.port, path) }.getOrNull()
    }

    internal fun bearerHeader(token: String?): String? {
        val value = token?.trim().orEmpty()
        val bytes = value.toByteArray(Charsets.UTF_8)
        if (
            bytes.isEmpty() ||
            bytes.size > TOKEN_MAX_BYTES ||
            bytes.any { it.toInt() !in 0x21..0x7e }
        ) {
            return null
        }
        return "Bearer $value"
    }

    internal fun boundedBody(body: String): ByteArray? {
        val bytes = body.toByteArray(Charsets.UTF_8)
        return bytes.takeIf { it.size <= BODY_MAX_BYTES }
    }

    private fun validCorrelationField(
        value: String,
        maxBytes: Int,
    ): Boolean =
        value.isNotEmpty() &&
            value.length <= maxBytes &&
            value.all { it.isLetterOrDigit() && it.code < 128 }

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
                    connectTimeout = TIMEOUT_MS
                    readTimeout = TIMEOUT_MS
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
                    connectTimeout = TIMEOUT_MS
                    readTimeout = TIMEOUT_MS
                    val bytes = if (responseCode == 200) inputStream.use { it.readBytes() } else null
                    disconnect()
                    bytes
                }
            }.getOrNull()
        }
}
