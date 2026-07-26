package app.envoix.host

/**
 * The JNI lane onto the Rust composition root (`envoix-host-android`).
 *
 * Bytes in, bytes out: every frame is a generated-contract JSON document
 * (read/command schemas) or a bounded platform work order/report. No logic
 * lives on this boundary.
 */
object NativeHost {
    /** The token [attach] returns when there is no host to observe. */
    const val NO_ATTACHMENT = 0L

    init {
        System.loadLibrary("envoix_host_android")
    }

    /** Boots the process-wide host over the app-private storage root. */
    external fun boot(storageRoot: String): Boolean

    /**
     * Opens a fresh frontend attachment: every known card's stream restarts at
     * a new epoch. Starts and stops nothing; [NO_ATTACHMENT] means no host is
     * running. There is no detach counterpart — a frontend that leaves stops
     * polling.
     *
     * The returned token IS the attachment. Opening the next one supersedes it,
     * and only the newest may take a frame off the destructive queue.
     */
    external fun attach(): Long

    /**
     * One encoded read/command contract frame for [token], or null when
     * drained. Throws [SupersededAttachment] once a newer attachment holds the
     * lane.
     */
    external fun pollFrame(token: Long): ByteArray?

    /** One encoded platform work order, or null when drained. */
    external fun pollWork(): ByteArray?

    /**
     * Hands one frontend-originated intent frame to the authority and returns
     * its encoded answer: an acceptance for a command on an existing card, or
     * a create result for a request that one be made. The frontend decides
     * neither — it asks, and is told what happened.
     */
    external fun intent(frame: ByteArray): ByteArray?

    /** Reports one executed work order; true when admitted fresh. */
    external fun reportDuty(report: ByteArray): Boolean

    /** Stops the runtime; durable truth is already on disk. */
    external fun shutdown()
}

/**
 * Raised by [NativeHost.pollFrame] when a newer attachment holds the lane. The
 * poll consumed nothing and never will again, which is why it is a refusal and
 * not an empty result.
 */
class SupersededAttachment(
    message: String,
) : IllegalStateException(message)
