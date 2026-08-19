package dev.envoix.app

/** Exceptional JNI bridge compiled into the typed core (libenvoix_ffi.so). */
object Native {
    init {
        System.loadLibrary("envoix_ffi")
    }

    /** Wire the Android VM + app context into the Rust network stack. Call once. */
    external fun initContext(context: android.content.Context)
}
