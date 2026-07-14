package dev.envoix.app

/** Sink for the core's `tracing` log lines. */
interface LogCallback {
    fun log(line: String)
}

/** JNI bootstrap for Android context + logs only; transfers use the UniFFI binding. */
object NativeBootstrap {
    init {
        System.loadLibrary("envoix_ffi")
    }

    /** Route the core's logs to [sink]. Call once, before [initContext]. */
    external fun initLogging(sink: LogCallback)

    /** Change the log filter at runtime (dev-mode verbosity toggle). */
    external fun setLogLevel(spec: String)

    /** Wire the Android VM + app context into the Rust network stack. Call once. */
    external fun initContext(context: android.content.Context)
}
