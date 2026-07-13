package dev.envoix.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import dev.envoix.app.ffi.FfiDataPathKind
import dev.envoix.app.ffi.FfiTransferActivityRecord
import dev.envoix.app.ffi.FfiTransferActivityState
import dev.envoix.app.ffi.FfiTransferDirection
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap

/** Foreground owner for canonical durable transfers and native file publication. */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val activeIds = ConcurrentHashMap.newKeySet<Long>()
    private val publishingIds = ConcurrentHashMap.newKeySet<Long>()
    private val multicastHolders = ConcurrentHashMap.newKeySet<Long>()
    private val observedIds = ConcurrentHashMap.newKeySet<Long>()
    private val specs = ConcurrentHashMap<Long, TransferSpec>()
    private val logTime = SimpleDateFormat("HH:mm:ss", Locale.US)

    private val multicastLock by lazy {
        (getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager)
            .createMulticastLock("envoix-mdns")
            .apply { setReferenceCounted(true) }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        UniffiTransferRunner.initialize(this)
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Transfers", NotificationManager.IMPORTANCE_LOW),
        )
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        when (intent?.action) {
            ACTION_RESTORE -> {
                enterForeground()
                restoreSessions()
            }
            ACTION_START -> startNew(intent)
            ACTION_RESUME -> resume(intent.getLongExtra(EXTRA_ID, -1L))
            ACTION_PAUSE -> pause(intent.getLongExtra(EXTRA_ID, -1L))
            ACTION_CANCEL -> cancel(intent.getLongExtra(EXTRA_ID, -1L))
            ACTION_REMOVE -> remove(intent.getLongExtra(EXTRA_ID, -1L))
        }
        return START_NOT_STICKY
    }

    private fun startNew(intent: Intent) {
        val direction = intent.getStringExtra(EXTRA_DIRECTION) ?: return
        val room = intent.getStringExtra(EXTRA_ROOM) ?: return
        val requestedPath = intent.getStringExtra(EXTRA_PATH) ?: return
        enterForeground()
        val id = TransferRepository.create(direction.toDirection(), room)
        val path =
            if (direction == "receive") {
                File(requestedPath, "android-$id").apply { mkdirs() }.absolutePath
            } else {
                requestedPath
            }
        val settings = SettingsStore.settings.value
        val internetAvailable = hasInternet()
        val spec =
            TransferSpec(
                direction = direction,
                room = room,
                path = path,
                broker = intent.getStringExtra(EXTRA_BROKER) ?: Endpoints.BROKER,
                relay = intent.getStringExtra(EXTRA_RELAY) ?: Endpoints.RELAY,
                config = intent.getStringExtra(EXTRA_CONFIG) ?: "",
                qrPayload = intent.getStringExtra(EXTRA_QR),
                transferInvite = intent.getStringExtra(EXTRA_TRANSFER_INVITE),
                internetAvailable = internetAvailable,
                useRoom = settings.useRoom && internetAvailable,
                useMdns = settings.useMdns,
                saveTreeUri = settings.saveTreeUri,
                saveFolder = settings.saveFolder,
            )
        specs[id] = spec
        TransferSpecStore.save(this, id, spec)
        TransferRepository.update(id) {
            it.copy(
                qrPayload = spec.qrPayload,
                fileName = if (spec.dir() == Direction.Send) File(spec.path).name else null,
            )
        }
        observe(id)
        markActive(id, spec)
        val started =
            UniffiTransferRunner.start(
                id = id,
                direction = spec.direction,
                code = spec.room,
                broker = spec.broker,
                relay = spec.relay,
                path = spec.path,
                configPath = spec.config,
                transferInvite = spec.transferInvite,
                internetAvailable = spec.internetAvailable,
                useRoom = spec.useRoom,
                useMdns = spec.useMdns,
                onUpdate = updateCallback(id),
            )
        if (!started) {
            activeIds.remove(id)
            releaseMulticast(id)
            TransferRepository.update(id) {
                it.copy(status = Status.Failed, error = "Could not start the durable transfer session")
            }
            stopIfIdle()
        }
    }

    private fun restoreSessions() {
        val records =
            runCatching { UniffiTransferRunner.records() }.getOrElse {
                LogStore.append("app: durable restore scan failed: ${it.message}")
                stopIfIdle()
                return
            }
        records.forEach { record ->
            val id = UniffiTransferRunner.parseActivityId(record.activityId) ?: return@forEach
            TransferSpecStore.load(this, id)?.also {
                specs[id] = it
            }
            applyActivity(id, record)
            observe(id)
            if (UniffiTransferRunner.hasSession(id)) {
                UniffiTransferRunner.attach(id, updateCallback(id))
                UniffiTransferRunner.activity(id)?.let { applyActivity(id, it) }
            } else {
                if (UniffiTransferRunner.restore(id, updateCallback(id))) {
                    UniffiTransferRunner.activity(id)?.let { applyActivity(id, it) }
                }
            }
        }
        stopIfIdle()
    }

    private fun observe(id: Long) {
        observedIds.add(id)
        UniffiTransferRunner.attach(id, updateCallback(id))
    }

    private fun updateCallback(id: Long): (DurableUpdate) -> Unit =
        { update ->
            scope.launch { handleUpdate(id, update) }
        }

    private fun handleUpdate(
        id: Long,
        update: DurableUpdate,
    ) {
        when (update) {
            is DurableUpdate.Activity -> applyActivity(id, update.record)
            is DurableUpdate.InviteReady ->
                TransferRepository.update(id) {
                    it.copy(qrPayload = update.invite, log = addLog(it.log, "invite ready"))
                }
            is DurableUpdate.Event ->
                TransferRepository.update(id) {
                    val event = update.event
                    val detail = event.diagnosticMessage.ifBlank { event.kind.name.lowercase() }
                    it.copy(log = addLog(it.log, "core · $detail"))
                }
            is DurableUpdate.Status -> {
                LogStore.append("core: ${update.message}")
                TransferRepository.update(id) {
                    it.copy(log = addLog(it.log, "core · ${update.message}"))
                }
            }
        }
    }

    private fun applyActivity(
        id: Long,
        record: FfiTransferActivityRecord,
    ) {
        val current = TransferRepository.transfers.value.firstOrNull { it.id == id }
        val sequence = record.sequence.toLongSaturated()
        if (current != null && current.sequence > sequence) return
        val spec = specs[id] ?: TransferSpecStore.load(this, id)?.also { specs[id] = it }
        val status = record.toStatus()
        val room = current?.room ?: spec?.room ?: record.token.ifBlank { "restored" }
        val direction =
            when (record.direction) {
                FfiTransferDirection.SEND -> Direction.Send
                FfiTransferDirection.RECEIVE -> Direction.Receive
                FfiTransferDirection.UNKNOWN -> current?.direction ?: spec?.dir() ?: Direction.Receive
            }
        val speed = if (status == Status.Transferring) record.speedBps.toLongSaturated().toDouble() else 0.0
        val previousHistory = current?.speedHistory.orEmpty()
        val history = if (speed > 0) (previousHistory + speed).takeLast(90) else previousHistory
        val stateChanged = current?.status != status
        val error =
            if (status == Status.Failed || status == Status.Unconfirmed || status == Status.Publishing) {
                record.diagnosticMessage.takeIf { it.isNotBlank() } ?: current?.error
            } else {
                null
            }
        val savedUri =
            record.completedFilePath.takeIf { status == Status.Completed && it.startsWith("content://") }
                ?: current?.savedUri
        val log =
            if (stateChanged) {
                addLog(current?.log.orEmpty(), "state · ${status.name.lowercase()}")
            } else {
                current?.log.orEmpty()
            }
        TransferRepository.upsert(
            Transfer(
                id = id,
                sequence = sequence,
                direction = direction,
                room = room,
                fileName = record.fileName.takeIf { it.isNotBlank() } ?: current?.fileName,
                attempt = record.attemptId.substringAfterLast('-').toIntOrNull() ?: current?.attempt ?: 1,
                proofDelivered = direction == Direction.Receive && status == Status.Completed,
                transferId = record.transferId.takeIf { it.isNotBlank() } ?: current?.transferId,
                pathType = record.dataPathKind.displayName(),
                pathAddr = record.dataPathDetail,
                bytes = record.bytesTransferred.toLongSaturated(),
                total = record.totalBytes.toLongSaturated(),
                speedBps = speed,
                avgBps = record.averageSpeedBps.toLongSaturated().toDouble(),
                status = status,
                retryable = record.retryable,
                error = error,
                savedUri = savedUri,
                qrPayload = record.invite.takeIf { it.isNotBlank() } ?: current?.qrPayload ?: spec?.qrPayload,
                speedHistory = history,
                log = log,
            ),
        )

        if (status.needsForeground()) {
            spec?.let { markActive(id, it) }
        } else {
            activeIds.remove(id)
            releaseMulticast(id)
        }
        when (status) {
            Status.Publishing -> publishReceived(id, record)
            Status.Completed, Status.Cancelled -> cleanupCompletedNativeFiles(id, spec)
            else -> Unit
        }
        updateNotification()
        stopIfIdle()
    }

    private fun resume(id: Long) {
        if (id < 0) return
        val spec = specs[id] ?: TransferSpecStore.load(this, id)?.also { specs[id] = it } ?: return
        enterForeground()
        observe(id)
        val status =
            TransferRepository.transfers.value
                .firstOrNull { it.id == id }
                ?.status
        if (status == Status.Publishing) {
            UniffiTransferRunner.activity(id)?.let { publishReceived(id, it) }
        } else if (UniffiTransferRunner.resume(id)) {
            markActive(id, spec)
        }
    }

    private fun pause(id: Long) {
        if (id < 0) return
        observe(id)
        UniffiTransferRunner.pause(id)
    }

    private fun cancel(id: Long) {
        if (id < 0) return
        observe(id)
        UniffiTransferRunner.cancel(id)
    }

    private fun remove(id: Long) {
        val transfer = TransferRepository.transfers.value.firstOrNull { it.id == id } ?: return
        if (!transfer.status.isTerminal) return
        val spec = specs.remove(id) ?: TransferSpecStore.load(this, id)
        if (UniffiTransferRunner.remove(id)) {
            cleanupRemovedNativeFiles(spec)
            TransferSpecStore.remove(this, id)
            TransferRepository.remove(id)
            observedIds.remove(id)
        }
        stopIfIdle()
    }

    private fun publishReceived(
        id: Long,
        record: FfiTransferActivityRecord,
    ) {
        if (!publishingIds.add(id)) return
        val spec = specs[id] ?: TransferSpecStore.load(this, id)
        val source = File(record.completedFilePath)
        val name = record.fileName
        scope.launch {
            val uri =
                if (spec == null || name.isBlank() || !source.isFile) {
                    null
                } else {
                    withContext(Dispatchers.IO) {
                        MediaStoreSaver.saveReceived(
                            context = this@TransferService,
                            source = source,
                            displayName = name,
                            treeUri = spec.saveTreeUri,
                            folder = spec.saveFolder,
                        )
                    }
                }
            publishingIds.remove(id)
            if (uri != null && UniffiTransferRunner.publicationSucceeded(id, uri.toString())) {
                withContext(Dispatchers.IO) {
                    source.delete()
                    source.parentFile?.delete()
                }
                LogStore.append("app: published $name to $uri")
            } else {
                TransferRepository.update(id) {
                    it.copy(
                        status = Status.Publishing,
                        error = "Failed to publish the verified file; private staging was retained",
                        log = addLog(it.log, "publish failed · staging retained"),
                    )
                }
                LogStore.append("app: publish failed id=$id; staging retained")
            }
        }
    }

    private fun cleanupCompletedNativeFiles(
        id: Long,
        spec: TransferSpec?,
    ) {
        if (spec == null) return
        if (spec.dir() == Direction.Receive) {
            File(spec.path).deleteRecursively()
        } else {
            cleanupCachedSend(spec.path)
        }
        specs.remove(id)
        TransferSpecStore.remove(this, id)
    }

    private fun cleanupRemovedNativeFiles(spec: TransferSpec?) {
        if (spec == null) return
        if (spec.dir() == Direction.Receive) File(spec.path).deleteRecursively() else cleanupCachedSend(spec.path)
    }

    private fun cleanupCachedSend(path: String) {
        val source = File(path)
        val sendCache = File(cacheDir, "send")
        val isCacheCopy =
            runCatching { source.canonicalFile.toPath().startsWith(sendCache.canonicalFile.toPath()) }
                .getOrDefault(false)
        if (isCacheCopy) source.delete()
    }

    private fun markActive(
        id: Long,
        spec: TransferSpec,
    ) {
        activeIds.add(id)
        acquireMulticast(id, spec)
        updateNotification()
    }

    private fun enterForeground() {
        startForeground(NOTIF_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    }

    private fun notification(): Notification {
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val active = TransferRepository.transfers.value.filter { it.status.needsForeground() }
        val builder =
            NotificationCompat
                .Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_upload)
                .setOngoing(active.isNotEmpty())
                .setOnlyAlertOnce(true)
                .setContentIntent(open)
        val transfer = active.singleOrNull()
        when {
            active.isEmpty() -> builder.setContentTitle("Envoix").setContentText("No active transfers")
            transfer == null -> builder.setContentTitle("Envoix").setContentText("${active.size} transfers in progress")
            else -> {
                val verb = if (transfer.direction == Direction.Send) "Sending" else "Receiving"
                builder.setContentTitle("$verb ${transfer.fileName ?: "…"}")
                if (transfer.status == Status.Transferring && transfer.total > 0) {
                    val percent = ((transfer.bytes * 100) / transfer.total).toInt().coerceIn(0, 100)
                    builder.setContentText("$percent% · ${humanBytes(transfer.bytes)} / ${humanBytes(transfer.total)}")
                    builder.setProgress(100, percent, false)
                } else {
                    builder.setContentText(
                        transfer.status.name
                            .lowercase()
                            .replaceFirstChar { it.uppercase() },
                    )
                    builder.setProgress(0, 0, true)
                }
            }
        }
        return builder.build()
    }

    private fun updateNotification() {
        if (activeIds.isNotEmpty()) {
            getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification())
        }
    }

    private fun acquireMulticast(
        id: Long,
        spec: TransferSpec,
    ) {
        if (spec.useMdns && multicastHolders.add(id)) {
            runCatching { multicastLock.acquire() }.onFailure { multicastHolders.remove(id) }
        }
    }

    private fun releaseMulticast(id: Long) {
        if (multicastHolders.remove(id)) runCatching { multicastLock.release() }
    }

    private fun stopIfIdle(): Int {
        if (activeIds.isEmpty() && publishingIds.isEmpty()) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun hasInternet(): Boolean {
        val manager = getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val capabilities = manager.activeNetwork?.let { manager.getNetworkCapabilities(it) } ?: return false
        return capabilities.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private fun addLog(
        current: List<String>,
        line: String,
    ): List<String> = (current + "${logTime.format(Date())}  $line").takeLast(TransferRepository.LOG_CAP)

    override fun onDestroy() {
        observedIds.forEach(UniffiTransferRunner::detach)
        scope.cancel()
        multicastHolders.toList().forEach(::releaseMulticast)
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIF_ID = 1
        private const val ACTION_RESTORE = "dev.envoix.app.RESTORE"
        private const val ACTION_START = "dev.envoix.app.START"
        private const val ACTION_CANCEL = "dev.envoix.app.CANCEL"
        private const val ACTION_PAUSE = "dev.envoix.app.PAUSE"
        private const val ACTION_RESUME = "dev.envoix.app.RESUME"
        private const val ACTION_REMOVE = "dev.envoix.app.REMOVE"
        private const val EXTRA_DIRECTION = "direction"
        private const val EXTRA_ROOM = "room"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_BROKER = "broker"
        private const val EXTRA_RELAY = "relay"
        private const val EXTRA_CONFIG = "config"
        private const val EXTRA_QR = "qr"
        private const val EXTRA_TRANSFER_INVITE = "transfer_invite"
        private const val EXTRA_ID = "id"

        fun restore(context: Context) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply { action = ACTION_RESTORE },
            )
        }

        fun start(
            context: Context,
            direction: String,
            room: String,
            path: String,
            broker: String,
            relay: String,
            config: String,
            qrPayload: String?,
            transferInvite: String?,
        ) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_START
                    putExtra(EXTRA_DIRECTION, direction)
                    putExtra(EXTRA_ROOM, room)
                    putExtra(EXTRA_PATH, path)
                    putExtra(EXTRA_BROKER, broker)
                    putExtra(EXTRA_RELAY, relay)
                    putExtra(EXTRA_CONFIG, config)
                    putExtra(EXTRA_QR, qrPayload)
                    putExtra(EXTRA_TRANSFER_INVITE, transferInvite)
                },
            )
        }

        fun cancel(
            context: Context,
            id: Long,
        ) = command(context, ACTION_CANCEL, id, foreground = false)

        fun pause(
            context: Context,
            id: Long,
        ) = command(context, ACTION_PAUSE, id, foreground = false)

        fun resume(
            context: Context,
            id: Long,
        ) = command(context, ACTION_RESUME, id, foreground = true)

        fun remove(
            context: Context,
            id: Long,
        ) = command(context, ACTION_REMOVE, id, foreground = false)

        private fun command(
            context: Context,
            actionValue: String,
            id: Long,
            foreground: Boolean,
        ) {
            val intent =
                Intent(context, TransferService::class.java).apply {
                    action = actionValue
                    putExtra(EXTRA_ID, id)
                }
            if (foreground) context.startForegroundService(intent) else context.startService(intent)
        }
    }
}

