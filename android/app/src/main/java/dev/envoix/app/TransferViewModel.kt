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
    ) = start("receive", room, incomingDir.absolutePath, broker, relay, qrPayload)

    fun startSend(
        room: String,
        sourceUri: String,
        broker: String,
        relay: String,
        qrPayload: String?,
    ) = start("send", room, "", broker, relay, qrPayload, sourceUri)

    private fun start(
        direction: String,
        room: String,
        path: String,
        broker: String,
        relay: String,
        qrPayload: String?,
        sourceUri: String? = null,
    ) {
        val cfg = SettingsStore.settings.value
        TransferService.start(
            getApplication(),
            direction,
            room,
            path,
            broker,
            relay,
            cfg.chunkSize,
            cfg.dataStreamWindow,
            cfg.candidatesAllow.joinToString(","),
            cfg.candidatesDeny.joinToString(","),
            qrPayload,
            sourceUri,
        )
    }

    fun cancel(id: Long) = TransferService.cancel(getApplication(), id)

    /** Remove a transfer from the list, cancelling it first if it's still active. */
    fun remove(id: Long) {
        // D2, the one true abandon: the service tears the session down and
        // discards the partial + resume state + receipt.
        TransferService.remove(getApplication(), id)
    }

    /** Pause a running transfer, or resume/retry a paused/failed one. */
    fun pauseResume(id: Long) {
        val t = TransferRepository.transfers.value.find { it.id == id } ?: return
        if (t.status == Status.Paused ||
            t.status == Status.Failed ||
            t.status == Status.Unconfirmed ||
            t.status == Status.Cancelled
        ) {
            TransferService.resume(getApplication(), id)
        } else if (t.status == Status.Completed && t.direction == Direction.Receive) {
            // Courier-tier service: serves the peer's re-verify; the card and
            // the machine stay Completed (mailbox-unreachable fallback).
            TransferService.reverify(getApplication(), id)
        } else {
            TransferService.pause(getApplication(), id)
        }
    }
}
