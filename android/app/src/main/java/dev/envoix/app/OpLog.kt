package dev.envoix.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File

/**
 * A breadcrumb trail of *user/app operations* — start / pause / resume / cancel a
 * transfer, remove one, change a setting — kept deliberately separate from the
 * core transfer trace ([LogStore]). It answers "what did the user do before the
 * problem", the way a crash reporter (Crashlytics / Sentry / Bugsnag) attaches
 * breadcrumbs to a report rather than burying them in verbose logs.
 *
 * Persisted to a single `op.log` (tail-capped, not per-transfer) so it survives a
 * crash and spans recent launches, each marked with a `── launch ──` line.
 */
object OpLog {
    private const val CAP = 400 // in-memory lines for the live view
    private const val FILE_CAP = 128 * 1024 // op.log tail on disk; breadcrumbs are small

    private val buffer = ArrayDeque<String>()
    private val _lines = MutableStateFlow<List<String>>(emptyList())
    val lines: StateFlow<List<String>> = _lines.asStateFlow()

    private var file: File? = null
    private val clock = java.time.format.DateTimeFormatter
        .ofPattern("HH:mm:ss").withZone(java.time.ZoneId.systemDefault())

    fun init(filesDir: File) {
        val f = File(File(filesDir, "logs").apply { mkdirs() }, "op.log")
        file = f
        // Trim to the tail if it grew past the cap, so it never grows unbounded.
        runCatching {
            if (f.exists() && f.length() > FILE_CAP) f.writeText(f.readText().takeLast(FILE_CAP))
        }
        add("── launch ──")
    }

    @Synchronized
    fun add(op: String) {
        val line = "${clock.format(java.time.Instant.now())}  $op"
        buffer.addLast(line)
        while (buffer.size > CAP) buffer.removeFirst()
        _lines.value = buffer.toList()
        file?.let { runCatching { it.appendText(line + "\n") } }
    }

    /** The persisted breadcrumbs across recent launches (for copy / upload); falls
     *  back to the in-memory buffer if the file is unreadable. */
    fun report(): String = file?.let { runCatching { it.readText() }.getOrNull() }?.ifBlank { null }
        ?: buffer.joinToString("\n")
}
