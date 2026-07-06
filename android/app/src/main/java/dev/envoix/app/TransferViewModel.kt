package dev.envoix.app

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.io.File

enum class Direction { Send, Receive }

enum class Status { Connecting, Transferring, Completed, Failed, Cancelled }

data class Transfer(
    val id: Long,
    val direction: Direction,
    val room: String,
    val fileName: String? = null,
    val pathType: String? = null,
    val pathAddr: String? = null,
    val bytes: Long = 0,
    val total: Long = 0,
    val speedBps: Double = 0.0,
    val status: Status = Status.Connecting,
    val error: String? = null,
)

/** Defaults point at the deployed Envoix broker + relay. */
object Endpoints {
    const val BROKER =
        "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    const val RELAY = "https://envoix.chkxwlyh.us:8444"
}

class TransferViewModel(app: Application) : AndroidViewModel(app) {
    private val outputDir = File(app.getExternalFilesDir(null), "received").apply { mkdirs() }

    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    private var nextId = 1L

    val receivedDir: String get() = outputDir.absolutePath

    fun startReceive(room: String) =
        launchTransfer(Direction.Receive, room, "receive", outputDir.absolutePath)

    fun startSend(room: String, filePath: String) =
        launchTransfer(Direction.Send, room, "send", filePath)

    private fun launchTransfer(dir: Direction, room: String, direction: String, path: String) {
        val id = nextId++
        upsert(Transfer(id = id, direction = dir, room = room))
        LogStore.append("app: start $direction room=$room path=$path")
        viewModelScope.launch {
            var lastTs = 0L
            var lastBytes = 0L
            NativeTransfer.run(direction, room, Endpoints.BROKER, Endpoints.RELAY, path)
                .collect { ev ->
                    update(id) { t ->
                        when (ev) {
                            CliEvent.Binding, CliEvent.Connecting ->
                                t.copy(status = Status.Connecting)
                            is CliEvent.Connected ->
                                t.copy(pathType = ev.pathType, pathAddr = ev.addr)
                            is CliEvent.Started ->
                                t.copy(fileName = ev.fileName, total = ev.totalBytes, status = Status.Transferring)
                            is CliEvent.Progress -> {
                                val now = System.currentTimeMillis()
                                val bps = if (lastTs > 0 && now > lastTs)
                                    (ev.bytesTransferred - lastBytes) * 1000.0 / (now - lastTs)
                                else t.speedBps
                                lastTs = now; lastBytes = ev.bytesTransferred
                                t.copy(bytes = ev.bytesTransferred, total = ev.totalBytes, speedBps = bps, status = Status.Transferring)
                            }
                            is CliEvent.Completed ->
                                t.copy(bytes = ev.bytesTransferred, speedBps = 0.0, status = Status.Completed)
                            is CliEvent.Failed ->
                                t.copy(status = Status.Failed, error = ev.error)
                            is CliEvent.Exit ->
                                if (t.status == Status.Connecting || t.status == Status.Transferring)
                                    t.copy(status = if (ev.code == 0) Status.Completed else Status.Failed)
                                else t
                        }
                    }
                }
        }
    }

    fun cancel(id: Long) =
        update(id) { if (it.status == Status.Completed) it else it.copy(status = Status.Cancelled) }

    fun dismiss(id: Long) {
        _transfers.value = _transfers.value.filterNot { it.id == id }
    }

    private fun upsert(t: Transfer) {
        _transfers.value = _transfers.value.filterNot { it.id == t.id } + t
    }

    private inline fun update(id: Long, crossinline f: (Transfer) -> Transfer) {
        _transfers.value = _transfers.value.map { if (it.id == id) f(it) else it }
    }
}
