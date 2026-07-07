package dev.envoix.app

import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import org.json.JSONObject

/** Parsed events from the Envoix core (see the client event stream schema). */
sealed interface CliEvent {
    data object Binding : CliEvent
    data object Connecting : CliEvent
    data class Connected(val pathType: String, val addr: String) : CliEvent
    data class Started(val transferId: String, val fileName: String, val totalBytes: Long) : CliEvent
    data class Progress(val bytesTransferred: Long, val totalBytes: Long) : CliEvent
    data class Completed(val bytesTransferred: Long) : CliEvent
    data class Failed(val error: String) : CliEvent
    data class Exit(val code: Int) : CliEvent
}

/**
 * Runs a transfer through the in-process native core and exposes its events as a
 * [Flow]. The native call blocks, so it runs on a dedicated worker thread and
 * delivers events through a callback.
 */
object NativeTransfer {
    fun run(
        id: Long,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        useRoom: Boolean,
        useMdns: Boolean,
    ): Flow<CliEvent> = callbackFlow {
        val callback = object : EventCallback {
            override fun onEvent(json: String) {
                parse(json)?.let { trySend(it) }
            }
        }
        val worker = Thread {
            try {
                Native.runTransfer(id, direction, code, broker, relay, path, configPath, useRoom, useMdns, callback)
            } catch (t: Throwable) {
                trySend(CliEvent.Failed(t.message ?: "native error"))
            } finally {
                trySend(CliEvent.Exit(0))
                channel.close()
            }
        }.apply { isDaemon = true; start() }

        awaitClose { /* native transfer runs to completion; no cancel yet */ }
    }

    private fun parse(line: String): CliEvent? {
        val t = line.trim()
        if (!t.startsWith("{")) return null
        return runCatching {
            val o = JSONObject(t)
            when (o.optString("event")) {
                "binding" -> CliEvent.Binding
                "connecting" -> CliEvent.Connecting
                "connected", "path_changed" -> o.optJSONObject("path")?.let {
                    CliEvent.Connected(it.optString("type"), it.optString("addr"))
                }
                "started" -> CliEvent.Started(
                    o.optString("transfer_id"),
                    o.optString("file_name"),
                    o.optLong("total_bytes"),
                )
                "progress" -> CliEvent.Progress(
                    o.optLong("bytes_transferred"),
                    o.optLong("total_bytes"),
                )
                "completed" -> CliEvent.Completed(o.optLong("bytes_transferred"))
                "failed" -> CliEvent.Failed(
                    o.optString("error").ifEmpty { o.optString("message", "transfer failed") }
                )
                else -> null
            }
        }.getOrNull()
    }
}
