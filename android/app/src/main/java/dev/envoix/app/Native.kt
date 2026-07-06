package dev.envoix.app

/** Callback invoked (off the main thread) with each transfer event's JSON. */
interface EventCallback {
    fun onEvent(json: String)
}

/** Sink for the core's `tracing` log lines. */
interface LogCallback {
    fun log(line: String)
}

/** JNI bridge to the in-process Envoix core (libenvoix_jni.so). */
object Native {
    init {
        System.loadLibrary("envoix_jni")
    }

    /** Route the core's logs to [sink]. Call once, before [initContext]. */
    external fun initLogging(sink: LogCallback)

    /** Wire the Android VM + app context into the Rust network stack. Call once. */
    external fun initContext(context: android.content.Context)

    /**
     * Run one transfer to completion, blocking the calling thread; each event's
     * JSON is delivered to [callback]. `direction` is "send" or "receive";
     * `path` is the file to send or the output directory to receive into.
     */
    external fun runTransfer(
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        callback: EventCallback,
    )
}
