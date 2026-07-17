package dev.envoix.app

import java.io.File

/**
 * Durable per-transfer logs, in TWO files per card (keyed by the same durable id
 * as the TransferRecord):
 *
 *   transfer-<id>.timeline.log — structured authority events (source_seq stamped).
 *                                Effectively never trimmed: the timeline is
 *                                bounded (tens of events), and only a runaway
 *                                producer would ever reach the generous safety cap.
 *   transfer-<id>.raw.log      — verbose iroh/core trace; capped, head+tail on
 *                                overflow.
 *
 * Separate files are the point (v2 P6): raw-trace VOLUME can no longer evict the
 * timeline on disk — the two share nothing. GC keeps the newest KEEP cards;
 * Remove (D2) deletes both files.
 */
object TransferLogs {
    private const val KEEP = 20

    /** Raw trace cap (the noise). */
    private const val RAW_CAP = 4L * 1024 * 1024

    /** Timeline safety cap — far above any real timeline, so it is effectively
     *  never trimmed; only a pathological producer would ever hit it. */
    private const val TIMELINE_CAP = 8L * 1024 * 1024

    private const val CHECK_EVERY = 256

    private lateinit var dir: File
    private val counts = java.util.concurrent.ConcurrentHashMap<String, Int>()

    // source_seq authority (v2 P5): one monotonic counter per transfer's timeline
    // file, covering BOTH producers (Rust core via the JNI callback + app-side
    // TransferTimeline) — no collision, seq order = true write order.
    private val seq = java.util.concurrent.ConcurrentHashMap<Long, Long>()

    fun init(filesDir: File) {
        dir = File(File(filesDir, "logs"), "transfers").apply { mkdirs() }
        gc()
    }

    private fun timelineFile(id: Long) = File(dir, "transfer-$id.timeline.log")

    private fun rawFile(id: Long) = File(dir, "transfer-$id.raw.log")

    /** Append a raw (unstructured) trace line — the verbose tier. */
    @Synchronized
    fun append(
        id: Long,
        line: String,
    ) = appendTo(rawFile(id), line, RAW_CAP)

    /**
     * Append a structured timeline line to the card's OWN timeline file, stamping
     * `source_seq` as the leading TAB column. Synchronized (same monitor as the
     * raw append) so seq assignment and the write are atomic — seq order = file
     * order — and separate from the raw file so raw volume can never evict it.
     */
    @Synchronized
    fun appendTimeline(
        id: Long,
        line: String,
    ) {
        val s = seq.merge(id, 0L) { prev, _ -> prev + 1 } ?: 0L
        appendTo(timelineFile(id), "$s\t$line", TIMELINE_CAP)
    }

    private fun appendTo(
        f: File,
        line: String,
        cap: Long,
    ) {
        if (!::dir.isInitialized) return
        runCatching { f.appendText(line + "\n") }
        val n = counts.merge(f.name, 1, Int::plus) ?: 1
        if (n % CHECK_EVERY == 0 && f.length() > cap) {
            // Keep HEAD (the beginning) AND TAIL (the failure), drop the middle.
            runCatching {
                val text = f.readText()
                val keep = (cap / 4).toInt()
                f.writeText(text.take(keep) + "\n[… middle trimmed — head & tail kept]\n" + text.takeLast(keep))
            }
        }
    }

    /** The structured timeline for a card (never budget-trimmed on disk), or "". */
    fun readTimeline(id: Long): String = if (::dir.isInitialized) runCatching { timelineFile(id).readText() }.getOrDefault("") else ""

    /** The raw trace for a card, or "". */
    fun readRaw(id: Long): String = if (::dir.isInitialized) runCatching { rawFile(id).readText() }.getOrDefault("") else ""

    /** D2: Remove deletes both of the card's log files. */
    fun delete(id: Long) {
        if (::dir.isInitialized) {
            timelineFile(id).delete()
            rawFile(id).delete()
        }
        seq.remove(id)
    }

    /** Keep the newest [KEEP] cards (each has up to two files, plus any legacy
     *  single-file `transfer-<id>.log` from before the split). */
    private fun gc() {
        val files = dir.listFiles { f -> f.name.startsWith("transfer-") } ?: return
        files
            .groupBy { it.name.substringAfter("transfer-").substringBefore('.') }
            .entries
            .sortedByDescending { (_, fs) -> fs.maxOf { it.lastModified() } }
            .drop(KEEP)
            .forEach { (_, fs) -> fs.forEach { it.delete() } }
    }
}
