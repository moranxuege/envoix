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
        rememberLabel: String?,
        rememberedRelationshipId: String?,
    ) = TransferService.startReceive(
        getApplication(),
        room,
        broker,
        relay,
        qrPayload,
        destinationCopyApproved,
        rememberLabel,
        rememberedRelationshipId,
    )

    fun startSend(
        room: String,
        jobId: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        rememberLabel: String?,
        rememberedRelationshipId: String?,
    ) = TransferService.startSend(
        getApplication(),
        room,
        broker,
        relay,
        jobId,
        qrPayload,
        rememberLabel,
        rememberedRelationshipId,
    )

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    fun approveReceive(id: Long) = TransferService.approveReceive(getApplication(), id)

    /** Remove a terminal transfer and its private artifacts from the list. */
    fun remove(id: Long) {
        // The service validates the terminal state before discarding private
        // session artifacts and the persisted transfer record.
        TransferService.remove(getApplication(), id)
    }

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        val actions = TransferPresentationPolicy.actions(t)
        when {
            actions.canResume -> TransferService.resume(getApplication(), id)
            actions.canPause -> TransferService.pause(getApplication(), id)
        }
    }
}
