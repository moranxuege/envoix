package app.envoix.host

import android.content.Intent

/**
 * A release-shaped build has no e2e surface: no bridge class, no JNI binding,
 * nothing exported. The release gate asserts that on the packaged dex.
 */
internal fun handleInstrumentation(
    intent: Intent,
    packageName: String,
): Boolean = false
