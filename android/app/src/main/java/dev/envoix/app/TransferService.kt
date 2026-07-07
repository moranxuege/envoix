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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File

/** Params needed to (re)launch a transfer, kept so a paused/failed one can resume. */
private data class Spec(
    val direction: String,
    val room: String,
    val path: String,
    val broker: String,
    val relay: String,
    val config: String,
    /** Invite payload to advertise as a QR while waiting (initiated sessions only). */
    val qrPayload: String?,
) {
    fun dir(): Direction = if (direction == "send") Direction.Send else Direction.Receive
}

/**
 * Foreground service that owns running transfers (decoupled from the Activity so
 * they survive backgrounding), updates [TransferRepository], publishes received
 * files to Downloads, and shows an ongoing progress notification.
 */
class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var active = 0

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val mgr = getSystemService(NotificationManager::class.java)
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL, "Transfers", NotificationManager.IMPORTANCE_LOW)
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                val direction = intent.getStringExtra(EXTRA_DIRECTION)
                val room = intent.getStringExtra(EXTRA_ROOM)
                val path = intent.getStringExtra(EXTRA_PATH)
                if (direction == null || room == null || path == null) return stopIfIdle()
                val spec = Spec(
                    direction, room, path,
                    intent.getStringExtra(EXTRA_BROKER) ?: Endpoints.BROKER,
                    intent.getStringExtra(EXTRA_RELAY) ?: Endpoints.RELAY,
                    intent.getStringExtra(EXTRA_CONFIG) ?: "",
                    intent.getStringExtra(EXTRA_QR),
                )
                enterForeground()
                val id = TransferRepository.create(spec.dir(), room)
                TransferRepository.update(id) {
                    it.copy(
                        qrPayload = spec.qrPayload,
                        // Show the outgoing file name right away; receives learn it on Started.
                        fileName = if (spec.dir() == Direction.Send) File(spec.path).name else it.fileName,
                    )
                }
                specs[id] = spec
                LogStore.append("app: start $direction room=${room.substringBefore('-')} id=$id")
                runLoop(id, spec)
            }
            ACTION_RESUME -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                val spec = specs[id] ?: return stopIfIdle()
                enterForeground()
                TransferRepository.update(id) { it.copy(status = Status.Connecting, error = null) }
                LogStore.append("app: resume id=$id")
                runLoop(id, spec)
            }
            ACTION_PAUSE -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                Native.cancel(id)
                TransferRepository.update(id) {
                    if (it.status.isTerminal) it else it.copy(status = Status.Paused, speedBps = 0.0)
                }
            }
            ACTION_CANCEL -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                Native.cancel(id)
                specs.remove(id)
                TransferRepository.update(id) {
                    if (it.status.isTerminal) it else it.copy(status = Status.Cancelled)
                }
            }
        }
        return START_NOT_STICKY
    }

    private fun enterForeground() =
        startForeground(NOTIF_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)

    private val logTime = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US)

    /** Append a timestamped line to a transfer's log, keeping the last 60. */
    private fun addLog(cur: List<String>, line: String): List<String> =
        (cur + "${logTime.format(java.util.Date())}  $line").takeLast(60)

    private fun runLoop(id: Long, spec: Spec) {
        active++
        updateNotification()
        scope.launch {
            var lastTs = 0L
            var lastBytes = 0L
            NativeTransfer.run(id, spec.direction, spec.room, spec.broker, spec.relay, spec.path, spec.config)
                .collect { ev ->
                    TransferRepository.update(id) { t ->
                        when (ev) {
                            CliEvent.Binding, CliEvent.Connecting ->
                                t.copy(
                                    status = Status.Connecting,
                                    log = if (t.log.isEmpty())
                                        addLog(t.log, "pairing in room ${spec.room.substringBefore('-')}…")
                                    else t.log,
                                )
                            is CliEvent.Connected ->
                                t.copy(
                                    pathType = ev.pathType, pathAddr = ev.addr,
                                    log = addLog(
                                        t.log,
                                        "connected · ${ev.pathType}" + if (ev.addr.isNotBlank()) " (${ev.addr})" else "",
                                    ),
                                )
                            is CliEvent.Started ->
                                t.copy(
                                    fileName = ev.fileName, total = ev.totalBytes, status = Status.Transferring,
                                    log = addLog(t.log, "started · ${ev.fileName}"),
                                )
                            is CliEvent.Progress -> {
                                val now = System.currentTimeMillis()
                                val bps = if (lastTs > 0 && now > lastTs)
                                    (ev.bytesTransferred - lastBytes) * 1000.0 / (now - lastTs)
                                else t.speedBps
                                lastTs = now; lastBytes = ev.bytesTransferred
                                t.copy(
                                    bytes = ev.bytesTransferred, total = ev.totalBytes, speedBps = bps,
                                    status = Status.Transferring,
                                    speedHistory = (t.speedHistory + bps).takeLast(90),
                                )
                            }
                            is CliEvent.Completed ->
                                t.copy(
                                    bytes = ev.bytesTransferred, speedBps = 0.0, status = Status.Completed,
                                    log = addLog(t.log, "complete"),
                                )
                            is CliEvent.Failed ->
                                // A pause/cancel surfaces as a Failed event; keep the
                                // Paused/Cancelled status the action already set.
                                if (t.status == Status.Cancelled || t.status == Status.Paused) t
                                else t.copy(status = Status.Failed, error = ev.error, log = addLog(t.log, "failed · ${ev.error}"))
                            is CliEvent.Exit ->
                                if (t.status.isTerminal || t.status == Status.Paused) t
                                else t.copy(status = if (ev.code == 0) Status.Completed else Status.Failed)
                        }
                    }
                }
            if (spec.dir() == Direction.Receive) publishReceived(id, spec.path)
            active--
            updateNotification()
            stopIfIdle()
        }
    }

    /** Move a completed received file from the private output dir into Downloads. */
    private fun publishReceived(id: Long, outputDir: String) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status != Status.Completed) return
        val name = t.fileName ?: return
        val src = File(outputDir, name)
        if (!src.exists()) return
        val uri = MediaStoreSaver.saveToDownloads(this, src, name, SettingsStore.settings.value.saveFolder)
        if (uri != null) {
            src.delete()
            TransferRepository.update(id) { it.copy(savedUri = uri.toString()) }
            LogStore.append("app: saved $name to Downloads")
        }
    }

    private fun notification(): Notification {
        val count = TransferRepository.activeCount()
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Envoix")
            .setContentText(if (count > 0) "$count transfer${if (count == 1) "" else "s"} in progress" else "Done")
            .setOngoing(count > 0)
            .setContentIntent(open)
            .build()
    }

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification())
    }

    private fun stopIfIdle(): Int {
        if (active <= 0) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIF_ID = 1
        private const val ACTION_START = "dev.envoix.app.START"
        private const val ACTION_CANCEL = "dev.envoix.app.CANCEL"
        private const val ACTION_PAUSE = "dev.envoix.app.PAUSE"
        private const val ACTION_RESUME = "dev.envoix.app.RESUME"

        /** Launch specs by id, so a paused/failed transfer can be resumed. */
        private val specs = java.util.concurrent.ConcurrentHashMap<Long, Spec>()
        private const val EXTRA_DIRECTION = "direction"
        private const val EXTRA_ROOM = "room"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_BROKER = "broker"
        private const val EXTRA_RELAY = "relay"
        private const val EXTRA_CONFIG = "config"
        private const val EXTRA_QR = "qr"
        private const val EXTRA_ID = "id"

        /** `direction` is "send"/"receive"; `path` is the file to send or the
         *  output directory to receive into; `config` is a config.toml path or "";
         *  `qrPayload` is the invite to show while waiting (null when joining). */
        fun start(
            context: Context,
            direction: String,
            room: String,
            path: String,
            broker: String,
            relay: String,
            config: String,
            qrPayload: String?,
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
                }
            )
        }

        fun cancel(context: Context, id: Long) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_CANCEL
                    putExtra(EXTRA_ID, id)
                }
            )
        }

        /** Stop a transfer but keep its partial + spec, so it can be resumed. */
        fun pause(context: Context, id: Long) {
            context.startService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_PAUSE
                    putExtra(EXTRA_ID, id)
                }
            )
        }

        /** Relaunch a paused/failed transfer from its stored spec (resumes the partial). */
        fun resume(context: Context, id: Long) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_RESUME
                    putExtra(EXTRA_ID, id)
                }
            )
        }
    }
}
