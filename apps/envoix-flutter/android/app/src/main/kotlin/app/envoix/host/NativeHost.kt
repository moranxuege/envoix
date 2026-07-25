package app.envoix.host

/**
 * The JNI lane onto the Rust composition root (`envoix-host-android`).
 *
 * Bytes in, bytes out: every frame is a generated-contract JSON document
 * (read/command schemas) or a bounded platform work order/report. No logic
 * lives on this boundary.
 */
object NativeHost {
    init {
        System.loadLibrary("envoix_host_android")
    }

    /** Boots the process-wide host over the app-private storage root. */
    external fun boot(storageRoot: String): Boolean

    /** One encoded read/command contract frame, or null when drained. */
    external fun pollFrame(): ByteArray?

    /** One encoded platform work order, or null when drained. */
    external fun pollWork(): ByteArray?

    /** Submits a command frame; returns the encoded acceptance frame. */
    external fun submit(frame: ByteArray): ByteArray?

    /** Reports one executed work order; true when admitted fresh. */
    external fun reportDuty(report: ByteArray): Boolean

    /** Stops the runtime; durable truth is already on disk. */
    external fun shutdown()
}
