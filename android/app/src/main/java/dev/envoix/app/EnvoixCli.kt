package dev.envoix.app

import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.launch
import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.EnvoixSession
import dev.envoix.app.ffi.FfiDataPathKind
import dev.envoix.app.ffi.FfiFailureCode
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiRendezvousPlan
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferEvent
import dev.envoix.app.ffi.FfiTransferEventKind
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.FfiTransferLimits
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.TransferObserver
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/** Parsed events from the Envoix core (see the client event stream schema). */
sealed interface CliEvent {
    data class InviteReady(val invite: String) : CliEvent
    data object Binding : CliEvent
    data object Connecting : CliEvent
    data class Connected(val pathType: String, val addr: String) : CliEvent
    data class Started(val transferId: String, val fileName: String, val totalBytes: Long) : CliEvent
    data class Progress(val bytesTransferred: Long, val totalBytes: Long) : CliEvent
    data class Completed(val bytesTransferred: Long) : CliEvent
    data class Failed(val error: String) : CliEvent
    data class CoreStatus(val message: String) : CliEvent
    data class Exit(val code: Int) : CliEvent
}

/**
 * Runs a transfer through the shared UniFFI core and exposes its callbacks as the
 * legacy Android [CliEvent] stream consumed by [TransferService].
 */
object UniffiTransferRunner {
    private const val ROOM_SEND_FALLBACK_TIMEOUT_MS = 38_000L

    private data class SessionEntry(val generation: Int, val session: EnvoixSession)

    private class ActiveTransfer(val activityId: String) {
        private val sessions = CopyOnWriteArrayList<SessionEntry>()

        fun add(generation: Int, session: EnvoixSession) {
            sessions.add(SessionEntry(generation, session))
        }

        fun cancelOlderThan(generation: Int) {
            sessions.filter { it.generation < generation }.forEach { entry ->
                entry.session.cancelActivity(activityId)
            }
        }

        fun cancelAll() {
            sessions.forEach { entry -> entry.session.cancelActivity(activityId) }
        }
    }

    private val active = ConcurrentHashMap<Long, ActiveTransfer>()

    fun run(
        id: Long,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        qrPayload: String?,
        transferInvite: String?,
        internetAvailable: Boolean,
        useRoom: Boolean,
        useMdns: Boolean,
    ): Flow<CliEvent> = callbackFlow {
        val activityId = "android-$id"
        val settings = EnvoixRuntimeSettings(
            concurrentTransfers = true,
            language = "en",
            serverUrl = broker,
            relayUrl = relay,
            configPath = configPath,
            speedLimitMbps = 40uL,
        )
        val activeTransfer = ActiveTransfer(activityId)
        val terminal = AtomicBoolean(false)
        val activeGeneration = AtomicInteger(0)
        var fallbackJob: Job? = null

        fun newSession(): EnvoixSession = EnvoixSession.newWithSettings(settings)

        fun isCurrent(generation: Int): Boolean =
            generation == activeGeneration.get() && !terminal.get()

        fun complete(bytes: ULong, generation: Int) {
            if (!isCurrent(generation)) return
            if (terminal.compareAndSet(false, true)) {
                fallbackJob?.cancel()
                active.remove(id)?.cancelAll()
                trySend(CliEvent.Completed(bytes.toLongSaturated()))
                trySend(CliEvent.Exit(0))
                close()
            }
        }

        fun fail(message: String, generation: Int) {
            if (!isCurrent(generation)) return
            if (terminal.compareAndSet(false, true)) {
                fallbackJob?.cancel()
                active.remove(id)?.cancelAll()
                trySend(CliEvent.Failed(message.ifBlank { "transfer failed" }))
                trySend(CliEvent.Exit(1))
                close()
            }
        }

        fun observerFor(generation: Int) = object : TransferObserver {
            override fun onInviteReady(invite: String) {
                if (!isCurrent(generation)) return
                trySend(CliEvent.InviteReady(invite))
            }

            override fun onStarted(fileName: String, totalBytes: ULong) = Unit

            override fun onProgress(transferred: ULong, total: ULong) = Unit

            override fun onCompleted(bytes: ULong) = complete(bytes, generation)

            override fun onTransferFailed(failure: FfiTransferFailure) = fail(failure.message(), generation)

            override fun onFailed(reason: String) = fail(reason, generation)

            override fun onTransferEvent(event: FfiTransferEvent) {
                if (!isCurrent(generation)) return
                if (event.mode == FfiTransferMode.MDNS) {
                    fallbackJob?.cancel()
                }
                mapEvent(event)?.let { trySend(it) }
            }

            override fun onTransferActivity(record: dev.envoix.app.ffi.FfiTransferActivityRecord) = Unit

            override fun onStatus(message: String) {
                if (!isCurrent(generation)) return
                if (message.isNotBlank()) {
                    if (message.contains("mDNS")) {
                        fallbackJob?.cancel()
                    }
                    LogStore.append("core: $message")
                    trySend(CliEvent.CoreStatus(message))
                }
            }
        }

        fun startAttempt(generation: Int, attemptUseRoom: Boolean, attemptUseMdns: Boolean) {
            if (terminal.get()) return
            val session = newSession()
            activeTransfer.add(generation, session)
            active[id] = activeTransfer
            val started = runCatching {
                session.startTransfer(
                    transferRequest(
                        activityId = activityId,
                        direction = direction,
                        code = code,
                        broker = broker,
                        relay = relay,
                        path = path,
                        configPath = configPath,
                        qrPayload = qrPayload,
                        transferInvite = transferInvite,
                        internetAvailable = internetAvailable,
                        useRoom = attemptUseRoom,
                        useMdns = attemptUseMdns,
                    ),
                    observerFor(generation),
                )
            }
            started.exceptionOrNull()?.let { error ->
                fail(error.message ?: "native error", generation)
            }
        }

        active[id] = activeTransfer
        startAttempt(generation = 0, attemptUseRoom = useRoom, attemptUseMdns = useMdns)

        if (shouldRestartRoomSenderWithMdns(direction, transferInvite, internetAvailable, useRoom, useMdns)) {
            fallbackJob = launch {
                delay(ROOM_SEND_FALLBACK_TIMEOUT_MS)
                if (!activeGeneration.compareAndSet(0, 1) || terminal.get()) return@launch
                val message = "room sender did not connect within ${ROOM_SEND_FALLBACK_TIMEOUT_MS / 1000}s; restarting with mDNS"
                LogStore.append("core: $message")
                trySend(CliEvent.CoreStatus(message))
                activeTransfer.cancelOlderThan(1)
                startAttempt(generation = 1, attemptUseRoom = false, attemptUseMdns = true)
            }
        }

        awaitClose {
            fallbackJob?.cancel()
            if (terminal.compareAndSet(false, true)) {
                active.remove(id)?.cancelAll()
            } else {
                active.remove(id)
            }
        }
    }

