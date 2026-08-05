package dev.envoix.app

import java.util.Locale
import kotlin.math.floor

internal fun transferRateString(bytesPerSecond: Double): String {
    val rate = bytesPerSecond.takeIf { it.isFinite() && it > 0.0 } ?: return "0 B/s"
    return when {
        rate >= 1_000_000_000.0 -> formatTransferRate(rate / 1_000_000_000.0, 2, "GB/s")
        rate >= 1_000_000.0 -> formatTransferRate(rate / 1_000_000.0, 1, "MB/s")
        rate >= 1_000.0 -> formatTransferRate(rate / 1_000.0, 0, "KB/s")
        else -> formatTransferRate(rate, 0, "B/s")
    }
}

private fun formatTransferRate(
    value: Double,
    fractionDigits: Int,
    unit: String,
): String {
    val scale =
        when (fractionDigits) {
            2 -> 100.0
            1 -> 10.0
            else -> 1.0
        }
    val rounded = floor(value * scale + 0.5) / scale
    return String.format(Locale.ROOT, "%.${fractionDigits}f %s", rounded, unit)
}

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
 * Reduces receiver per-entry native phase reports into one aggregate transfer
 * status. Verification is only presented after all payload bytes arrived, and
 * that final verification cannot flicker back to transferring.
 */
data class TransferStatusPresentationDecision(
    val status: Status,
    val shouldPublish: Boolean,
)

object TransferStatusPresentationReducer {
    fun decide(
        direction: Direction,
        current: Status,
        reported: Status,
        bytes: Long,
        total: Long,
    ): TransferStatusPresentationDecision {
        val status =
            when {
                direction != Direction.Receive -> reported
                reported == Status.Verifying && bytes < total -> current
                current == Status.Verifying && reported == Status.Transferring -> current
                else -> reported
            }
        val redundantEntryPhase =
            direction == Direction.Receive &&
                status == current &&
                (reported == Status.Verifying || reported == Status.Transferring)
        return TransferStatusPresentationDecision(
            status = status,
            shouldPublish = !redundantEntryPhase,
        )
    }
}

/**
 * Pure lifecycle-to-presentation policy shared by every Android transfer
 * surface. Compose renders this result and does not infer actions or progress
 * behavior independently.
 */
object TransferPresentationPolicy {
    fun actions(transfer: Transfer): TransferActionAvailability {
        val state = transfer.status
        val canCancel =
            state == Status.Preparing ||
                state == Status.WaitingForPeer ||
                state == Status.Pairing ||
                state == Status.Connecting ||
                state == Status.AwaitingDecision ||
                state == Status.Transferring ||
                state == Status.Verifying ||
                state == Status.Paused
        return TransferActionAvailability(
            // Current invitation sessions consume their authentication secret
            // after pairing and Android does not restore their process-only
            // spec. A Pause/Resume control would promise a continuation that
            // cannot be honored; use Cancel and create a fresh room offer.
            canPause = false,
            canResume = false,
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
            -> TransferProgressPresentation.Complete
            Status.Delivered -> TransferProgressPresentation.Hidden
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
