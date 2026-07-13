package dev.envoix.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import kotlinx.coroutines.flow.StateFlow
import java.io.File

/**
 * Thin bridge between the UI and the [TransferService] + [TransferRepository]:
 * the service owns the transfers, this just issues commands and exposes state.
 */
class TransferViewModel(
    app: Application,
) : AndroidViewModel(app) {
    private val incomingDir = File(app.filesDir, "incoming").apply { mkdirs() }

    val transfers: StateFlow<List<Transfer>> = TransferRepository.transfers

    fun startReceive(
        room: String,
        broker: String,
        relay: String,
        qrPayload: String?,
    ) = start("receive", room, incomingDir.absolutePath, broker, relay, qrPayload, null)

    fun startSend(
        room: String,
        filePath: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        transferInvite: String?,
    ) = start("send", room, filePath, broker, relay, qrPayload, transferInvite)

    private fun start(
        direction: String,
        room: String,
        path: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        transferInvite: String?,
    ) {
        val config = SettingsStore.renderConfig(getApplication()) ?: ""
        TransferService.start(getApplication(), direction, room, path, broker, relay, config, qrPayload, transferInvite)
    }

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    /** Delete a finished/cancelled/failed history item. Active transfers must be cancelled explicitly. */
    fun remove(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id }
        if (t != null && t.status.isTerminal) TransferService.remove(getApplication(), id)
    }

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (
            t.status == Status.Paused ||
            t.status == Status.Failed ||
            t.status == Status.Unconfirmed ||
            t.status == Status.Publishing
        ) {
            TransferService.resume(getApplication(), id)
        } else {
            TransferService.pause(getApplication(), id)
        }
    }
}
