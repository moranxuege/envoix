package app.envoix.host

import android.content.Intent
import android.util.Log

/**
 * Debug-only instrumentation for `packaged_process_death_preserves_cards`.
 *
 * It exists ONLY in the debug source set — the release variant's own
 * `handleInstrumentation` forwards to nothing, so neither this class nor these
 * JNI bindings are in a release dex at all. The debug manifest puts the exported
 * service behind a signature-level permission, so no other installed app can
 * reach it (the shell needs `adb root`).
 */
object E2eBridge {
    /** Handles one instrumentation action; false = not an instrumentation intent. */
    fun handle(
        intent: Intent,
        packageName: String,
    ): Boolean {
        when (intent.action) {
            "$packageName.action.e2e-create" -> {
                val name = intent.getStringExtra("name") ?: "e2e.bin"
                val total = intent.getLongExtra("total", 1024L)
                Log.i(TAG, "created=%016x".format(createForE2e(name, total)))
            }

            "$packageName.action.e2e-probe" ->
                Log.i(TAG, "restored=${liveCards().substringBefore(DURABLE_SEPARATOR)}")

            // What is actually ON DISK, not what the runtime is holding.
            "$packageName.action.e2e-durable" ->
                Log.i(TAG, "durable=${liveCards().substringAfter(DURABLE_SEPARATOR, "")}")

            else -> return false
        }
        return true
    }

    /** Creates one durable card, returning its id (0 = failure). */
    private external fun createForE2e(
        name: String,
        totalBytes: Long,
    ): Long

    /**
     * The restored card ids, then `;durable=` and each card's latest committed
     * state read off disk. [handle] splits it so the restore probe's own output
     * stays exactly what BN4 and F1b already assert.
     */
    private external fun liveCards(): String

    const val TAG = "EnvoixE2e"
    private const val DURABLE_SEPARATOR = ";durable="
}
