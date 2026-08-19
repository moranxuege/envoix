package dev.envoix.app

interface NearbyInviteCallback {
    fun onEvent(json: String)
}

/** Sink for the core's `tracing` log lines. */
interface LogCallback {
    fun log(
        room: String?,
        line: String,
    )

    /**
     * A structured authority-event line for the transfer timeline (v2),
     * pre-built by the core and routed by durable [sessionId] (the card id) —
     * NOT by room. The writer stamps `source_seq`; the core does not.
     */
    fun timeline(
        sessionId: Long,
        line: String,
    )
}

/** Exceptional JNI bridge compiled into the typed core (libenvoix_ffi.so). */
object Native {
    init {
        System.loadLibrary("envoix_ffi")
    }

    /** Route the core's logs to [sink]. Call once, before [initContext]. */
    external fun initLogging(sink: LogCallback)

    /** Change the log filter at runtime (dev-mode verbosity toggle). [spec] is an
     *  env-filter directive, e.g. `envoix=trace,iroh=debug`. */
    external fun setLogLevel(spec: String)

    /** Wire the Android VM + app context into the Rust network stack. Call once. */
    external fun initContext(context: android.content.Context)

    external fun startNearbyInviteInbox(
        id: Long,
        paramsJson: String,
        callback: NearbyInviteCallback,
    )

    external fun sendNearbyInvite(
        id: Long,
        requestId: String,
        routeJson: String,
        invite: String,
    ): String

    external fun stopNearbyInviteInbox(id: Long)
}
