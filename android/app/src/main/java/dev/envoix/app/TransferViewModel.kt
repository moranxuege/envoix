package dev.envoix.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import kotlinx.coroutines.flow.StateFlow

/**
 * Thin bridge between the UI and the [TransferService] + [TransferRepository]:
 * the service owns the transfers, this just issues commands and exposes state.
 */
class TransferViewModel(
    app: Application,
) : AndroidViewModel(app) {
    val transfers: StateFlow<List<Transfer>> = TransferRepository.transfers

    fun startReceive(
        room: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        destinationCopyApproved: Boolean,
    ) = TransferService.startReceive(
        getApplication(),
        room,
        broker,
        relay,
        qrPayload,
        destinationCopyApproved,
    )

    fun startSend(
        room: String,
        jobId: String,
        broker: String,
        relay: String,
        qrPayload: String?,
    ) = TransferService.startSend(getApplication(), room, broker, relay, jobId, qrPayload)

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    fun approveReceive(id: Long) = TransferService.approveReceive(getApplication(), id)

    /** Remove a transfer from the list, cancelling it first if it's still active. */
    fun remove(id: Long) {
        // D2, the one true abandon: the service tears the session down and
        // discards the partial + checkpoint state.
        TransferService.remove(getApplication(), id)
    }

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status == Status.Paused || t.status == Status.Failed || t.status == Status.Cancelled) {
            TransferService.resume(getApplication(), id)
        } else {
            TransferService.pause(getApplication(), id)
        }
    }
}
