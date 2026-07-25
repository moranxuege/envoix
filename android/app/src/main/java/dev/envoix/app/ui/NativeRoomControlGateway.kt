package dev.envoix.app.ui

import android.content.Context
import dev.envoix.app.Native
import dev.envoix.app.RoomControlCallback
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.mapNotNull
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

internal class NativeRoomControlGateway(
    context: Context,
) : RoomControlGateway {
    override val available: Boolean = true
    private val mutableEvents =
        MutableSharedFlow<GeneratedRoomControlEvent>(
            extraBufferCapacity = 64,
            onBufferOverflow = BufferOverflow.DROP_OLDEST,
        )
    override val events: Flow<RoomControlEvent> =
        mutableEvents.mapNotNull { generated ->
            synchronized(sessionLock) {
                val terminal =
                    generated.event is RoomControlEvent.Closed ||
                        generated.event is RoomControlEvent.Failed
                generated.event.takeIf {
                    activeSessionId == generated.sessionId ||
                        (terminal && latestSessionId == generated.sessionId)
                }
            }
        }

    private val identityPath =
        File(context.filesDir, "room-control/identity.json").absolutePath
    private val nextSessionId = AtomicLong(1L)
    private val sessionLock = Any()

    @Volatile
    private var activeSessionId: Long? = null
    private var latestSessionId: Long? = null

    @Volatile
    private var connected = false

    @Volatile
    private var localCloseReason: RoomCloseReason? = null
    private var hostSettings: HostSettings? = null
    private val pendingOfferResponses = ConcurrentHashMap<String, PendingOfferResponse>()

    override suspend fun host(
        displayName: String,
        broker: String,
        relay: String,
    ) {
        hostSettings = HostSettings(displayName, broker, relay)
        val invite = parseInviteResponse(Native.generateRoomControlInvite(broker, relay))
        startSession(
            mode = "host",
            input = invite.payload,
            displayName = displayName,
            fallbackBroker = broker,
            fallbackRelay = relay,
            initialEvent = RoomControlEvent.Hosting(invite),
        )
    }

    override suspend fun join(
        input: String,
        displayName: String,
    ) {
        hostSettings = null
        val settings = dev.envoix.app.SettingsStore.settings.value
        val invite =
            parseInviteResponse(
                Native.parseRoomControlInvite(
                    input,
                    settings.broker,
                    settings.relay,
                ),
            )
        startSession(
            mode = "join",
            input = invite.payload,
            displayName = displayName,
            fallbackBroker = settings.broker,
            fallbackRelay = settings.relay,
            initialEvent = RoomControlEvent.Joining,
        )
    }

    override suspend fun refreshInvite() {
        val settings = hostSettings ?: error("Only a hosted room invitation can be refreshed")
        // host() generates and validates the replacement first; startSession()
        // swaps generations only after that succeeds.
        host(settings.displayName, settings.broker, settings.relay)
    }

    override suspend fun offerTransfer(draft: RoomTransferOfferDraft) {
        sendCommand(
            JSONObject()
                .put("command", "offer")
                .put("offer_id", draft.id)
                .put("transfer_invite", draft.transferInvite)
                .put("root_names", JSONArray(draft.rootNames.take(3)))
                .put("item_count", draft.itemCount.coerceAtLeast(0))
                .put("total_bytes", draft.totalBytes.coerceAtLeast(0L)),
        )
    }

    override suspend fun respondToOffer(
        offerId: String,
        accept: Boolean,
    ) {
        val pending =
            PendingOfferResponse(
                accepted = accept,
                delivered = CompletableDeferred(),
            )
        check(pendingOfferResponses.putIfAbsent(offerId, pending) == null) {
            "A response for this file offer is already pending"
        }
        try {
            sendCommand(
                JSONObject()
                    .put("command", "respond")
                    .put("offer_id", offerId)
                    .put("accept", accept),
            )
            // JNI queueing is not delivery. Wait until the Rust control task
            // confirms that the response was actually sent to the peer.
            // Do not impose a local timeout: the Rust send may still complete
            // after a stalled stream recovers. Canceling the receiver while a
            // late Accept can reach the peer would create a one-sided transfer.
            // Closed/Failed/command_failed are the authoritative barriers.
            pending.delivered.await()
        } finally {
            pendingOfferResponses.remove(offerId, pending)
        }
    }

    override suspend fun updatePolicy(policy: RoomLifetimePolicy) {
        sendCommand(
            JSONObject()
                .put("command", "policy")
                .put("policy", policy.wireValue()),
        )
    }

    override suspend fun close(reason: RoomCloseReason) {
        val localCloseGeneration =
            synchronized(sessionLock) {
                val id = activeSessionId ?: return@synchronized null
                localCloseReason = reason
                if (!connected) {
                    cancelGenerationLocked(id)
                    id
                } else {
                    runCatching {
                        sendCommandLocked(
                            expectedSessionId = id,
                            command =
                                JSONObject()
                                    .put("command", "close")
                                    .put("reason", reason.wireValue()),
                        )
                    }.fold(
                        onSuccess = { null },
                        onFailure = {
                            cancelGenerationLocked(id)
                            id
                        },
                    )
                }
            }
        localCloseGeneration?.let { generation ->
            emit(generation, RoomControlEvent.Closed(reason))
        }
    }

    private fun startSession(
        mode: String,
        input: String,
        displayName: String,
        fallbackBroker: String,
        fallbackRelay: String,
        initialEvent: RoomControlEvent,
    ) = synchronized(sessionLock) {
        cancelCurrentLocked()
        val id = nextSessionId.getAndIncrement()
        activeSessionId = id
        latestSessionId = id
        connected = false
        localCloseReason = null
        emit(id, initialEvent)
        val params =
            JSONObject()
                .put("mode", mode)
                .put("input", input)
                .put("display_name", displayName)
                .put("identity_path", identityPath)
                .put("fallback_broker", fallbackBroker)
                .put("fallback_relay", fallbackRelay)
        Native.startRoomControlSession(
            id,
            params.toString(),
            object : RoomControlCallback {
                override fun onEvent(json: String) {
                    handleNativeEvent(id, json)
                }
            },
        )
    }

    private fun sendCommand(command: JSONObject) =
        synchronized(sessionLock) {
            val id = activeSessionId ?: error("Room control is not active")
            sendCommandLocked(id, command)
        }

    private fun sendCommandLocked(
        expectedSessionId: Long,
        command: JSONObject,
    ) {
        check(activeSessionId == expectedSessionId) {
            "Room-control session changed before the command was queued"
        }
        val response =
            JSONObject(
                Native.sendRoomControlCommand(
                    expectedSessionId,
                    command.toString(),
                ),
            )
        response.optString("error").takeIf(String::isNotBlank)?.let(::error)
        check(response.optBoolean("queued")) { "Room-control command was not queued" }
    }

    private fun handleNativeEvent(
        id: Long,
        json: String,
    ) = synchronized(sessionLock) {
        // A callback from an old generation may already be in flight while a
        // replacement starts. Re-check under the same lock used by start and
        // cancel before touching any state or completing any operation.
        if (activeSessionId != id) return@synchronized
        val value =
            runCatching { JSONObject(json) }
                .getOrElse {
                    emit(id, RoomControlEvent.Failed("Room control returned an invalid event"))
                    return@synchronized
                }
        when (value.optString("state")) {
            "connected" -> {
                connected = true
                emit(
                    id,
                    RoomControlEvent.Connected(
                        peerName = value.optString("peer_name").takeIf(String::isNotBlank),
                        creator = value.optBoolean("creator"),
                        policy = value.optString("policy").roomPolicy(),
                    ),
                )
            }
            "incoming_offer" -> {
                val offer = value.optJSONObject("offer")
                if (offer == null) {
                    emit(id, RoomControlEvent.Failed("Room control returned an invalid file offer"))
                } else {
                    emit(id, RoomControlEvent.IncomingOffer(offer.roomOffer()))
                }
            }
            "offer_accepted" ->
                emit(id, RoomControlEvent.OfferAccepted(value.getString("offer_id")))
            "offer_rejected" ->
                emit(
                    id,
                    RoomControlEvent.OfferRejected(
                        offerId = value.getString("offer_id"),
                        reason = value.optString("reason").takeIf(String::isNotBlank),
                    ),
                )
            "offer_response_sent" -> {
                val offerId = value.optString("offer_id")
                val accepted = value.optBoolean("accepted")
                pendingOfferResponses[offerId]
                    ?.takeIf { it.accepted == accepted }
                    ?.delivered
                    ?.complete(Unit)
            }
            "command_failed" -> {
                if (localCloseReason != null) return@synchronized
                val message =
                    value.optString("message").ifBlank {
                        "Room-control command failed"
                    }
                if (value.optString("command") == "respond") {
                    pendingOfferResponses[value.optString("offer_id")]
                        ?.delivered
                        ?.completeExceptionally(IllegalStateException(message))
                } else {
                    emit(
                        id,
                        RoomControlEvent.CommandFailed(
                            command = value.optString("command"),
                            offerId = value.optString("offer_id").takeIf(String::isNotBlank),
                            message = message,
                        ),
                    )
                }
            }
            "policy_changed" ->
                emit(id, RoomControlEvent.PolicyChanged(value.optString("policy").roomPolicy()))
            "closed" -> {
                failPendingResponses("The room closed before the response was delivered")
                connected = false
                activeSessionId = null
                val localReason = localCloseReason
                localCloseReason = null
                val nativeReason = value.optString("reason").roomCloseReason()
                emit(
                    id,
                    RoomControlEvent.Closed(
                        localReason
                            ?: if (nativeReason == RoomCloseReason.UserEnded) {
                                RoomCloseReason.PeerEnded
                            } else {
                                nativeReason
                            },
                    ),
                )
            }
            "failed" -> {
                failPendingResponses(
                    value.optString("message").ifBlank { "Room connection failed" },
                )
                connected = false
                activeSessionId = null
                Native.cancelRoomControlSession(id)
                emit(
                    id,
                    RoomControlEvent.Failed(
                        value.optString("message").ifBlank { "Room connection failed" },
                    ),
                )
            }
        }
    }

    private fun cancelCurrentLocked() {
        val id = activeSessionId
        if (id != null) {
            cancelGenerationLocked(id)
        } else {
            failPendingResponses("The room-control session was canceled")
            connected = false
            localCloseReason = null
        }
    }

    private fun cancelGenerationLocked(id: Long) {
        if (activeSessionId != id) return
        failPendingResponses("The room-control session was canceled")
        activeSessionId = null
        connected = false
        localCloseReason = null
        Native.cancelRoomControlSession(id)
    }

    private fun emit(
        sessionId: Long,
        event: RoomControlEvent,
    ) {
        mutableEvents.tryEmit(GeneratedRoomControlEvent(sessionId, event))
    }

    private fun failPendingResponses(message: String) {
        pendingOfferResponses.values.forEach {
            it.delivered.completeExceptionally(IllegalStateException(message))
        }
        pendingOfferResponses.clear()
    }

    private fun parseInviteResponse(json: String): RoomControlInvite {
        val value = JSONObject(json)
        value.optString("error").takeIf(String::isNotBlank)?.let(::error)
        return RoomControlInvite(
            code = value.getString("code"),
            payload = value.getString("payload"),
            expiresAtEpochMs = value.getLong("expires_at_epoch_ms"),
        )
    }

    private data class PendingOfferResponse(
        val accepted: Boolean,
        val delivered: CompletableDeferred<Unit>,
    )

    private data class GeneratedRoomControlEvent(
        val sessionId: Long,
        val event: RoomControlEvent,
    )

    private data class HostSettings(
        val displayName: String,
        val broker: String,
        val relay: String,
    )
}

