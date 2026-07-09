package dev.envoix.app

import android.app.Application

/** Initializes logging + the native Android context once, before any transfer. */
class EnvoixApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SettingsStore.init(this)
        LogStore.init(filesDir)
        NativeBootstrap.initLogging(LogSink) // before initContext, so init logs are captured
        SettingsStore.applyLogLevel() // restore the saved (dev) verbosity
        NativeBootstrap.initContext(this)

        // Capture uncaught exceptions into the log (foundation for crash reporting).
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            LogStore.append("FATAL ${error.javaClass.simpleName}: ${error.message}")
            LogStore.writeCrash(error)
            previous?.uncaughtException(thread, error)
        }
    }
}
