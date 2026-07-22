package dev.envoix.app

enum class Direction { Send, Receive }

data class TransferInventoryEntry(
    val entryId: Int,
    val parentEntryId: Int?,
    val name: String,
    val directory: Boolean,
    val size: Long,
)

/** Native projection of canonical Manifest-v2 session phases. */
enum class Status(
    val wire: String,
) {
    Preparing("preparing"),
    Connecting("connecting"),
    AwaitingDecision("awaiting_decision"),
    Transferring("transferring"),
    Receiving("receiving"),
    Verifying("verifying"),
    Saving("saving"),
    WaitingForReceiverSave("waiting_for_receiver_save"),
    FinalizingDelivery("finalizing_delivery"),
    Paused("paused"),
    Completed("completed"),
    Failed("failed"),
    Cancelled("cancelled"),
    ;

    companion object {
        /** Canonical phase wire string to native UI state. */
        fun fromWire(wire: String): Status? = entries.firstOrNull { it.wire == wire }
    }
}

/** One transfer's observable state, shown as a card. */
data class Transfer(
    val id: Long,
    val direction: Direction,
    val room: String,
    val fileName: String? = null,
    /** Stable canonical Manifest-v2 job identity. */
    val jobId: String? = null,
    val rootCount: Int = 0,
    val fileCount: Int = 0,
    val directoryCount: Int = 0,
    val exceptionalOffer: Boolean = false,
    /** Bounded authenticated offer projection; the native ledger retains the
     * complete inventory and exposes additional pages on demand. */
    val inventoryPreview: List<TransferInventoryEntry> = emptyList(),
    val inventoryHasMore: Boolean = false,
    /** Local attempt number (resume bumps it). */
    val attempt: Int = 1,
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
    val savedUris: List<String> = emptyList(),
    /** The name the received file was actually saved under — may differ from
     *  [fileName] (the transfer identity) after a collision bump, e.g. "photo (1).jpg". */
    val savedName: String? = null,
    /** Stable machine cause; never reconstructed from [error]. */
    val failureCause: String? = null,
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
            Status.Failed,
            Status.Cancelled,
            -> true
            Status.Preparing,
            Status.Connecting,
            Status.AwaitingDecision,
            Status.Transferring,
            Status.Receiving,
            Status.Verifying,
            Status.Saving,
            Status.WaitingForReceiverSave,
            Status.FinalizingDelivery,
            Status.Paused,
            -> false
        }

/** Human-readable byte count shared by every transfer surface. */
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
