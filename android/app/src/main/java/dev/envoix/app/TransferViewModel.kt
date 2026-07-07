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

    fun startReceive(room: String) = start("receive", room, incomingDir.absolutePath)

    fun startSend(room: String, filePath: String) = start("send", room, filePath)

    private fun start(direction: String, room: String, path: String) {
        val s = SettingsStore.settings.value
        val config = SettingsStore.renderConfig(getApplication()) ?: ""
        TransferService.start(getApplication(), direction, room, path, s.broker, s.relay, config)
    }

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    /** Remove a (terminal) transfer from the list. */
    fun remove(id: Long) = TransferRepository.remove(id)

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status == Status.Paused || t.status == Status.Failed)
            TransferService.resume(getApplication(), id)
        else
            TransferService.pause(getApplication(), id)
    }
}
