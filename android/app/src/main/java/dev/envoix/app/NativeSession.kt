package dev.envoix.app

import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import org.json.JSONObject

/**
 * A transfer session's notice stream (state-machine snapshots + mailbox courier
 * requests) as a [Flow] of parsed JSON. The Rust driver owns the machine; this
 * is a rendering feed, not an event stream to interpret.
 */
object NativeSession {
    /** Rehydrate a persisted session; its initial snapshot repopulates the card. */
    fun restore(id: Long): Flow<JSONObject> =
        callbackFlow {
            val callback =
                object : EventCallback {
                    override fun onEvent(json: String) {
                        runCatching { JSONObject(json) }.getOrNull()?.let { trySend(it) }
                    }
                }
            Native.restoreSession(id, callback)
            awaitClose { Native.destroySession(id, false) }
        }.buffer(capacity = kotlinx.coroutines.channels.Channel.UNLIMITED)

    fun start(
        id: Long,
        paramsJson: String,
    ): Flow<JSONObject> =
        callbackFlow {
            val callback =
                object : EventCallback {
                    override fun onEvent(json: String) {
                        runCatching { JSONObject(json) }.getOrNull()?.let { trySend(it) }
                    }
                }
            Native.createSession(id, paramsJson, callback)
            awaitClose { Native.destroySession(id, false) }
            // No drops here, unlike the legacy event flow: the driver already
            // throttles snapshots at the source (100ms), and a dropped courier
            // notice would silently lose a receipt post. Low volume, unbounded.
        }.buffer(capacity = kotlinx.coroutines.channels.Channel.UNLIMITED)
}
