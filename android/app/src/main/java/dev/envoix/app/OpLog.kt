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

    // The SAME clock family as the core trace (UTC ISO), so op lines and core
    // lines correspond 1:1 when read side by side or interleaved.
    private val clock = java.time.format.DateTimeFormatter.ISO_INSTANT

    fun init(filesDir: File) {
        val f = File(File(filesDir, "logs").apply { mkdirs() }, "op.log")
        file = f
        // Trim to the tail if it grew past the cap, so it never grows unbounded.
        runCatching {
            if (f.exists() && f.length() > FILE_CAP) f.writeText(f.readText().takeLast(FILE_CAP))
        }
        add("── launch · v${BuildConfig.VERSION_NAME} (${BuildConfig.GIT_COMMIT}) ──")
    }

    @Synchronized
    fun add(
        op: String,
        transferId: Long? = null,
    ) {
        val line = "${clock.format(java.time.Instant.now())}  $op"
        buffer.addLast(line)
        while (buffer.size > CAP) buffer.removeFirst()
        _lines.value = buffer.toList()
        file?.let { f ->
            runCatching {
                f.appendText(line + "\n")
                // Trim in-session too, not only at init() — a long-running
                // launch would otherwise grow op.log unbounded (breadcrumbs are
                // low-volume, so the over-cap readText/writeText is rare).
                if (f.length() > FILE_CAP) f.writeText(f.readText().takeLast(FILE_CAP))
            }
        }
        // Correspondence: every breadcrumb also lands in the core trace
        // (greppable "OP " inline with the tracing lines it explains), and in
        // the transfer's durable log when it targets a card.
        LogStore.append("OP  $op")
        if (transferId != null) TransferLogs.append(transferId, "OP  $op")
    }

    /** The persisted breadcrumbs across recent launches (for copy / upload); falls
     *  back to the in-memory buffer if the file is unreadable. */
    fun report(): String =
        file?.let { runCatching { it.readText() }.getOrNull() }?.ifBlank { null }
            ?: buffer.joinToString("\n")
}
