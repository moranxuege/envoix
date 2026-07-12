package dev.envoix.app

import java.io.File

/**
 * Durable per-transfer log files: `logs/transfers/transfer-<id>.log`, keyed by
 * the SAME durable id as the TransferRecord (`record-<id>.json`). Survives the
 * core-trace ring churning at -vvv (the field lesson: a needed room's story
 * rotated away mid-debug). GC by count; size-capped per file; Remove (D2)
 * deletes a card's file with the card.
 */
object TransferLogs {
    private const val KEEP = 20
    private const val FILE_CAP = 4L * 1024 * 1024
    private const val CHECK_EVERY = 256

    private lateinit var dir: File
    private val counts = java.util.concurrent.ConcurrentHashMap<Long, Int>()

    // source_seq authority (docs/design/diagnostics.md v2, P5): one monotonic
    // counter per transfer file. This single writer covers BOTH producers — the
    // Rust core (via the JNI timeline callback) and app-side TransferTimeline —
    // so the two never collide, and seq order is the true write order.
    private val seq = java.util.concurrent.ConcurrentHashMap<Long, Long>()

    fun init(filesDir: File) {
        dir = File(File(filesDir, "logs"), "transfers").apply { mkdirs() }
        gc()
    }

    private fun file(id: Long) = File(dir, "transfer-$id.log")

    /** Append one (already formatted) line to a transfer's durable log. */
    @Synchronized
    fun append(
        id: Long,
        line: String,
    ) {
        if (!::dir.isInitialized) return
        val f = file(id)
        runCatching { f.appendText(line + "\n") }
        val n = counts.merge(id, 1, Int::plus) ?: 1
        if (n % CHECK_EVERY == 0 && f.length() > FILE_CAP) {
            // keep the newest half; the tail is where failures live
            runCatching {
                val text = f.readText()
                f.writeText("[… truncated — newest half kept]\n" + text.takeLast((FILE_CAP / 2).toInt()))
            }
        }
    }

    /**
     * Append a structured timeline line, stamping `source_seq` as the leading
     * TAB column. Synchronized with [append] (same monitor) so seq assignment
     * and the write are atomic together — seq order equals file order.
     */
    @Synchronized
    fun appendTimeline(
        id: Long,
        line: String,
    ) {
        val s = seq.merge(id, 0L) { prev, _ -> prev + 1 } ?: 0L
        append(id, "$s\t$line")
    }

    /** The complete durable log for a card, or "" when none. */
    fun read(id: Long): String = if (::dir.isInitialized) runCatching { file(id).readText() }.getOrDefault("") else ""

    /** D2: Remove deletes the card's log with the card. */
    fun delete(id: Long) {
        if (::dir.isInitialized) file(id).delete()
        seq.remove(id)
    }

    /** Keep only the newest [KEEP] files. */
    private fun gc() {
        val files = dir.listFiles { f -> f.name.startsWith("transfer-") } ?: return
        files.sortedByDescending { it.lastModified() }.drop(KEEP).forEach { it.delete() }
    }
}
