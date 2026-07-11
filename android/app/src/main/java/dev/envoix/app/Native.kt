package dev.envoix.app

/** Callback invoked (off the main thread) with each transfer event's JSON. */
interface EventCallback {
    fun onEvent(json: String)
}

/** Sink for the core's `tracing` log lines. */
interface LogCallback {
    fun log(
        room: String?,
        line: String,
    )
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

    /** Set the durable transfer-record directory (once, at app start). */
    external fun initRecords(dir: String)

    /** All persisted transfer records, as a JSON array. */
    external fun listRecords(): String

    /** Rehydrate a persisted session (no attempt launched); notices flow to
     *  [callback] like [createSession]. */
    external fun restoreSession(
        id: Long,
        callback: EventCallback,
    )

    /** Create + start a transfer session (the Rust state-machine driver).
     *  Notices (snapshots + mailbox courier requests) arrive on [callback] as
     *  JSON; returns immediately. */
    external fun createSession(
        id: Long,
        paramsJson: String,
        callback: EventCallback,
    )

    /** Route a user intent ("pause" / "resume" / "cancel") to a live session. */
    external fun sessionIntent(
        id: Long,
        intent: String,
    )

    /** Answer a fetch_receipt notice: the blob (base64), or "" for an empty slot. */
    external fun receiptResponse(
        id: Long,
        blobB64: String,
    )

    /** Tear a session down; with [discard], delete partial/resume/receipt (D2). */
    external fun destroySession(
        id: Long,
        discard: Boolean,
    )

    /** Generate a room invite for [role] ("send"/"receive"). Returns JSON
     *  `{"code":..,"payload":..}` (payload = the QR string), or `{"error":..}`. */
    external fun generateInvite(
        role: String,
        broker: String,
        relay: String,
    ): String

    /** Parse a typed code or a scanned `envoix://` payload. Returns JSON
     *  `{"code":..,"broker":..,"relay":..,"role":..}`, or `{"error":..}`. */
    external fun parseInvite(input: String): String
}
