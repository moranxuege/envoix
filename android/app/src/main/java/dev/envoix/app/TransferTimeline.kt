package dev.envoix.app

/**
 * App-side producer of transfer-timeline events (docs/design/diagnostics.md v2).
 *
 * Writes the SAME delimited envelope the Rust core emits, into the same durable
 * per-transfer file ([TransferLogs]), so the app's own authority events —
 * staging, publish, courier post-failures — join the one `session_id`-routed
 * timeline instead of hiding in the UI drawer or the global core ring. The
 * single [TransferLogs] writer stamps `source_seq` (v2 P5); this builds every
 * other column so a Rust line and a Kotlin line are indistinguishable to a
 * reader.
 */
object TransferTimeline {
    private const val SCHEMA = 1

    /**
     * Emit one event for card [id]. Fixed columns are safe by construction
     * (controlled identifiers / digits); [fields] values are redacted at the
     * call site and escaped here.
     */
    fun event(
        id: Long,
        layer: String,
        event: String,
        outcome: String = "",
        side: String = "",
        attempt: String = "",
        fields: Map<String, String> = emptyMap(),
    ) {
        val sb = StringBuilder(64)
        sb
            .append(SCHEMA)
            .append('\t')
            .append(System.currentTimeMillis())
            .append('\t')
            .append(android.os.Process.myPid()) // == Rust std::process::id() (same process)
            .append('\t')
            .append(id)
            .append('\t')
            .append(attempt)
            .append('\t')
            .append(side)
            .append('\t')
            .append(layer)
            .append('\t')
            .append(event)
            .append('\t')
            .append(outcome)
        for ((k, v) in fields) {
            sb
                .append('\t')
                .append(k)
                .append('=')
                .append(escape(v))
        }
        TransferLogs.appendTimeline(id, sb.toString())
    }

    /** Percent-encode ONLY the three delimiter-breaking octets (matches Rust). */
    private fun escape(value: String): String =
        value
            .replace("%", "%25")
            .replace("\t", "%09")
            .replace("\n", "%0A")

    // --- redaction: never let a full URI / path / secret reach an uploaded
    // timeline (v2 "Redaction"). ---

    /** A content:// or file URI → scheme + display name only; the tree/provider
     *  path (a device-scoped identifier) is dropped. */
    fun redactUri(uri: String): String =
        runCatching {
            val u = android.net.Uri.parse(uri)
            "${u.scheme}:…/${u.lastPathSegment ?: "?"}"
        }.getOrDefault("uri")

    /** A filesystem path → basename only. */
    fun redactPath(path: String): String = path.substringAfterLast('/')
}
