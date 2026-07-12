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
        val sections =
            buildList {
                add("── envoix-android v${BuildConfig.VERSION_NAME} (${BuildConfig.GIT_COMMIT}) · $kind ──" to 512)
                if (kind == Kind.Crash) add(section("crash", runCatching { crashFile().readText() }.getOrDefault("")) to cap(CRASH_MAX))
                if (kind == Kind.Transfer && transferId != null) {
                    // The structured timeline is the authority — emitted first
                    // and uncapped (it is bounded, tens of events); the verbose
                    // raw iroh trace is the appendix that yields space (v2 P6).
                    val (timeline, raw) = splitTransfer(transferId)
                    add(section("timeline $transferId", timeline) to Int.MAX_VALUE)
                    add(section("transfer raw trace", raw) to cap(TRANSFER_MAX))
                }
                add(section("operations", OpLog.report()) to cap(OPS_MAX))
                add(section("core trace (tail)", LogStore.dump()) to Int.MAX_VALUE)
            }
        // Fixed-cap sections first; core gets whatever budget remains.
        var remaining = budget
        val out = StringBuilder()
        for ((text, cap) in sections) {
            val allowed = minOf(cap, remaining)
            val piece = tail(text, allowed)
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

    /** Last [maxBytes] UTF-8 bytes, marked when clipped — failures live at the tail. */
    fun tail(
        text: String,
        maxBytes: Int,
    ): String {
        val bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.size <= maxBytes) return text
        val note = "[… trimmed — last ${maxBytes / 1024} KB of ${bytes.size / 1024} KB]\n"
        return note + String(bytes, bytes.size - maxBytes, maxBytes, Charsets.UTF_8)
    }
}
