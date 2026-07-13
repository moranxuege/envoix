package dev.envoix.app

import java.io.File

/**
 * THE report assembler (docs/design/diagnostics.md): every copy/upload surface
 * builds its payload here, and nowhere else assembles or truncates. Sections
 * by priority — header > crash > transfer > ops > core — trimmed tail-first
 * into the rdz body budget.
 */
object Diagnostics {
    /** Clipboard cap (Binder transactions die near 1 MB). */
    const val CLIP_MAX = 256 * 1024

    /** rdz log endpoint body cap. */
    const val UPLOAD_MAX = 480 * 1024
    private const val OPS_MAX = 32 * 1024
    private const val TRANSFER_MAX = 256 * 1024
    private const val CRASH_MAX = 64 * 1024

    enum class Kind { Transfer, App, Crash }

    private lateinit var filesDir: File

    fun init(dir: File) {
        filesDir = dir
    }

    private fun crashFile() = File(File(filesDir, "logs"), "crash-latest.log")

    private fun ackFile() = File(File(filesDir, "logs"), "crash-acked")

    /** A crash newer than the last acknowledgement is pending report. */
    fun pendingCrash(): Boolean {
        val crash = crashFile()
        if (!crash.exists()) return false
        val acked = runCatching { ackFile().readText().trim().toLong() }.getOrDefault(0L)
        return crash.lastModified() > acked
    }

    /** Acknowledge the current crash (uploaded OR dismissed — never nag twice). */
    fun ackCrash() {
        runCatching { ackFile().writeText(crashFile().lastModified().toString()) }
    }

    /** Build one report, within [budget] bytes. In debug builds nothing is
     *  trimmed (server space is not a concern pre-release, and a clipped
     *  diagnostic is unusable); release keeps the caps. */
    fun build(
        kind: Kind,
        transferId: Long? = null,
        budget: Int = if (BuildConfig.DEBUG) Int.MAX_VALUE else UPLOAD_MAX,
    ): String {
        val full = BuildConfig.DEBUG && budget == Int.MAX_VALUE

        fun cap(release: Int) = if (full) Int.MAX_VALUE else release
        // Each section: (text, byte cap, keepHeadAndTail). The timeline is the
        // authority — emitted FIRST and uncapped (it is bounded, tens of events),
        // so under a tight release budget it survives intact and the verbose raw
        // trace is what yields space. The raw trace keeps its HEAD and TAIL (the
        // connection setup AND the failure), not tail-only (v2 P6).
        val sections =
            buildList {
                add(Triple("── envoix-android v${BuildConfig.VERSION_NAME} (${BuildConfig.GIT_COMMIT}) · $kind ──", 512, false))
                if (kind == Kind.Crash) {
                    add(Triple(section("crash", runCatching { crashFile().readText() }.getOrDefault("")), cap(CRASH_MAX), false))
                }
                if (kind == Kind.Transfer && transferId != null) {
                    val (timeline, raw) = splitTransfer(transferId)
                    add(Triple(section("timeline $transferId", timeline), Int.MAX_VALUE, false))
                    add(Triple(section("transfer raw trace", raw), cap(TRANSFER_MAX), true))
                }
                add(Triple(section("operations", OpLog.report()), cap(OPS_MAX), false))
                add(Triple(section("core trace (tail)", LogStore.dump()), Int.MAX_VALUE, false))
            }
        var remaining = budget
        val out = StringBuilder()
        for ((text, cap, headAndTail) in sections) {
            val allowed = minOf(cap, remaining)
            val piece = if (headAndTail) headAndTail(text, allowed) else tail(text, allowed)
            out.append(piece).append('\n')
            remaining -= piece.toByteArray().size + 1
            if (remaining <= 0) break
        }
        return out.toString()
    }

    // A structured timeline line begins: <source_seq>\t<schema>\t<epoch-ms>\t…
    private val TIMELINE_LINE = Regex("""^\d+\t\d+\t\d{13}\t""")

    /** Split a transfer's durable log into (structured timeline, raw trace) by
     *  line shape — the two tiers coexist in one file (v2), separated here. */
    private fun splitTransfer(id: Long): Pair<String, String> {
        val timeline = StringBuilder()
        val raw = StringBuilder()
        for (line in TransferLogs.read(id).lineSequence()) {
            when {
                line.isEmpty() -> {}
                TIMELINE_LINE.containsMatchIn(line) -> timeline.append(line).append('\n')
                else -> raw.append(line).append('\n')
            }
        }
        return timeline.toString() to raw.toString()
    }

    private fun section(
        name: String,
        body: String,
    ) = "\n══════ $name ══════\n" + body.ifBlank { "(empty)" }

    /** Last bytes, marked when clipped — failures live at the tail. The marker's
     *  own bytes are RESERVED, so the result never exceeds [maxBytes]. */
    fun tail(
        text: String,
        maxBytes: Int,
    ): String {
        val bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.size <= maxBytes) return text
        val note = "[… trimmed — tail of ${bytes.size / 1024} KB]\n"
        val room = (maxBytes - note.toByteArray(Charsets.UTF_8).size).coerceAtLeast(0)
        return note + String(bytes, bytes.size - room, room, Charsets.UTF_8)
    }

    /** First AND last bytes, marked when clipped — for the raw trace, where the
     *  connection setup (head) matters as much as the failure (tail). The marker
     *  is reserved, so the result never exceeds [maxBytes]. */
    fun headAndTail(
        text: String,
        maxBytes: Int,
    ): String {
        val bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.size <= maxBytes) return text
        val note = "\n[… middle trimmed — head & tail of ${bytes.size / 1024} KB …]\n"
        val room = (maxBytes - note.toByteArray(Charsets.UTF_8).size).coerceAtLeast(0)
        val half = room / 2
        return String(bytes, 0, half, Charsets.UTF_8) + note + String(bytes, bytes.size - half, half, Charsets.UTF_8)
    }
}