private fun String.toDirection(): Direction = if (this == "send") Direction.Send else Direction.Receive

private fun FfiTransferActivityRecord.toStatus(): Status =
    when (state) {
        FfiTransferActivityState.QUEUED,
        FfiTransferActivityState.BINDING,
        FfiTransferActivityState.WAITING_FOR_PEER,
        -> Status.Waiting
        FfiTransferActivityState.PAIRING,
        FfiTransferActivityState.CONNECTING,
        -> Status.Connecting
        FfiTransferActivityState.TRANSFERRING -> Status.Transferring
        FfiTransferActivityState.VERIFYING ->
            if (diagnosticMessage == "confirming") Status.Confirming else Status.Verifying
        FfiTransferActivityState.UNCONFIRMED -> Status.Unconfirmed
        FfiTransferActivityState.PUBLISHING -> Status.Publishing
        FfiTransferActivityState.COMPLETED -> Status.Completed
        FfiTransferActivityState.FAILED,
        FfiTransferActivityState.UNKNOWN,
        -> Status.Failed
        FfiTransferActivityState.PAUSED -> Status.Paused
        FfiTransferActivityState.CANCELED -> Status.Cancelled
    }

private fun FfiDataPathKind.displayName(): String? =
    when (this) {
        FfiDataPathKind.DIRECT -> "direct"
        FfiDataPathKind.RELAY -> "relay"
        FfiDataPathKind.OTHER -> "other"
        FfiDataPathKind.NONE -> null
    }

private fun Status.needsForeground(): Boolean =
    this == Status.Waiting ||
        this == Status.Connecting ||
        this == Status.Verifying ||
        this == Status.Transferring ||
        this == Status.Confirming ||
        this == Status.Publishing ||
        this == Status.Unconfirmed

private fun ULong.toLongSaturated(): Long = if (this > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else toLong()
