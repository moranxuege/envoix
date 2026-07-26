package app.envoix.host

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicReference

/**
 * The document the user picked, held by the PLATFORM between the picker and the
 * card that will own it.
 *
 * This is where the `content://` URI lives, and the only place it lives. The
 * frontend is told the provider's display name and size — sanitized metadata,
 * never an artifact key (`SF09`) — and nothing else, so no Dart value can name
 * a file, open a stream, or be forged into one (`XP01`).
 *
 * It is deliberately NOT durable. A persistable read grant is taken so the OS
 * keeps the permission, but this build holds no durable pointer to the picked
 * document, so a process death really does lose the pick — and the card, whose
 * source was created non-recoverable, then fails honestly with "re-pick the
 * source" (`RS04`) instead of promising a resume it cannot deliver. Durable
 * source retention arrives with the F3 staging slice.
 */
object SourcePicks {
    /** What the platform will tell the frontend about a pick. */
    data class Granted(
        val displayName: String,
        val sizeBytes: Long,
    )

    /**
     * The pick no card has claimed yet. One slot, because there is one picker
     * and one Activity: picking again replaces it, which is what the user just
     * asked for.
     */
    private val offered = AtomicReference<Uri?>(null)

    /** Card id (16 hex digits) to the source bound to it. */
    private val bound = ConcurrentHashMap<String, Uri>()

    /** Records a pick and reads back what the frontend may be told about it. */
    fun offer(
        context: Context,
        uri: Uri,
    ): Granted {
        offered.set(uri)
        return describe(context, uri)
    }

    /**
     * The source this card owns, binding the outstanding pick to it the first
     * time. Idempotent per card: a duty re-delivered for a card that already
     * holds a source resolves to the same one instead of eating the next pick.
     */
    fun claim(card: String): Uri? {
        bound[card]?.let { return it }
        val picked = offered.getAndSet(null) ?: return null
        return bound.putIfAbsent(card, picked) ?: picked
    }

    /** Whether the bound source is still readable through its grant. */
    fun readable(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching { context.contentResolver.openInputStream(uri)?.use { true } }
            .getOrNull() == true

    private fun describe(
        context: Context,
        uri: Uri,
    ): Granted {
        val projection =
            arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return context.contentResolver
            .query(uri, projection, null, null, null)
            ?.use { cursor ->
                if (!cursor.moveToFirst()) {
                    return@use null
                }
                val name = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val size = cursor.getColumnIndex(OpenableColumns.SIZE)
                Granted(
                    displayName = if (name >= 0) cursor.getString(name).orEmpty() else "",
                    sizeBytes = if (size >= 0 && !cursor.isNull(size)) cursor.getLong(size) else 0L,
                )
            } ?: Granted(displayName = "", sizeBytes = 0L)
    }
}
