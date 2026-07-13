package dev.envoix.app

import dev.envoix.app.ffi.FfiDataPathKind
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiTransferActivityState
import dev.envoix.app.ffi.FfiTransferEventKind
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow

/** Compatibility vocabulary for existing device tests; production uses canonical snapshots directly. */
sealed interface CliEvent {
    data class InviteReady(
        val invite: String,
    ) : CliEvent

    data object Binding : CliEvent

    data object Connecting : CliEvent

    data class Connected(
        val pathType: String,
        val addr: String,
    ) : CliEvent

    data class Started(
        val transferId: String,
        val fileName: String,
        val totalBytes: Long,
    ) : CliEvent

    data class Progress(
        val bytesTransferred: Long,
        val totalBytes: Long,
    ) : CliEvent

    data class Publishing(
        val fileName: String,
        val stagedPath: String,
        val bytesTransferred: Long,
    ) : CliEvent

    data object Verifying : CliEvent

    data object Confirming : CliEvent

    data object Paused : CliEvent

    data class Completed(
        val bytesTransferred: Long,
    ) : CliEvent

    data class Failed(
        val error: String,
    ) : CliEvent

    data class CoreStatus(
        val message: String,
    ) : CliEvent

    data class Exit(
        val code: Int,
    ) : CliEvent
}

@Suppress("UNUSED_PARAMETER")
fun UniffiTransferRunner.run(
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
    pathPolicy: FfiPathPolicy = FfiPathPolicy.AUTO,
    publicationRequired: Boolean = false,
): Flow<CliEvent> =
    callbackFlow {
        val started =
            start(
                id = id,
                direction = direction,
                code = code,
                broker = broker,
                relay = relay,
                path = path,
                configPath = configPath,
                transferInvite = transferInvite,
                internetAvailable = internetAvailable,
                useRoom = useRoom,
                useMdns = useMdns,
                pathPolicy = pathPolicy,
                publicationRequired = publicationRequired,
                onUpdate = { update ->
                    when (update) {
                        is DurableUpdate.InviteReady -> trySend(CliEvent.InviteReady(update.invite))
                        is DurableUpdate.Status -> trySend(CliEvent.CoreStatus(update.message))
                        is DurableUpdate.Event -> {
                            val event = update.event
                            when (event.kind) {
                                FfiTransferEventKind.BINDING,
                                FfiTransferEventKind.ADVERTISED,
                                FfiTransferEventKind.PAIRING,
                                -> trySend(CliEvent.Binding)
                                FfiTransferEventKind.CONNECTING -> trySend(CliEvent.Connecting)
                                FfiTransferEventKind.CONNECTED,
                                FfiTransferEventKind.PATH_CHANGED,
                                ->
                                    trySend(
                                        CliEvent.Connected(
                                            pathType = event.dataPathKind.testDisplayName(),
                                            addr = event.dataPathDetail,
                                        ),
                                    )
                                FfiTransferEventKind.STARTED ->
                                    trySend(
                                        CliEvent.Started(
                                            transferId = event.transferId,
                                            fileName = event.fileName,
                                            totalBytes = event.totalBytes.toLongSaturatedForTest(),
                                        ),
                                    )
                                FfiTransferEventKind.PROGRESS ->
                                    trySend(
                                        CliEvent.Progress(
                                            bytesTransferred = event.bytesTransferred.toLongSaturatedForTest(),
                                            totalBytes = event.totalBytes.toLongSaturatedForTest(),
                                        ),
                                    )
                                FfiTransferEventKind.VERIFYING -> trySend(CliEvent.Verifying)
                                else -> Unit
                            }
                        }
                        is DurableUpdate.Activity -> {
                            val record = update.record
                            when (record.state) {
                                FfiTransferActivityState.COMPLETED -> {
                                    trySend(CliEvent.Completed(record.bytesTransferred.toLongSaturatedForTest()))
                                    trySend(CliEvent.Exit(0))
                                    close()
                                }
                                FfiTransferActivityState.FAILED,
                                FfiTransferActivityState.CANCELED,
                                -> {
                                    trySend(CliEvent.Failed(record.diagnosticMessage.ifBlank { "transfer failed" }))
                                    trySend(CliEvent.Exit(1))
                                    close()
                                }
                                FfiTransferActivityState.VERIFYING ->
                                    trySend(
                                        if (record.diagnosticMessage == "confirming") {
                                            CliEvent.Confirming
                                        } else {
                                            CliEvent.Verifying
                                        },
                                    )
                                FfiTransferActivityState.PUBLISHING ->
                                    trySend(
                                        CliEvent.Publishing(
                                            fileName = record.fileName,
                                            stagedPath = record.completedFilePath,
                                            bytesTransferred = record.bytesTransferred.toLongSaturatedForTest(),
                                        ),
                                    )
                                FfiTransferActivityState.PAUSED -> trySend(CliEvent.Paused)
                                else -> Unit
                            }
                        }
                    }
                },
            )
        if (!started) {
            trySend(CliEvent.Failed("durable test session failed to start"))
            trySend(CliEvent.Exit(1))
            close()
        }
        awaitClose { detach(id) }
    }

private fun FfiDataPathKind.testDisplayName(): String =
    when (this) {
        FfiDataPathKind.DIRECT -> "direct"
        FfiDataPathKind.RELAY -> "relay"
        FfiDataPathKind.OTHER -> "other"
        FfiDataPathKind.NONE -> ""
    }

private fun ULong.toLongSaturatedForTest(): Long = if (this > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else toLong()
