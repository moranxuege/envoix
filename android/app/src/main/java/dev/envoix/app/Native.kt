package dev.envoix.app

/** Canonical Manifest-v2 callbacks. [onSaveRequired] is synchronous because
 * the core must not emit receiver results or delivery proof until the actual
 * SAF/MediaStore save has durably completed. It runs on a native worker. */
interface ManifestV2Callback {
    fun onEvent(json: String)

    fun onPlanRequired(requestJson: String): String

    fun onSaveRequired(requestJson: String): String

    /** Persist a negotiated or rotated opaque relationship credential before
     * the core sends any Manifest frame. */
    fun onRememberedCredential(
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean
}

interface NearbyInviteCallback {
    fun onEvent(json: String)
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

    /** Receiver-only production bridge until its destination/result gate is typed. */
    external fun generateInvite(
        role: String,
        broker: String,
        relay: String,
    ): String

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

    /** Receiver-only production parser paired with the JNI receiver session. */
    external fun parseInviteForRole(
        input: String,
        role: String,
    ): String

    /** Receiver session retained until SAF/MediaStore is a typed result-gate port. */
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
