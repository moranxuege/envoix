package dev.envoix.app

import android.content.Context
import dev.envoix.app.ffi.DurableEnvoixSession
import dev.envoix.app.ffi.EnvoixRuntimeSettings
import dev.envoix.app.ffi.FfiPathPolicy
import dev.envoix.app.ffi.FfiRendezvousPlan
import dev.envoix.app.ffi.FfiTransferActivityRecord
import dev.envoix.app.ffi.FfiTransferDirection
import dev.envoix.app.ffi.FfiTransferEvent
import dev.envoix.app.ffi.FfiTransferFailure
import dev.envoix.app.ffi.FfiTransferLimits
import dev.envoix.app.ffi.FfiTransferMode
import dev.envoix.app.ffi.FfiTransferRequest
import dev.envoix.app.ffi.MailboxObserverV2
import dev.envoix.app.ffi.TransferObserver
import dev.envoix.app.ffi.listDurableTransferRecords
import dev.envoix.app.ffi.restoreDurableTransferV2
import dev.envoix.app.ffi.startDurableTransferV2
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.io.File
import java.net.URL
import java.util.concurrent.ConcurrentHashMap

sealed interface DurableUpdate {
    data class Activity(
        val record: FfiTransferActivityRecord,
    ) : DurableUpdate

    data class InviteReady(
        val invite: String,
    ) : DurableUpdate

    data class Event(
        val event: FfiTransferEvent,
    ) : DurableUpdate

    data class Status(
        val message: String,
    ) : DurableUpdate
}

/** Owns one canonical durable Rust session per Android Activity card. */
object UniffiTransferRunner {
    private data class Entry(
        val session: DurableEnvoixSession,
        val observer: TransferObserver,
    )

    private val sessions = ConcurrentHashMap<Long, Entry>()
    private val callbacks = ConcurrentHashMap<Long, (DurableUpdate) -> Unit>()
    private val ioScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private lateinit var recordsDir: File

    private val mailbox =
        object : MailboxObserverV2 {
            override fun onFetchReceipt(
                activityId: String,
                key: String,
                server: String?,
            ) {
                val id = parseActivityId(activityId) ?: return
                val endpoint = receiptEndpoint(server) ?: return
                if (!validMailboxKey(key)) return
                ioScope.launch {
                    val data = LogUpload.getBytes("$endpoint/receipts/$key") ?: byteArrayOf()
                    sessions[id]?.session?.receiptResponse(data)
                }
            }

            override fun onPostReceipt(
                activityId: String,
                key: String,
                blob: ByteArray,
                server: String?,
            ) {
                val id = parseActivityId(activityId) ?: return
                val endpoint = receiptEndpoint(server) ?: return
                if (!validMailboxKey(key)) return
                ioScope.launch {
                    val delaysMs = longArrayOf(0, 1_000, 3_000, 10_000, 30_000)
                    for (delayMs in delaysMs) {
                        if (delayMs > 0) delay(delayMs)
                        if (LogUpload.postBytes("$endpoint/receipts/$key", blob)) {
                            sessions[id]?.session?.receiptPosted()
                            return@launch
                        }
                    }
                }
            }
        }

    fun initialize(context: Context) {
        recordsDir = File(context.filesDir, "transfer-records").apply { mkdirs() }
    }

    fun records(): List<FfiTransferActivityRecord> {
        check(::recordsDir.isInitialized) { "UniffiTransferRunner is not initialized" }
        return listDurableTransferRecords(recordsDir.absolutePath)
    }

    fun start(
        id: Long,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        transferInvite: String?,
        internetAvailable: Boolean,
        useRoom: Boolean,
        useMdns: Boolean,
        receiptServer: String,
        onUpdate: (DurableUpdate) -> Unit,
        pathPolicy: FfiPathPolicy = FfiPathPolicy.AUTO,
        publicationRequired: Boolean = direction == "receive",
    ): Boolean {
        val settings =
            EnvoixRuntimeSettings(
                concurrentTransfers = true,
                language = "en",
                serverUrl = broker,
                relayUrl = relay,
                configPath = configPath,
                speedLimitMbps = 40uL,
            )
        val request =
            transferRequest(
                activityId = activityId(id),
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
            )
        callbacks[id] = onUpdate
        val observer = observer(id)
        return runCatching {
            val session =
                startDurableTransferV2(
                    settings = settings,
                    request = request,
                    recordsDir = recordsDir.absolutePath,
                    receiptServer = receiptServer,
                    observer = observer,
                    mailbox = mailbox,
                )
            sessions[id] = Entry(session, observer)
            emit(id, DurableUpdate.Activity(session.activity()))
        }.onFailure {
            callbacks.remove(id)
            LogStore.append("core: durable start failed id=$id: ${it.message}")
        }.isSuccess
    }

