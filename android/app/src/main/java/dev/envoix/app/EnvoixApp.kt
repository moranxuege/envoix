package dev.envoix.app

import android.app.Application
import dev.envoix.app.ui.NativeRoomControlGateway
import dev.envoix.app.ui.RoomControlGatewayProvider

/** Initializes logging + the native Android context once, before any transfer. */
class EnvoixApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SettingsStore.init(this)
        LogStore.init(filesDir)
        OpLog.init(filesDir)
        TransferLogs.init(filesDir)
        Diagnostics.init(filesDir)
        TransferRepository.seedNextId(TransferService.nextSessionIdFloor(this))
        LogStore.append("envoix-android v${BuildConfig.VERSION_NAME} (${BuildConfig.GIT_COMMIT})")
        Native.initLogging(LogSink) // before initContext, so init logs are captured
        SettingsStore.applyLogLevel() // restore the saved (dev) verbosity
        Native.initContext(this)
        RoomControlGatewayProvider.gateway = NativeRoomControlGateway(this)

        // Capture uncaught exceptions into the log (foundation for crash reporting).
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            OpLog.add("CRASH ${error.javaClass.simpleName}: ${error.message}")
            LogStore.append("FATAL ${error.javaClass.simpleName}: ${error.message}")
            LogStore.writeCrash(error)
            previous?.uncaughtException(thread, error)
        }
    }
}
