package dev.envoix.app

import android.util.Log
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

/**
 * The native log sink, wired via [NativeBootstrap.initLogging]. Every core line goes to
 * the whole-app [LogStore]; lines scoped to a transfer (the `tracing` span
 * carries `room="…"`) are also compacted and routed into that transfer's own
 * log, so the detail drawer shows the real core story, not just lifecycle events.
 */
object LogSink : LogCallback {
    private val ROOM = Regex("""room="([^"]+)"""")
    // "<LEVEL> <spans>: <message>" — we re-stamp in local time so core lines line
    // up with the app's own event lines instead of the core's UTC timestamp.
    private val LEVEL = Regex("""\s(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)""")
    // the outer transfer{…} span is redundant in a per-transfer log — drop it
    private val TRANSFER_SPAN = Regex("""^transfer\{[^}]*\}:\s*""")
    private val clock =
        java.time.format.DateTimeFormatter.ofPattern("HH:mm:ss").withZone(java.time.ZoneId.systemDefault())

    override fun log(line: String) {
        LogStore.append(line)
        val m = LEVEL.find(line) ?: return
        val (level, tail) = m.destructured
        writeLogcat(level, line)
        val room = ROOM.find(line)?.groupValues?.get(1) ?: return
        val stamp = clock.format(java.time.Instant.now())
        TransferRepository.appendCoreLog(room, "$stamp  $level  ${tail.replaceFirst(TRANSFER_SPAN, "")}")
    }

    private fun writeLogcat(level: String, line: String) {
        when (level) {
            "TRACE" -> Log.v("Envoix", line)
            "DEBUG" -> Log.d("Envoix", line)
            "INFO" -> Log.i("Envoix", line)
            "WARN" -> Log.w("Envoix", line)
            "ERROR" -> Log.e("Envoix", line)
            else -> Log.d("Envoix", line)
        }
    }
}
