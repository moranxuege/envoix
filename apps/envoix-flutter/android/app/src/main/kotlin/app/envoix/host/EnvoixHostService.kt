package app.envoix.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * The foreground service that OWNS the transfer runtime for this process.
 *
 * Frontends (F1/F2 Flutter) later bind as observers/commanders over the
 * generated contracts; their attach/detach never starts or stops transfers
 * (Pillar 7). The service boots the Rust host over the app-private storage
 * root, keeps the process foregrounded while work exists, and executes
 * typed platform work orders.
 */
class EnvoixHostService : Service() {
    private val running = AtomicBoolean(false)
    private lateinit var executor: DutyExecutor

    override fun onCreate() {
        super.onCreate()
        startInForeground()
        val root = File(filesDir, STORAGE_ROOT)
        if (!NativeHost.boot(root.absolutePath)) {
            stopSelf()
            return
        }
        SourcePicks.recover(this)
        executor = DutyExecutor(this)
        running.set(true)
        thread(name = "envoix-work-pump", isDaemon = true) { workPump() }
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        // Instrumentation actions (the derived <applicationId>.action.e2e-*
        // namespace) live in the debug source set only; the release source set
        // has no bridge, no JNI binding and no class by that name at all.
        intent?.let { handleInstrumentation(it, packageName) }
        return START_STICKY
    }

    override fun onDestroy() {
        if (running.compareAndSet(true, false)) {
            NativeHost.shutdown()
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /** Executes platform work orders until the service stops. */
    private fun workPump() {
        while (running.get()) {
            val removedCard = NativeHost.pollSourceRelease()
            if (removedCard != null) {
                SourcePicks.release(this, removedCard)
                continue
            }
            val order = NativeHost.pollWork()
            if (order == null) {
                Thread.sleep(WORK_POLL_MILLIS)
                continue
            }
            executor.execute(order)?.let { report ->
                NativeHost.reportDuty(report)
            }
        }
    }

    private fun startInForeground() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                "Envoix transfers",
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        val notification =
            Notification
                .Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setContentTitle("Envoix")
                .setContentText("Transfer host running")
                .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                FOREGROUND_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(FOREGROUND_ID, notification)
        }
    }

    companion object {
        /** Mirrors the catalogued `android.private_storage_root`. */
        const val STORAGE_ROOT = "state-envoix"
        const val CHANNEL_ID = "envoix.host"
        const val FOREGROUND_ID = 1
        const val WORK_POLL_MILLIS = 100L
    }
}
