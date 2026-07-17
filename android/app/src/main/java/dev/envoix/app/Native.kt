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

    /** The narrow restore summary of every persisted record (typed by the
     *  core): a flat JSON array of {id, direction, code, path, use_room,
     *  use_mdns, qr?, saved_uri?}. The frontend never parses raw record JSON. */
    external fun listRestoreContexts(): String

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

    /** Answer a fetch_receipt notice: the blob (base64), or "" for an empty
     *  slot. [key] echoes the notice's mailbox key, so the driver can drop
     *  answers from a superseded attempt. */
    external fun receiptResponse(
        id: Long,
        key: String,
        blobB64: String,
    )

    /** Create a SEND session that stages its content:// source first: the
     *  session starts in Preparing and the record is committed before Kotlin
     *  copies a byte. Notices flow to [callback] like [createSession]. */
    external fun createStagingSession(
        id: Long,
        paramsJson: String,
        callback: EventCallback,
    )

    /** A Preparing send: staging copied [bytes] so far — moves the bar.
     *  [generation] is the `attempt` the staging worker was authorized by; the
     *  reducer drops a stale worker's callback. */
    external fun stageProgress(
        id: Long,
        generation: Int,
        bytes: Long,
    )

    /** A Preparing send: staging finished — launch the first attempt. */
    external fun stageComplete(
        id: Long,
        generation: Int,
    )

    /** A Preparing send: staging failed — fail the transfer with [reason]. */
    external fun stageFailed(
        id: Long,
        generation: Int,
        reason: String,
    )

    /** Replace the card context (QR payload, saved URI) persisted with the
     *  transfer's record; opaque to the core, surfaced via listRestoreContexts.
     *  Returns "" on success, or an error message if [extrasJson] failed the
     *  boundary validation (an unknown/mistyped key). */
    external fun setSessionExtras(
        id: Long,
        extrasJson: String,
    ): String

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
