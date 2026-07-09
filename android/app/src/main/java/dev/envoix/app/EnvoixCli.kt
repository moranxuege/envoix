package dev.envoix.app

import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
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
import java.util.concurrent.atomic.AtomicBoolean

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
    data class Exit(val code: Int) : CliEvent
}

/**
 * Runs a transfer through the shared UniFFI core and exposes its callbacks as the
 * legacy Android [CliEvent] stream consumed by [TransferService].
 */
object NativeTransfer {
    private data class ActiveTransfer(val activityId: String, val session: EnvoixSession)

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
        val session = EnvoixSession.newWithSettings(
            EnvoixRuntimeSettings(
                concurrentTransfers = true,
                language = "en",
                serverUrl = broker,
                relayUrl = relay,
                configPath = configPath,
                speedLimitMbps = 40uL,
            )
        )
        val terminal = AtomicBoolean(false)

        fun complete(bytes: ULong) {
            if (terminal.compareAndSet(false, true)) {
                active.remove(id)
                trySend(CliEvent.Completed(bytes.toLongSaturated()))
                trySend(CliEvent.Exit(0))
                close()
            }
        }

        fun fail(message: String) {
            if (terminal.compareAndSet(false, true)) {
                active.remove(id)
                trySend(CliEvent.Failed(message.ifBlank { "transfer failed" }))
                trySend(CliEvent.Exit(1))
                close()
            }
        }

        val observer = object : TransferObserver {
            override fun onInviteReady(invite: String) {
                trySend(CliEvent.InviteReady(invite))
            }

            override fun onStarted(fileName: String, totalBytes: ULong) = Unit

            override fun onProgress(transferred: ULong, total: ULong) = Unit

            override fun onCompleted(bytes: ULong) = complete(bytes)

            override fun onTransferFailed(failure: FfiTransferFailure) = fail(failure.message())

            override fun onFailed(reason: String) = fail(reason)

            override fun onTransferEvent(event: FfiTransferEvent) {
                mapEvent(event)?.let { trySend(it) }
            }

            override fun onTransferActivity(record: dev.envoix.app.ffi.FfiTransferActivityRecord) = Unit

            override fun onStatus(message: String) {
                if (message.isNotBlank()) LogStore.append("core: $message")
            }
        }

        active[id] = ActiveTransfer(activityId, session)
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
                    useRoom = useRoom,
                    useMdns = useMdns,
                ),
                observer,
            )
        }
        started.exceptionOrNull()?.let { error ->
            fail(error.message ?: "native error")
        }

        awaitClose {
            if (terminal.compareAndSet(false, true)) {
                active.remove(id)?.let { transfer ->
                    transfer.session.cancelActivity(transfer.activityId)
                }
            } else {
                active.remove(id)
            }
        }
    }

    fun cancel(id: Long) {
        val transfer = active[id] ?: return
        transfer.session.cancelActivity(transfer.activityId)
    }

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
        val mode = if (invite.isNotBlank()) FfiTransferMode.INVITE else FfiTransferMode.ROOM
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
