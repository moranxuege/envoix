package dev.envoix.app

/** A rendezvous room code (`<digits>-<word>-<word>`). The numeric [id] is the
 *  only part the broker and logs see; the remainder is the SPAKE2 password.
 *  Replaces the `substringBefore('-')` convention scattered through the app. */
@JvmInline
value class Room(
    val code: String,
) {
    val id: String get() = code.substringBefore('-')
}
