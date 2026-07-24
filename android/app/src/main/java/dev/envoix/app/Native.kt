package dev.envoix.app

/** Canonical Manifest-v2 callbacks. [onSaveRequired] is synchronous because
 * the core must not emit receiver results or delivery proof until the actual
 * SAF/MediaStore save has durably completed. It runs on a native worker. */
interface ManifestV2Callback {
    fun onEvent(json: String)

    fun onPlanRequired(requestJson: String): String

    fun onSaveRequired(requestJson: String): String
}

/** Platform-owned reliable byte stream. Null from [receive] is EOF. Rust owns
 * TLS, invitation authentication, Manifest-v2 framing, and file semantics. */
interface NativeDuplexTransport {
    fun send(bytes: ByteArray)

    fun receive(maxBytes: Int): ByteArray?

    fun close()
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

    /** Create the durable canonical job as soon as the first source is chosen. */
    external fun createManifestV2Job(
        storeDirectory: String,
        compression: String,
    ): String

    /** Restore durable jobs that have local preparation but have not crossed
     * the explicit Send/Seal boundary. */
    external fun listManifestV2PreparingJobs(storeDirectory: String): String

    /** Add already stabilized local roots and run local-only preparation. */
    external fun prepareManifestV2Job(
        storeDirectory: String,
        jobId: String,
        rootsJson: String,
    ): String

    external fun resolveManifestV2Source(
        storeDirectory: String,
        jobId: String,
        rootItemId: Long,
        decision: String,
        reauthorizedPath: String,
    ): String

    external fun reauthorizeManifestV2ProviderSource(
        storeDirectory: String,
        jobId: String,
        rootItemId: Long,
        rootJson: String,
    ): String

    /** Start the only Android transfer engine. A send seals its existing job;
     * a receive first emits an authenticated offer and waits for a decision. */
    external fun startManifestV2Session(
        id: Long,
        paramsJson: String,
        callback: ManifestV2Callback,
    )

    /** Start the same canonical engine on an already established platform
     * Wi-Fi Aware byte stream. */
    external fun startManifestV2NativeSession(
        id: Long,
        paramsJson: String,
        pairingToken: String,
        transport: NativeDuplexTransport,
        callback: ManifestV2Callback,
    )

    external fun continueManifestV2Receive(
        id: Long,
        decisionJson: String,
    ): String

    external fun listManifestV2OfferEntries(
        id: Long,
        offset: Long,
        limit: Long,
    ): String

    external fun cancelManifestV2Session(id: Long)
}
