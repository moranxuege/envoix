package dev.envoix.app

enum class Direction { Send, Receive }

enum class Status { Connecting, Transferring, Paused, Completed, Unconfirmed, Failed, Cancelled }

/** One transfer's observable state, shown as a card. */
data class Transfer(
    val id: Long,
    val direction: Direction,
    val room: String,
    val fileName: String? = null,
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
    /** For an initiated session, the invite payload to show as a QR while waiting
     *  for a peer to pair (null when we joined someone else's code). */
    val qrPayload: String? = null,
    /** Recent throughput samples (bytes/s), for the detail drawer's speed chart. */
    val speedHistory: List<Double> = emptyList(),
    /** Timestamped log lines scoped to this transfer, for the detail drawer. */
    val log: List<String> = emptyList(),
)

val Status.isTerminal: Boolean
    get() = this == Status.Completed || this == Status.Unconfirmed ||
        this == Status.Failed || this == Status.Cancelled