    fun restore(
        id: Long,
        onUpdate: (DurableUpdate) -> Unit,
    ): Boolean {
        callbacks[id] = onUpdate
        val observer = observer(id)
        return runCatching {
            val session =
                restoreDurableTransferV2(
                    activityId = activityId(id),
                    recordsDir = recordsDir.absolutePath,
                    observer = observer,
                    mailbox = mailbox,
                )
            sessions[id] = Entry(session, observer)
            emit(id, DurableUpdate.Activity(session.activity()))
        }.onFailure {
            callbacks.remove(id)
            LogStore.append("core: durable restore failed id=$id: ${it.message}")
        }.isSuccess
    }

    fun pause(id: Long): Boolean = sessions[id]?.session?.pause() == true

    fun hasSession(id: Long): Boolean = sessions.containsKey(id)

    fun activity(id: Long): FfiTransferActivityRecord? = sessions[id]?.session?.activity()

    fun resume(id: Long): Boolean = sessions[id]?.session?.resume() == true

    fun cancel(id: Long): Boolean = sessions[id]?.session?.cancel() == true

    fun publicationSucceeded(
        id: Long,
        uri: String,
    ): Boolean = sessions[id]?.session?.publicationSucceeded(uri) == true

    fun attach(
        id: Long,
        onUpdate: (DurableUpdate) -> Unit,
    ) {
        callbacks[id] = onUpdate
    }

    fun detach(id: Long) {
        callbacks.remove(id)
    }

    fun remove(id: Long): Boolean {
        val entry = sessions.remove(id) ?: return false
        callbacks.remove(id)
        val removed = entry.session.remove()
        entry.session.close()
        return removed
    }

    private fun observer(id: Long): TransferObserver =
        object : TransferObserver {
            override fun onInviteReady(invite: String) {
                emit(id, DurableUpdate.InviteReady(invite))
            }

            override fun onStarted(
                fileName: String,
                totalBytes: ULong,
            ) = Unit

            override fun onProgress(
                transferred: ULong,
                total: ULong,
            ) = Unit

            override fun onCompleted(bytes: ULong) = Unit

            override fun onTransferFailed(failure: FfiTransferFailure) = Unit

            override fun onFailed(reason: String) = Unit

            override fun onTransferEvent(event: FfiTransferEvent) {
                emit(id, DurableUpdate.Event(event))
            }

            override fun onTransferActivity(record: FfiTransferActivityRecord) {
                emit(id, DurableUpdate.Activity(record))
            }

            override fun onStatus(message: String) {
                if (message.isNotBlank()) emit(id, DurableUpdate.Status(message))
            }
        }

    private fun emit(
        id: Long,
        update: DurableUpdate,
    ) {
        callbacks[id]?.invoke(update)
    }

    private fun transferRequest(
        activityId: String,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        transferInvite: String?,
        internetAvailable: Boolean,
        useRoom: Boolean,
        useMdns: Boolean,
        pathPolicy: FfiPathPolicy,
        publicationRequired: Boolean,
    ): FfiTransferRequest {
        val ffiDirection =
            when (direction) {
                "send" -> FfiTransferDirection.SEND
                "receive" -> FfiTransferDirection.RECEIVE
                else -> throw IllegalArgumentException("unsupported transfer direction: $direction")
            }
        val invite = transferInvite.orEmpty()
        val mode =
            when {
                ffiDirection == FfiTransferDirection.SEND && invite.isNotBlank() -> FfiTransferMode.INVITE
                ffiDirection == FfiTransferDirection.RECEIVE && !useRoom && !useMdns -> FfiTransferMode.SHOW_INVITE
                else -> FfiTransferMode.ROOM
            }
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
            pathPolicy = pathPolicy,
            resume = true,
            publicationRequired = ffiDirection == FfiTransferDirection.RECEIVE && publicationRequired,
            limits =
                FfiTransferLimits(
                    maxParallelTransfers = 2u,
                    maxParallelFiles = 1u,
                    maxParallelChunksPerFile = 1u,
                    speedLimitBps = 0uL,
                ),
            rendezvous =
                FfiRendezvousPlan(
                    useRoom = mode == FfiTransferMode.ROOM && useRoom,
                    useMdns = mode == FfiTransferMode.ROOM && useMdns,
                    internetAvailable = internetAvailable,
                ),
        )
    }

    private fun activityId(id: Long): String = "android-$id"

    fun parseActivityId(activityId: String): Long? =
        activityId
            .removePrefix("android-")
            .takeIf { activityId.startsWith("android-") }
            ?.toLongOrNull()

    private fun validMailboxKey(key: String): Boolean = key.length in 1..128 && key.all { it in '0'..'9' || it in 'a'..'f' }

    private fun receiptEndpoint(server: String?): String? {
        val candidate =
            server
                ?.trim()
                .orEmpty()
                .ifEmpty {
                    SettingsStore.settings.value.logServer
                        .trim()
                }
        val parsed = runCatching { URL(candidate) }.getOrNull() ?: return null
        if (parsed.protocol != "http" && parsed.protocol != "https") return null
        if (parsed.host.isNullOrBlank()) return null
        return candidate.trimEnd('/')
    }
}
