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

    fun startReceive(room: String) =
        TransferService.start(getApplication(), "receive", room, incomingDir.absolutePath)

    fun startSend(room: String, filePath: String) =
        TransferService.start(getApplication(), "send", room, filePath)

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    fun dismiss(id: Long) = TransferRepository.remove(id)
}
