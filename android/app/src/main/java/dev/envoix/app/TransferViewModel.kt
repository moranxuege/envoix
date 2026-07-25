package dev.envoix.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

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

    /**
     * Starts a receiver and reports only after its native Manifest session has
     * been launched. Room control must not acknowledge an offer before this
     * barrier, otherwise a fast sender can race a receiver that is not bound.
     */
    fun startReceiveWhenReady(
        room: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        destinationCopyApproved: Boolean,
        completion: (id: Long, error: String?) -> Unit,
    ) {
        val id =
            runCatching {
                startReceive(
                    room,
                    broker,
                    relay,
                    qrPayload,
                    destinationCopyApproved,
                )
            }.getOrElse { error ->
                completion(-1L, error.message ?: "The receiver could not start")
                return
            }
        viewModelScope.launch {
            val ready =
                withTimeoutOrNull(RECEIVER_START_TIMEOUT_MS) {
                    transfers
                        .mapNotNull { list -> list.firstOrNull { it.id == id } }
                        .first { it.status != Status.Connecting }
                }
            val error =
                when {
                    ready == null -> "The receiver did not start in time"
                    ready.status == Status.Failed ->
                        ready.error ?: "The receiver could not start"
                    ready.status == Status.Canceled ->
                        "The receiver was canceled before it became ready"
                    else -> null
                }
            completion(id, error)
        }
    }

    fun startSend(
        room: String,
        jobId: String,
        broker: String,
        relay: String,
        qrPayload: String?,
    ) = TransferService.startSend(getApplication(), room, broker, relay, jobId, qrPayload)

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

    private companion object {
        const val RECEIVER_START_TIMEOUT_MS = 10_000L
    }
}
