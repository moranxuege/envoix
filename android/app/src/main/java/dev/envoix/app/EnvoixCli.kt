package dev.envoix.app

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.launch
import org.json.JSONObject

/** Parsed events from the Envoix core (see the client event stream schema). */
sealed interface CliEvent {
    data object Binding : CliEvent
    data object Connecting : CliEvent
    data class Connected(val pathType: String, val addr: String) : CliEvent
    data class Started(val transferId: String, val fileName: String, val totalBytes: Long) : CliEvent
    data class Progress(val bytesTransferred: Long, val totalBytes: Long) : CliEvent
    data class Completed(val bytesTransferred: Long) : CliEvent
    /** [reasonCode] is the core's typed failure classification (snake_case:
     *  "paused" / "cancelled" / "peer_paused" / "peer_cancelled" /
     *  "connection_lost" / "other"); empty on synthetic or legacy events. */
    data class Failed(val error: String, val reasonCode: String = "") : CliEvent
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
        chunkSize: String,
        candidatesAllow: String,
        candidatesDeny: String,
        useRoom: Boolean,
        useMdns: Boolean,
    ): Flow<CliEvent> = callbackFlow {
        val callback = object : EventCallback {
            override fun onEvent(json: String) {
                parse(json)?.let { trySend(it) }
            }
        }
        // The native call blocks, so run it on the IO dispatcher as a child of
        // this flow's scope; close the flow when it returns, reporting the real
        // terminal state rather than a blanket Exit(0).
        val job = launch(Dispatchers.IO) {
            val result = runCatching {
                Native.runTransfer(id, direction, code, broker, relay, path, chunkSize, candidatesAllow, candidatesDeny, useRoom, useMdns, callback)
            }
            result.exceptionOrNull()?.let { t ->
                if (t !is CancellationException) trySend(CliEvent.Failed(t.message ?: "native error"))
            }
            trySend(CliEvent.Exit(if (result.isSuccess) 0 else 1))
            close()
        }

        // If the collector is cancelled, stop the native transfer too — otherwise
        // it keeps running detached.
        awaitClose {
            Native.cancel(id)
            job.cancel()
        }
        // The sender writes ~one flow-control window of chunks into the send
        // buffer in a burst, so Progress events can arrive far faster than the UI
        // drains them. trySend on a full BUFFERED channel drops the *newest*
        // event, which sticks the sender's progress bar; keep the latest by
        // dropping the oldest instead so the bar tracks the real byte count.
    }.buffer(capacity = 64, onBufferOverflow = BufferOverflow.DROP_OLDEST)

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
                // The client event stream carries the message as "reason"; the
                // JNI's synthetic terminal event uses "error".
                "failed" -> CliEvent.Failed(
                    o.optString("reason").ifEmpty {
                        o.optString("error").ifEmpty { o.optString("message", "transfer failed") }
                    },
                    o.optString("reason_code"),
                )
                else -> null
            }
        }.getOrNull()
    }
}
