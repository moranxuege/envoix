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
                startForeground(
                    NOTIF_ID,
                    notification(),
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
                )
                launchTransfer(direction, room, path)
            }
            ACTION_CANCEL -> {
                val id = intent.getLongExtra(EXTRA_ID, -1L)
                Native.cancel(id)
                TransferRepository.update(id) {
                    if (it.status.isTerminal) it else it.copy(status = Status.Cancelled)
                }
            }
        }
        return START_NOT_STICKY
    }

    private fun launchTransfer(directionStr: String, room: String, path: String) {
        val dir = if (directionStr == "send") Direction.Send else Direction.Receive
        val id = TransferRepository.create(dir, room)
        LogStore.append("app: start $directionStr room=${room.substringBefore('-')} id=$id")
        active++
        updateNotification()
        scope.launch {
            var lastTs = 0L
            var lastBytes = 0L
            NativeTransfer.run(id, directionStr, room, Endpoints.BROKER, Endpoints.RELAY, path)
                .collect { ev ->
                    TransferRepository.update(id) { t ->
                        when (ev) {
                            CliEvent.Binding, CliEvent.Connecting ->
                                t.copy(status = Status.Connecting)
                            is CliEvent.Connected ->
                                t.copy(pathType = ev.pathType, pathAddr = ev.addr)
                            is CliEvent.Started ->
                                t.copy(fileName = ev.fileName, total = ev.totalBytes, status = Status.Transferring)
                            is CliEvent.Progress -> {
                                val now = System.currentTimeMillis()
                                val bps = if (lastTs > 0 && now > lastTs)
                                    (ev.bytesTransferred - lastBytes) * 1000.0 / (now - lastTs)
                                else t.speedBps
                                lastTs = now; lastBytes = ev.bytesTransferred
                                t.copy(bytes = ev.bytesTransferred, total = ev.totalBytes, speedBps = bps, status = Status.Transferring)
                            }
                            is CliEvent.Completed ->
                                t.copy(bytes = ev.bytesTransferred, speedBps = 0.0, status = Status.Completed)
                            is CliEvent.Failed ->
                                t.copy(status = Status.Failed, error = ev.error)
                            is CliEvent.Exit ->
                                if (!t.status.isTerminal)
                                    t.copy(status = if (ev.code == 0) Status.Completed else Status.Failed)
                                else t
                        }
                    }
                }
            if (dir == Direction.Receive) publishReceived(id, path)
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
        val uri = MediaStoreSaver.saveToDownloads(this, src, name)
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
        private const val EXTRA_DIRECTION = "direction"
        private const val EXTRA_ROOM = "room"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_ID = "id"

        /** `direction` is "send"/"receive"; `path` is the file to send or the
         *  output directory to receive into. */
        fun start(context: Context, direction: String, room: String, path: String) {
            context.startForegroundService(
                Intent(context, TransferService::class.java).apply {
                    action = ACTION_START
                    putExtra(EXTRA_DIRECTION, direction)
                    putExtra(EXTRA_ROOM, room)
                    putExtra(EXTRA_PATH, path)
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
    }
}
