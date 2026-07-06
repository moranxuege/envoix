package dev.envoix.app

enum class Direction { Send, Receive }

enum class Status { Connecting, Transferring, Completed, Failed, Cancelled }

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
    val speedBps: Double = 0.0,
    val status: Status = Status.Connecting,
    val error: String? = null,
    /** Where a received file ended up (a `content://` in Downloads), for opening. */
    val savedUri: String? = null,
)

val Status.isTerminal: Boolean
    get() = this == Status.Completed || this == Status.Failed || this == Status.Cancelled
