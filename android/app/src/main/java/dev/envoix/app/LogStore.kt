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
    private const val FILE_CAP_BYTES = 32L * 1024 * 1024 // rotate past 32 MB (-vvv fills 8 MB in minutes)
    private const val KEEP = 3 // previous-launch logs to retain: core-1.log .. core-3.log

    private val buffer = ArrayDeque<String>()
    private val _lines = MutableStateFlow<List<String>>(emptyList())
    val lines: StateFlow<List<String>> = _lines.asStateFlow()

    private var logDir: File? = null
    private var sink: java.io.FileOutputStream? = null
    private var written = 0L

    fun init(filesDir: File) {
        val dir = File(filesDir, "logs").apply { mkdirs() }
        logDir = dir
        // A launch boundary: retain the last KEEP sessions (core-1..core-KEEP), then
        // start fresh. A session may end in a native crash that skips the JVM handler,
        // so its log must survive several relaunches, not just the next one.
        shiftRing(dir)
        sink = runCatching { java.io.FileOutputStream(File(dir, "core.log")) }.getOrNull()
        written = 0L
    }

    @Synchronized
    fun append(line: String) {
        buffer.addLast(line)
        while (buffer.size > CAP) buffer.removeFirst()
        _lines.value = buffer.toList()
        persist(line)
    }

    /** Append the line to the on-disk log unbuffered, so the OS retains it even if
     *  the process aborts (a native crash bypasses the JVM uncaught handler).
     *  Rotates to core-prev.log past a size cap so it never grows without bound. */
    private fun persist(line: String) {
        val out = sink ?: return
        runCatching {
            val bytes = (line + "\n").toByteArray()
            out.write(bytes)
            written += bytes.size
            if (written >= FILE_CAP_BYTES) rotate()
        }
    }

    private fun rotate() {
        val dir = logDir ?: return
        runCatching {
            sink?.close()
            shiftRing(dir)
            sink = java.io.FileOutputStream(File(dir, "core.log"))
            written = 0L
        }
    }

    /** Shift the retained ring one slot: core.log -> core-1 -> core-2 -> ... ->
     *  core-KEEP, dropping the oldest. Folds the legacy single core-prev.log in on
     *  first run so an upgrade doesn't lose the last crash log. */
    private fun shiftRing(dir: File) {
        File(dir, "core-prev.log").let { legacy ->
            if (legacy.exists() && !File(dir, "core-1.log").exists()) {
                legacy.renameTo(File(dir, "core-1.log"))
            } else {
                legacy.delete()
            }
        }
        File(dir, "core-$KEEP.log").delete()
        for (i in KEEP downTo 2) File(dir, "core-${i - 1}.log").renameTo(File(dir, "core-$i.log"))
        File(dir, "core.log").renameTo(File(dir, "core-1.log"))
    }

    @Synchronized
    fun dump(): String = buffer.joinToString("\n")

    @Synchronized
    fun clear() {
        buffer.clear()
        _lines.value = emptyList()
    }

    /** A retained session log: its file, a display label, and byte size. */
    data class Session(
        val file: File,
        val label: String,
        val bytes: Long,
    )

    /** The retained session logs, newest first: the current session (core.log)
     *  then the last [KEEP] launches. Only existing, non-empty files. Backs the
     *  dev-mode "previous sessions" log UI (view / copy / upload). */
    fun sessions(): List<Session> {
        val dir = logDir ?: return emptyList()
        val current = File(dir, "core.log")
        val files = listOf(current) + (1..KEEP).map { File(dir, "core-$it.log") }
        return files.filter { it.exists() && it.length() > 0 }.mapIndexed { i, f ->
            Session(f, if (f == current) "Current session" else "$i launch(es) ago", f.length())
        }
    }

    /** Read a retained session log's full text (for copy / upload). */
    fun readSession(file: File): String = runCatching { file.readText() }.getOrDefault("")

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
 * The native log sink, wired via [Native.initLogging]. Every core line goes to
 * the whole-app [LogStore]; lines scoped to a transfer (the `tracing` span
 * carries `room="…"`) are also compacted and routed into that transfer's own
 * log, so the detail drawer shows the real core story, not just lifecycle events.
 */
object LogSink : LogCallback {
    // "<LEVEL> <spans>: <message>" — re-stamped in local time so core lines
    // line up with the app's own event lines.
    private val LEVEL = Regex("""\s(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)""")

    // the outer transfer{…} span is redundant in a per-transfer log — drop it
    private val TRANSFER_SPAN = Regex("""^transfer\{[^}]*\}:\s*""")
    private val clock =
        java.time.format.DateTimeFormatter
            .ofPattern("HH:mm:ss")
            .withZone(java.time.ZoneId.systemDefault())

    /** [room] is the span field, extracted STRUCTURALLY by the JNI tracing
     *  layer (see docs/design/diagnostics.md) — no more regex on formatted text. */
    override fun log(
        room: String?,
        line: String,
    ) {
        LogStore.append(line)
        if (room.isNullOrEmpty()) return
        val m = LEVEL.find(line) ?: return
        val (level, tail) = m.destructured
        val stamp = clock.format(java.time.Instant.now())
        val compact = "$stamp  $level  ${tail.replaceFirst(TRANSFER_SPAN, "")}"
        val id = TransferRepository.appendCoreLog(room, compact) ?: return
        TransferLogs.append(id, compact)
    }

    /** A structured timeline line, routed directly by durable card id. */
    override fun timeline(
        sessionId: Long,
        line: String,
    ) {
        TransferLogs.appendTimeline(sessionId, line)
    }
}
