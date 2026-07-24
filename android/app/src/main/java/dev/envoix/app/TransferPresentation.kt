package dev.envoix.app

data class TransferActionAvailability(
    val canPause: Boolean = false,
    val canResume: Boolean = false,
    val canCancel: Boolean = false,
    val canApprove: Boolean = false,
    val canRemove: Boolean = false,
    val isFinalizing: Boolean = false,
)

enum class TransferProgressPresentation {
    Hidden,
    Active,
    Complete,
    Retained,
}

/**
 * Pure lifecycle-to-presentation policy shared by every Android transfer
 * surface. Compose renders this result and does not infer actions or progress
 * behavior independently.
 */
object TransferPresentationPolicy {
    fun actions(transfer: Transfer): TransferActionAvailability {
        val state = transfer.status
        val canPause =
            when (state) {
                Status.WaitingForPeer,
                Status.Pairing,
                Status.Connecting,
                Status.Transferring,
                Status.Verifying,
                -> true
                else -> false
            }
        val canCancel =
            canPause ||
                state == Status.Preparing ||
                state == Status.AwaitingDecision ||
                state == Status.Paused
        return TransferActionAvailability(
            canPause = canPause,
            canResume = state == Status.Paused || (state == Status.Failed && transfer.retryable),
            canCancel = canCancel,
            canApprove = state == Status.AwaitingDecision,
            canRemove = isTerminal(state),
            isFinalizing = isFinalizing(state),
        )
    }

    fun progress(state: Status): TransferProgressPresentation =
        when (state) {
            Status.Preparing,
            Status.WaitingForPeer,
            Status.Pairing,
            Status.Connecting,
            Status.AwaitingDecision,
            -> TransferProgressPresentation.Hidden
            Status.Transferring -> TransferProgressPresentation.Active
            Status.Verifying,
            Status.Saving,
            Status.WaitingForReceiverSave,
            Status.FinalizingDelivery,
            Status.Delivered,
            -> TransferProgressPresentation.Complete
            Status.Paused,
            Status.Failed,
            Status.Canceled,
            -> TransferProgressPresentation.Retained
        }

    fun isFinalizing(state: Status): Boolean =
        state == Status.Saving ||
            state == Status.WaitingForReceiverSave ||
            state == Status.FinalizingDelivery

    fun isTerminal(state: Status): Boolean =
        state == Status.Delivered ||
            state == Status.Failed ||
            state == Status.Canceled
}
