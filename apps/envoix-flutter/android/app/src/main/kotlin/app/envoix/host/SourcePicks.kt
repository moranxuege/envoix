package app.envoix.host

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
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
     * told. Null when the provider will not describe it.
     *
     * REPLACES an unclaimed pick under the same key. A second completion for an
     * acquisition that is still selectable is a person correcting their choice,
     * and it is the only way they can: the authority refuses `RePickSource`
     * while the card is `Preparing`, because that state already has this ask
     * outstanding. Put-if-absent silently answered with the FIRST file — the
     * user chose again and was shown what they were replacing.
     *
     * Once the acquisition is BOUND the pick is settled: the same document is
     * an idempotent repeat, and a different one has to wait for a new
     * acquisition, because the duty has already been answered with the first.
     */
    @Synchronized
    fun offer(
        context: Context,
        acquisition: SourceAcquisitionKeyView,
        uri: Uri,
    ): Granted? {
        val key = keyOf(acquisition)
        bound[key]?.let { settled ->
            return if (settled == uri) describe(context, settled) else null
        }
        offered[key] = uri
        return describe(context, uri)
    }

    /**
     * Drops an unclaimed pick — an exchange the authority did not accept, or a
     * card that has gone away with a selection still in hand.
     */
    @Synchronized
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
     *
     * Removal arrives naming a CARD, because that is what the authority durably
     * removes — but a card owns one entry per acquisition it ever completed, so
     * this releases EVERY entry the card owns. Looking up the card as a whole
     * key found nothing once the journal became acquisition-keyed, and a
     * removed card's grant would have been retained until an unrelated
     * [recover] happened to notice it.
     */
    @Synchronized
    fun release(
        context: Context,
        card: String,
    ) {
        val journal = journal(context)
        val prefix = "$card-"
        val owned: Map<String, String> =
            journal
                .all
                .asSequence()
                .filter { (key, value) -> key.startsWith(prefix) && value is String }
                .associate { (key, value) -> key to value as String }
        if (owned.isEmpty()) {
            bound.keys.removeAll { it.startsWith(prefix) }
            return
        }
        val survivors: Set<String> =
            journal
                .all
                .asSequence()
                .filter { (key, _) -> !owned.containsKey(key) }
                .mapNotNull { (_, value) -> value as? String }
                .toSet()
        bound.keys.removeAll { it.startsWith(prefix) }
        // An unclaimed selection goes too. The durable card is gone, so nothing
        // will ever claim it, and leaving it here holds the URI and its
        // transient grant for the life of the process.
        offered.keys.removeAll { it.startsWith(prefix) }
        val edit = journal.edit()
        owned.keys.forEach(edit::remove)
        if (!edit.commit()) {
            // The entries are still ownership on disk, so nothing may be
            // released against them. Releasing anyway would leave a stale owner
            // that later makes a genuinely orphaned grant look owned — to this
            // function AND to `recover`, which trusts every journal value — and
            // the grant would then be retained forever. The removal is re-issued
            // idempotently by the authority's outbox, so failing closed here is
            // a retry rather than a loss.
            return
        }
        // A URI another acquisition still owns is not this card's to release.
        // Two cards CAN name one document: nothing stops a person choosing the
        // same file twice.
        owned.values.toSet().filterNot(survivors::contains).forEach { uri ->
            releaseGrant(context, Uri.parse(uri))
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

    /**
     * Whether Android really retained a read permission for this document.
     *
     * The claim above ASKS for a persistable grant; whether the provider gave
     * one is its own answer, and this reads the answer rather than the request.
     * A source reported `persisted` that a restart cannot reopen would send a
     * card into a resume it can never complete.
     */
    fun isPersisted(
        context: Context,
        uri: Uri,
    ): Boolean =
        context.contentResolver.persistedUriPermissions.any {
            it.isReadPermission && it.uri == uri
        }

    /**
     * Opens the bound source for reading. Null when it cannot be opened at all,
     * which is a different answer to any property of an open document.
     *
     * ONE open, deliberately. It answers seekability AND becomes the descriptor
     * Rust reads through, so what was probed and what is read are the same open
     * document. Probing with one open and handing down another would leave a
     * window in which a provider could answer two — and the seekability the
     * authority stored would then describe a document nobody reads.
     *
     * The caller owns what comes back and must close it.
     */
    fun open(
        context: Context,
        uri: Uri,
    ): ParcelFileDescriptor? = runCatching { context.contentResolver.openFileDescriptor(uri, "r") }.getOrNull()

    /**
     * Whether THIS open descriptor can be re-read from an offset.
     *
     * Asked of the OS about the exact open file description, not inferred from
     * `statSize`. A size is a stat answer — a provider can report one for a
     * stream, and report `-1` for a file it simply will not measure — whereas
     * a seek asks the only question that matters: does this description have a
     * position at all? A pipe answers `ESPIPE`, and a source wrongly reported
     * seekable would make every resume silently restart from zero.
     *
     * `SEEK_CUR` with a zero offset, so the probe is also the identity: it
     * cannot disturb the position of a descriptor that does have one.
     */
    fun probeSeekable(descriptor: ParcelFileDescriptor): Boolean =
        runCatching {
            Os.lseek(descriptor.fileDescriptor, 0, OsConstants.SEEK_CUR)
        }.isSuccess

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

    /** Null when the provider will not say what the document is. */
    private fun describe(
        context: Context,
        uri: Uri,
    ): Granted? {
        val projection =
            arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return context.contentResolver
            .query(uri, projection, null, null, null)
            ?.use { cursor ->
                // A provider that answers no row has not described the
                // document. An empty name and no size is what an EXISTING file
                // with neither looks like, and the capability contract has
                // `metadata_unavailable` precisely so the two stay apart.
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
            }
    }

    private const val OWNERSHIP_JOURNAL = "envoix-source-ownership"
}
