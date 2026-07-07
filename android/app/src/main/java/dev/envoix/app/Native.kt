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

    /** Change the log filter at runtime (dev-mode verbosity toggle). [spec] is an
     *  env-filter directive, e.g. `envoix=trace,iroh=debug`. */
    external fun setLogLevel(spec: String)

    /** Wire the Android VM + app context into the Rust network stack. Call once. */
    external fun initContext(context: android.content.Context)

    /**
     * Run one transfer to completion, blocking the calling thread; each event's
     * JSON is delivered to [callback]. `direction` is "send" or "receive"; `path`
     * is the file to send or the output directory to receive into. `id` keys the
     * transfer so it can be [cancel]led.
     */
    external fun runTransfer(
        id: Long,
        direction: String,
        code: String,
        broker: String,
        relay: String,
        path: String,
        configPath: String,
        callback: EventCallback,
    )

    /** Request cancellation of the in-flight transfer with the given [id]. */
    external fun cancel(id: Long)

    /** Generate a room invite for [role] ("send"/"receive"). Returns JSON
     *  `{"code":..,"payload":..}` (payload = the QR string), or `{"error":..}`. */
    external fun generateInvite(role: String, broker: String, relay: String): String

    /** Parse a typed code or a scanned `envoix://` payload. Returns JSON
     *  `{"code":..,"broker":..,"relay":..,"role":..}`, or `{"error":..}`. */
    external fun parseInvite(input: String): String
}