private fun JSONObject.roomOffer(): RoomTransferOffer {
    val names = optJSONArray("root_names") ?: JSONArray()
    return RoomTransferOffer(
        id = getString("id"),
        transferInvite = getString("transfer_invite"),
        rootNames =
            (0 until names.length())
                .mapNotNull { index -> names.optString(index).takeIf(String::isNotBlank) }
                .take(3),
        itemCount = optInt("item_count").coerceAtLeast(0),
        totalBytes = optLong("total_bytes").coerceAtLeast(0L),
    )
}

private fun String.roomPolicy(): RoomLifetimePolicy =
    if (this == "until_foreground_ends") {
        RoomLifetimePolicy.UntilForegroundEnds
    } else {
        RoomLifetimePolicy.Idle15Minutes
    }

private fun RoomLifetimePolicy.wireValue(): String =
    when (this) {
        RoomLifetimePolicy.Idle15Minutes -> "idle_15_minutes"
        RoomLifetimePolicy.UntilForegroundEnds -> "until_foreground_ends"
    }

private fun String.roomCloseReason(): RoomCloseReason =
    when (this) {
        "idle_expired" -> RoomCloseReason.IdleExpired
        "invitation_expired" -> RoomCloseReason.InvitationExpired
        "peer_ended" -> RoomCloseReason.PeerEnded
        "backgrounded" -> RoomCloseReason.Backgrounded
        "network_lost" -> RoomCloseReason.NetworkLost
        "protocol_failure" -> RoomCloseReason.ProtocolFailure
        else -> RoomCloseReason.UserEnded
    }

private fun RoomCloseReason.wireValue(): String =
    when (this) {
        RoomCloseReason.UserEnded -> "user_ended"
        RoomCloseReason.IdleExpired -> "idle_expired"
        RoomCloseReason.InvitationExpired -> "invitation_expired"
        RoomCloseReason.PeerEnded -> "peer_ended"
        RoomCloseReason.Backgrounded -> "backgrounded"
        RoomCloseReason.NetworkLost -> "network_lost"
        RoomCloseReason.ProtocolFailure -> "protocol_failure"
    }
