package dev.envoix.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/**
 * Thin bridge between the UI and the [TransferService] + [TransferRepository]:
 * the service owns the transfers, this just issues commands and exposes state.
 */
class TransferViewModel(app: Application) : AndroidViewModel(app) {
    private val incomingDir = File(app.filesDir, "incoming").apply { mkdirs() }

    val transfers: StateFlow<List<Transfer>> = TransferRepository.transfers

    fun startReceive(room: String, broker: String, relay: String, qrPayload: String?) =
        start("receive", room, incomingDir.absolutePath, broker, relay, qrPayload)

    fun startSend(room: String, filePath: String, broker: String, relay: String, qrPayload: String?) =
        start("send", room, filePath, broker, relay, qrPayload)

    private fun start(direction: String, room: String, path: String, broker: String, relay: String, qrPayload: String?) {
        val cfg = SettingsStore.settings.value
        TransferService.start(
            getApplication(), direction, room, path, broker, relay,
            cfg.chunkSize, cfg.candidatesAllow.joinToString(","), cfg.candidatesDeny.joinToString(","),
            qrPayload,
        )
    }

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    /** Remove a transfer from the list, cancelling it first if it's still active. */
    fun remove(id: Long) {
        OpLog.add("remove transfer id=$id")
        val t = TransferRepository.transfers.value.find { it.id == id }
        if (t != null && !t.status.isTerminal) TransferService.cancel(getApplication(), id)
        TransferRepository.remove(id)
    }

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status == Status.Paused || t.status == Status.Failed ||
            t.status == Status.Unconfirmed || t.status == Status.Cancelled ||
            // A Done RECEIVER can re-join to serve the peer's re-verify: the
            // completion receipt re-delivers its lost CompleteAck, no bytes
            // re-sent. (A Done sender is already confirmed - nothing to re-join.)
            (t.status == Status.Completed && t.direction == Direction.Receive))
            TransferService.resume(getApplication(), id)
        else
            TransferService.pause(getApplication(), id)
    }
}
