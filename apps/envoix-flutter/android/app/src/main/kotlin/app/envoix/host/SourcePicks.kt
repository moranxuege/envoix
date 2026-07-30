package app.envoix.host

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import com.envoix.bindings.capability.SourceAcquisitionKeyView
import java.util.concurrent.ConcurrentHashMap

/**
 * The document the user picked, held by the PLATFORM between the picker and the
 * card that will own it.
 *
 * This is where the `content://` URI lives, and the only place it lives. The
 * frontend is told the provider's display name and size — sanitized metadata,
 * never an artifact key (`SF09`) — and nothing else, so no Dart value can name
 * a file, open a stream, or be forged into one (`XP01`).
 *
 * A pick has no durable authority. Only a card whose initial record has
 * committed may claim it; that claim takes the persistable read grant and
 * journals ACQUISITION → URI before reporting the source duty complete. Durable
 * removal travels back from the Rust authority on a separate service lane and
 * deletes the journal ownership before releasing the OS grant. Thus retained
 * access without a live card is not a state this object can represent.
 *
 * Everything here is keyed by the WHOLE acquisition — card, generation and
 * request — never by the card alone. Card-keyed storage meant an acquire duty
 * for generation 2 was answered with the document bound in generation 1: a
 * later ask silently inherited an earlier one's file. That is the ownership
 * defect `SourceAcquisitionKey` exists to close, and it reopened here because
 * this registry was the one place that did not carry the key.
 */
object SourcePicks {
    /**
     * What the platform will tell the frontend about a pick.
     *
     * `sizeBytes` is NULLABLE. A provider may genuinely not know, and `0` is a
     * real empty file — collapsing the two is what the untyped map this
     * replaces did, and it made "unknown" and "empty" the same answer.
     */
    data class Granted(
        val displayName: String,
        val sizeBytes: Long?,
    )

    /** The whole acquisition, as one storage key. */
    private fun keyOf(acquisition: SourceAcquisitionKeyView): String =
        "${acquisition.card}-${"%08x".format(acquisition.generation)}-${acquisition.request}"

    /**
     * Picks that have been made but not yet claimed by their card's duty, one
     * per acquisition. Keyed rather than a single slot: two cards can be
     * awaiting a document at once, and a single slot let the second pick
     * silently overwrite the first.
     */
    private val offered = ConcurrentHashMap<String, Uri>()

    /** Acquisition key to the source bound to it. */
    private val bound = ConcurrentHashMap<String, Uri>()

    /**
     * Records a pick FOR one acquisition and reads what the frontend may be
     * told.
     *
     * Put-if-absent, not replace: a second pick under a key that already has
     * one is a repeat of an exchange already answered, and honouring it would
     * change which document the outstanding acquire duty will bind. A user who
     * genuinely wants a different file re-picks, which advances the generation
     * and mints a new key.
     */
    fun offer(
        context: Context,
        acquisition: SourceAcquisitionKeyView,
        uri: Uri,
    ): Granted {
        val key = keyOf(acquisition)
        val held = offered.putIfAbsent(key, uri) ?: uri
        return describe(context, held)
    }

    /** Drops an unclaimed pick, for an exchange the authority did not accept. */
    fun discard(acquisition: SourceAcquisitionKeyView) {
        offered.remove(keyOf(acquisition))
    }

    /**
     * The source this ACQUISITION owns, binding its outstanding pick the first
     * time. Idempotent per acquisition: a duty re-delivered under the same key
     * resolves to the same document instead of eating another acquisition's
     * pick.
     */
    @Synchronized
    fun claim(
        context: Context,
        acquisition: SourceAcquisitionKeyView,
    ): Uri? {
        val card = keyOf(acquisition)
        bound[card]?.let { return it }
        val journal = journal(context)
        journal.getString(card, null)?.let(Uri::parse)?.let { owned ->
            return bound.putIfAbsent(card, owned) ?: owned
        }
        val picked = offered.remove(card) ?: return null
        val source = bound.putIfAbsent(card, picked) ?: picked
        // Some providers grant only process-lifetime access. Such a source can
        // still satisfy this process's duty honestly, but it is not written as
        // durable ownership unless Android actually retained the permission.
        val retained =
            runCatching {
                context.contentResolver.takePersistableUriPermission(
                    source,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION,
                )
                true
            }.getOrDefault(false)
        if (retained && !journal.edit().putString(card, source.toString()).commit()) {
            // A capability without durable ownership is not retained. The grant
            // is gone, so this card holds nothing — returning the URI anyway
            // would hand back a source we just released, and whether reading it
            // still works would depend on an ephemeral grant nobody recorded.
            //
            // Answering "no source" is honest and is NOT yet a recovery. There
            // is no implemented path back: the pick was consumed by the
            // `getAndSet` above, the report reaches only the process-local duty
            // ledger, and the `RePickSource` command the card later offers
            // submits a core command WITHOUT opening the picker. The card is
            // therefore stuck until source acquisition gains a durable, typed
            // result path. Do not persist the duty ledger before that lands —
            // it would convert this process-local dead end into a durable one.
            releaseGrant(context, source)
            bound.remove(card)
            return null
        }
        return source
    }

    /**
     * Ends one durably removed card's ownership. Journal removal comes first:
     * a crash after it leaves an unowned persisted grant, which [recover]
     * releases; doing these operations in the opposite order could retain a
     * dead card forever after a crash.
     */
    @Synchronized
    fun release(
        context: Context,
        card: String,
    ) {
        val journal = journal(context)
        val owned = journal.getString(card, null)?.let(Uri::parse)
        val sharedByAnotherCard =
            owned != null &&
                journal.all.any { (owner, uri) ->
                    owner != card && uri == owned.toString()
                }
        bound.remove(card)
        journal.edit().remove(card).commit()
        if (owned != null && !sharedByAnotherCard) {
            releaseGrant(context, owned)
        }
    }

    /**
     * Closes the only claim crash window: Android retained the grant, but the
     * process died before CARD → URI committed. Every persisted read URI not
     * named by the ownership journal is therefore an orphan and is released.
     */
    @Synchronized
    fun recover(context: Context) {
        val owners =
            journal(context)
                .all.values
                .filterIsInstance<String>()
                .toSet()
        context.contentResolver.persistedUriPermissions
            .asSequence()
            .filter { it.isReadPermission && it.uri.toString() !in owners }
            .forEach { releaseGrant(context, it.uri) }
    }

    /** Whether the bound source is still readable through its grant. */
    fun readable(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching { context.contentResolver.openInputStream(uri)?.use { true } }
            .getOrNull() == true

    private fun journal(context: Context) =
        context.getSharedPreferences(
            OWNERSHIP_JOURNAL,
            Context.MODE_PRIVATE,
        )

    private fun releaseGrant(
        context: Context,
        uri: Uri,
    ) {
        runCatching {
            context.contentResolver.releasePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
    }

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
                    // Absent, not zero, when the provider did not say. The
                    // contract carries the difference and Rust treats a claimed
                    // size as advisory either way.
                    sizeBytes = if (size >= 0 && !cursor.isNull(size)) cursor.getLong(size) else null,
                )
            } ?: Granted(displayName = "", sizeBytes = null)
    }

    private const val OWNERSHIP_JOURNAL = "envoix-source-ownership"
}
