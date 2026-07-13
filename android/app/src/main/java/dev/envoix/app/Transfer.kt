package dev.envoix.app

enum class Direction { Send, Receive }

enum class Status {
    Waiting,
    Connecting,
    Verifying,
    Transferring,
    Confirming,
    Publishing,
    Paused,
    Completed,
    Unconfirmed,
    Failed,
    Cancelled,
}

/** One transfer's observable state, shown as a card. */
data class Transfer(
    val id: Long,
    /** Monotonic canonical snapshot sequence; older callbacks are discarded. */
    val sequence: Long = 0,
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
    /** Canonical core verdict: retry is meaningful for the current failure. */
    val retryable: Boolean = false,
    val error: String? = null,
    /** Where a received file ended up (a `content://` in Downloads), for opening. */
    val savedUri: String? = null,
    /** For an initiated session, the invite payload to show as a QR while waiting
     *  for a peer to pair (null when we joined someone else's code). */
    val qrPayload: String? = null,
    /** Recent throughput samples (bytes/s), for the detail drawer's speed chart. */
    val speedHistory: List<Double> = emptyList(),
    /** Timestamped log lines scoped to this transfer, for the detail drawer. */
    val log: List<String> = emptyList(),
)

val Status.isTerminal: Boolean
    get() =
        this == Status.Completed ||
            this == Status.Failed ||
            this == Status.Cancelled

enum class TransferAction { Pause, Resume, Retry, Cancel, Delete, Open }

/** Canonical action policy consumed by Activity; every rendered button must
 * correspond to an operation accepted for the current snapshot. */
fun availableTransferActions(transfer: Transfer): List<TransferAction> =
    when (transfer.status) {
        Status.Waiting,
        Status.Connecting,
        Status.Verifying,
        Status.Transferring,
        -> listOf(TransferAction.Pause, TransferAction.Cancel)
        Status.Paused -> listOf(TransferAction.Resume, TransferAction.Cancel)
        Status.Unconfirmed -> listOf(TransferAction.Retry, TransferAction.Cancel)
        Status.Publishing ->
            if (transfer.error != null) {
                listOf(TransferAction.Retry, TransferAction.Cancel)
            } else {
                emptyList()
            }
        Status.Completed ->
            buildList {
                if (transfer.savedUri != null) add(TransferAction.Open)
                add(TransferAction.Delete)
            }
        Status.Failed ->
            buildList {
                if (transfer.retryable) add(TransferAction.Retry)
                add(TransferAction.Delete)
            }
        Status.Cancelled -> listOf(TransferAction.Delete)
        Status.Confirming -> emptyList()
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
