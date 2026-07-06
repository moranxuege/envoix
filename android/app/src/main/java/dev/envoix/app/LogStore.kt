package dev.envoix.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/**
 * In-memory ring buffer of log lines from the native core (via [LogSink]) and
 * the app itself, exposed to the UI and dumpable for copy/share. The crash dump
 * is the seam a future "report on crash / report to server" feature builds on.
 */
object LogStore {
    private const val CAP = 4000

    private val buffer = ArrayDeque<String>()
    private val _lines = MutableStateFlow<List<String>>(emptyList())
    val lines: StateFlow<List<String>> = _lines.asStateFlow()

    private var logDir: File? = null

    fun init(filesDir: File) {
        logDir = File(filesDir, "logs").apply { mkdirs() }
    }

    @Synchronized
    fun append(line: String) {
        buffer.addLast(line)
        while (buffer.size > CAP) buffer.removeFirst()
        _lines.value = buffer.toList()
    }

    @Synchronized
    fun dump(): String = buffer.joinToString("\n")

    @Synchronized
    fun clear() {
        buffer.clear()
        _lines.value = emptyList()
    }

    /** Persist the current buffer + an uncaught trace to a fixed path; a later
     *  "report on crash" reads this on the next launch and offers to upload it. */
    fun writeCrash(t: Throwable): File? {
        val dir = logDir ?: return null
        return runCatching {
            File(dir, "crash-latest.log").apply {
                writeText(dump() + "\n\n=== UNCAUGHT ===\n" + t.stackTraceToString())
            }
        }.getOrNull()
    }
}

/** The native log sink, wired via [Native.initLogging]. */
object LogSink : LogCallback {
    override fun log(line: String) = LogStore.append(line)
}
