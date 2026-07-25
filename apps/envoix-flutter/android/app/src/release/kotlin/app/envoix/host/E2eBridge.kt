package app.envoix.host

import android.content.Intent

/**
 * Release twin of the debug instrumentation bridge: a release-shaped build has
 * no e2e surface at all — no handler, no JNI binding, nothing exported.
 */
object E2eBridge {
    fun handle(
        intent: Intent,
        packageName: String,
    ): Boolean = false
}
