package dev.envoix.app

enum class Direction { Send, Receive }

/**
 * Card status. Each variant carries the exact `wire` string the Rust `State`
 * enum serializes to (serde snake_case, `envoix-client/src/api/machine.rs`) —
 * the wire string is the single source, so [fromWire] can't silently drift from
 * the enum. A Rust rename breaks the Rust serialization test; an unmapped state
 * is surfaced by [fromWire] returning null (never a silent card freeze).
 */
enum class Status(
    val wire: String,
) {
    Preparing("preparing"),
    Waiting("waiting"),
    Connecting("connecting"),
    Verifying("verifying"),
    Transferring("transferring"),
    Confirming("confirming"),
    Paused("paused"),
    Completed("completed"),
    Unconfirmed("unconfirmed"),
    Failed("failed"),
    Cancelled("cancelled"),
    ;

    companion object {
        /** The core `State` wire string → Status, or null if this build has no
         *  mapping for it (the Rust enum gained a state we don't know). */
        fun fromWire(wire: String): Status? = entries.firstOrNull { it.wire == wire }
    }
}

/** One transfer's observable state, shown as a card. */
data class Transfer(
    val id: Long,
    val direction: Direction,
    val room: String,
    val fileName: String? = null,
    /** Machine attempt number (resume bumps it). */
    val attempt: Int = 1,
    /** Receiver: the confirmation duty is discharged (receipt on the rdz). */
    val proofDelivered: Boolean = false,
    /** Core transfer id (from Started) — keys the rdz receipt mailbox. */
    val transferId: String? = null,
    val pathType: String? = null,
    val pathAddr: String? = null,
    val bytes: Long = 0,
    val total: Long = 0,
    /** Instantaneous throughput (bytes/s) of the last interval. */
    val speedBps: Double = 0.0,
    /** True average throughput (total bytes / elapsed), matching the CLI's avg_bps. */
    val avgBps: Double = 0.0,
    val status: Status = Status.Connecting,
    val error: String? = null,
    /** Where a received file ended up (a `content://` in Downloads), for opening. */
    val savedUri: String? = null,
    /** The name the received file was actually published under — may differ from
     *  [fileName] (the transfer identity) after a collision bump, e.g. "photo (1).jpg". */
    val publishedName: String? = null,
    /** Public-artifact evidence captured while copying the verified staging file. */
    val publishedSize: Long? = null,
    val publishedSha256: String? = null,
    /** The public artifact was deleted, changed, or cannot be proven to match.
     *  Its private receipt must never be served while this is true. */
    val publicationInvalid: Boolean = false,
    /** A received file that finished transferring but could not be published to
     *  public storage (a non-collision publish failure). The bytes are safe in
     *  staging and a retry re-drives; surfaced so the user isn't left thinking it
     *  silently vanished. */
    val publishFailed: Boolean = false,
    /** For an initiated session, the invite payload to show as a QR while waiting
     *  for a peer to pair (null when we joined someone else's code). */
    val qrPayload: String? = null,
    /** Recent throughput samples (bytes/s), for the detail drawer's speed chart. */
    val speedHistory: List<Double> = emptyList(),
    /** Timestamped log lines scoped to this transfer, for the detail drawer. */
    val log: List<String> = emptyList(),
)

val Status.isTerminal: Boolean
    // Exhaustive (no `else`) so a new Status must be classified here too.
    get() =
        when (this) {
            Status.Completed,
            Status.Unconfirmed,
            Status.Failed,
            Status.Cancelled,
            -> true
            Status.Preparing,
            Status.Waiting,
            Status.Connecting,
            Status.Verifying,
            Status.Transferring,
            Status.Confirming,
            Status.Paused,
            -> false
        }

/** Human-readable byte count (the ONE implementation - was duplicated). */
fun humanBytes(n: Long): String =
    when {
        n < 1024 -> "$n B"
        n < 1024 * 1024 -> "%.0f KB".format(n / 1024.0)
        n < 1024L * 1024 * 1024 -> "%.1f MB".format(n / 1048576.0)
        else -> "%.2f GB".format(n / 1073741824.0)
    }

/** Trailing-window average of the 250 ms-sampled rate (~3 s): the headline
 *  speed/ETA smoothing policy, kept beside the model (not in a composable). */
fun smoothedBps(t: Transfer): Double {
    val window = t.speedHistory.takeLast(12)
    return if (window.isEmpty()) t.avgBps else window.average()
}
