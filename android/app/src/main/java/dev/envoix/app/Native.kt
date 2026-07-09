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
        chunkSize: String,
        candidatesAllow: String,
        candidatesDeny: String,
        useRoom: Boolean,
        useMdns: Boolean,
        resume: Boolean,
        callback: EventCallback,
    )

    /** Request cancellation of the in-flight transfer with the given [id]. */
    external fun cancel(id: Long)

    /** Request a pause of the in-flight transfer with the given [id]: same stop
     *  mechanics as [cancel], but reported — locally and (best-effort) to the
     *  peer — as a pause, so both sides can show a resumable state. */
    external fun pause(id: Long)

    /** Create + start a transfer session (the Rust state-machine driver).
     *  Notices (snapshots + mailbox courier requests) arrive on [callback] as
     *  JSON; returns immediately. */
    external fun createSession(id: Long, paramsJson: String, callback: EventCallback)

    /** Route a user intent ("pause" / "resume" / "cancel") to a live session. */
    external fun sessionIntent(id: Long, intent: String)

    /** Answer a fetch_receipt notice: the blob (base64), or "" for an empty slot. */
    external fun receiptResponse(id: Long, blobB64: String)

    /** Tear a session down; with [discard], delete partial/resume/receipt (D2). */
    external fun destroySession(id: Long, discard: Boolean)

    /** The rdz mailbox key this transfer's completion receipt is stored under. */
    external fun receiptMailboxKey(transferId: String): String

    /** Seal a completion receipt (its local JSON) for the rdz mailbox.
     *  Returns `{"key":"<hex>","blob":"<base64>"}` or `{"error":..}`. */
    external fun sealReceipt(transferId: String, code: String, receiptJson: String): String

    /** Open a mailbox blob and verify it against the local source file.
     *  Returns `{"ok":true}` or `{"error":..}`. */
    external fun verifyReceipt(transferId: String, code: String, blobB64: String, filePath: String): String

    /** Generate a room invite for [role] ("send"/"receive"). Returns JSON
     *  `{"code":..,"payload":..}` (payload = the QR string), or `{"error":..}`. */
    external fun generateInvite(role: String, broker: String, relay: String): String

    /** Parse a typed code or a scanned `envoix://` payload. Returns JSON
     *  `{"code":..,"broker":..,"relay":..,"role":..}`, or `{"error":..}`. */
    external fun parseInvite(input: String): String
}
