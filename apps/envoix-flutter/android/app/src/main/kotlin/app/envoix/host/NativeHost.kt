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

    /** One durably removed card whose persistable source grant must end. */
    external fun pollSourceRelease(): String?

    /**
     * Hands one frontend-originated intent frame to the authority and returns
     * its encoded answer: an acceptance for a command on an existing card, or
     * a create result for a request that one be made. The frontend decides
     * neither — it asks, and is told what happened.
     */
    external fun intent(frame: ByteArray): ByteArray?

    /** Reports one executed work order; true when admitted fresh. */
    external fun reportDuty(report: ByteArray): Boolean

    /**
     * Hands one acquisition's open source descriptor down to Rust, which reads
     * the bytes itself.
     *
     * A crossing of its own, correlated by the whole acquisition — card,
     * generation and request — rather than folded into [reportDuty], which
     * would need a sentinel on every duty that has no descriptor.
     *
     * **The descriptor is LENT, not handed over.** Rust duplicates it inside the
     * call and owns only the duplicate; this side keeps its own and must close
     * it, which a `use` block does whatever happens here — including when the
     * call itself fails to link. Detaching would read tidier and leave the file
     * open with no owner in either language on exactly that path.
     */
    external fun bindSourceDescriptor(
        card: String,
        generation: Int,
        request: String,
        fd: Int,
    ): Boolean

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

/**
 * Raised by [NativeHost.intent] when the host received the bytes but refused
 * them as a non-contract intent. This is neither a null host nor a lost answer:
 * no command/create handler ran, so no durable effect can exist.
 */
class RejectedIntent(
    message: String,
) : IllegalArgumentException(message)
