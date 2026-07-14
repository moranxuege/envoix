package dev.envoix.app

import android.app.Application

/** Initializes logging, Android context, and the durable UniFFI runner once. */
class EnvoixApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SettingsStore.init(this)
        LogStore.init(filesDir)
        OpLog.init(filesDir)
        TransferLogs.init(filesDir)
        Diagnostics.init(filesDir)

        NativeBootstrap.initLogging(LogSink)
        SettingsStore.applyLogLevel()
        NativeBootstrap.initContext(this)
        UniffiTransferRunner.initialize(this)

        val nextId =
            runCatching {
                UniffiTransferRunner.records()
                    .mapNotNull { UniffiTransferRunner.parseActivityId(it.activityId) }
                    .maxOrNull()
                    ?.plus(1)
                    ?: 1L
            }.getOrDefault(1L)
        TransferRepository.seedNextId(nextId)
        LogStore.append("envoix-android v${BuildConfig.VERSION_NAME} (${BuildConfig.GIT_COMMIT})")
        TransferService.restore(this)

        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            OpLog.add("CRASH ${error.javaClass.simpleName}: ${error.message}")
            LogStore.append("FATAL ${error.javaClass.simpleName}: ${error.message}")
            LogStore.writeCrash(error)
            previous?.uncaughtException(thread, error)
        }
    }
}
