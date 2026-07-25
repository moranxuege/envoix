package app.envoix.host

import android.content.Intent

/**
 * The debug source set's instrumentation seam: it forwards to the bridge that
 * exists only here. Shaping the CALL SITE by source set is what lets the
 * release variant have no bridge at all, the same way the Rust lane's
 * `E2eBridge` exports exist only under a non-default cargo feature.
 */
internal fun handleInstrumentation(
    intent: Intent,
    packageName: String,
): Boolean = E2eBridge.handle(intent, packageName)