    fun cancel(id: Long) {
        val transfer = active[id] ?: return
        transfer.cancelAll()
    }

    private fun shouldRestartRoomSenderWithMdns(
        direction: String,
        transferInvite: String?,
        internetAvailable: Boolean,
        useRoom: Boolean,
        useMdns: Boolean,
    ): Boolean =
        direction == "send" &&
            transferInvite.isNullOrBlank() &&
            internetAvailable &&
            useRoom &&
            useMdns

    private fun transferRequest(
        activityId: String,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        qrPayload: String?,
        transferInvite: String?,
        internetAvailable: Boolean,
        useRoom: Boolean,
        useMdns: Boolean,
    ): FfiTransferRequest {
        val ffiDirection = when (direction) {
            "send" -> FfiTransferDirection.SEND
            "receive" -> FfiTransferDirection.RECEIVE
            else -> throw IllegalArgumentException("unsupported transfer direction: $direction")
        }
        val invite = transferInvite.orEmpty()
        val mode = transferMode(
            direction = ffiDirection,
            invite = invite,
            useRoom = useRoom,
            useMdns = useMdns,
        )
        return FfiTransferRequest(
            activityId = activityId,
            direction = ffiDirection,
            mode = mode,
            filePath = if (ffiDirection == FfiTransferDirection.SEND) path else "",
            outputDir = if (ffiDirection == FfiTransferDirection.RECEIVE) path else "",
            peerDescriptor = "",
            invite = invite,
            code = if (mode == FfiTransferMode.ROOM) code else "",
            token = if (mode == FfiTransferMode.ROOM) code else "",
            broker = broker,
            relay = relay,
            configPath = configPath,
            pathPolicy = FfiPathPolicy.AUTO,
            resume = true,
            limits = FfiTransferLimits(
                maxParallelTransfers = 1u,
                maxParallelFiles = 1u,
                maxParallelChunksPerFile = 1u,
                speedLimitBps = 0uL,
            ),
            rendezvous = FfiRendezvousPlan(
                useRoom = mode == FfiTransferMode.ROOM && useRoom,
                useMdns = mode == FfiTransferMode.ROOM && useMdns,
                internetAvailable = internetAvailable,
            ),
        )
    }

    private fun transferMode(
        direction: FfiTransferDirection,
        invite: String,
        useRoom: Boolean,
        useMdns: Boolean,
    ): FfiTransferMode =
        when {
            direction == FfiTransferDirection.SEND && invite.isNotBlank() -> FfiTransferMode.INVITE
            direction == FfiTransferDirection.RECEIVE && !useRoom && !useMdns -> FfiTransferMode.SHOW_INVITE
            else -> FfiTransferMode.ROOM
        }

    private fun mapEvent(event: FfiTransferEvent): CliEvent? =
        when (event.kind) {
            FfiTransferEventKind.BINDING,
            FfiTransferEventKind.ADVERTISED,
            FfiTransferEventKind.PAIRING -> CliEvent.Binding
            FfiTransferEventKind.CONNECTING -> CliEvent.Connecting
            FfiTransferEventKind.CONNECTED,
            FfiTransferEventKind.PATH_CHANGED -> CliEvent.Connected(
                pathType(event.dataPathKind),
                event.dataPathDetail,
            )
            FfiTransferEventKind.STARTED -> CliEvent.Started(
                event.transferId,
                event.fileName,
                event.totalBytes.toLongSaturated(),
            )
            FfiTransferEventKind.PROGRESS -> CliEvent.Progress(
                event.bytesTransferred.toLongSaturated(),
                event.totalBytes.toLongSaturated(),
            )
            else -> null
        }

    private fun pathType(kind: FfiDataPathKind): String =
        when (kind) {
            FfiDataPathKind.DIRECT -> "direct"
            FfiDataPathKind.RELAY -> "relay"
            FfiDataPathKind.OTHER -> "other"
            FfiDataPathKind.NONE -> ""
        }

    private fun FfiTransferFailure.message(): String =
        diagnosticMessage.ifBlank {
            userMessageKey.ifBlank {
                if (code == FfiFailureCode.UNKNOWN) "transfer failed" else code.name.lowercase()
            }
        }

    private fun ULong.toLongSaturated(): Long =
        if (this > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else toLong()
}
