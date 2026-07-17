package dev.envoix.app

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteConstraintException
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import androidx.documentfile.provider.DocumentFile
import java.io.File

/**
 * Publishes a received file into public storage: the user's SAF folder if one is
 * picked, else Downloads/<folder> via MediaStore (no storage permission on
 * Android 10+).
 *
 * The publish is split into reserve → copy → commit so a journal can be written
 * BETWEEN the steps (see the publish journal): the reserved destination's URI is
 * recorded before any bytes are copied, so a crash mid-copy leaves a recoverable
 * trail (delete the half-written candidate) instead of a stranded, truncated,
 * user-visible file.
 */
object MediaStoreSaver {
    /** A reserved (empty) publish destination, awaiting the copy. */
    data class Reserved(
        val uri: Uri,
        /** MediaStore rows are inserted IS_PENDING and must be committed to
         *  become visible; SAF documents are visible on creation. */
        val mediaStorePending: Boolean,
        /** The name actually reserved — the SAF-uniquified name, or the requested
         *  Downloads name (which [commit] may still bump on a collision). */
        val displayName: String,
    )

    /** A successful publish: the URI, and the name it actually landed under (may
     *  differ from the requested name after a collision bump). */
    data class PublishOutcome(
        val uri: Uri,
        val displayName: String,
    )

    /**
     * Reserve an empty destination — a SAF document (uniquified) or a pending
     * MediaStore row — without copying any bytes. Returns null if it can't be
     * created.
     */
    fun reserve(
        context: Context,
        displayName: String,
        treeUri: String,
        folder: String,
    ): Reserved? =
        treeUri
            .takeIf { it.isNotBlank() }
            ?.let { reserveInTree(context, displayName, Uri.parse(it)) }
            ?: reserveInDownloads(context, displayName, folder)

    /** Copy [source] into a reserved target. Returns the failure cause (a
     *  typed result, not a bare Boolean) so `platform.publish.failed` can carry
     *  a real reason instead of a swallowed exception. */
    fun copyInto(
        context: Context,
        source: File,
        target: Reserved,
    ): Result<Unit> =
        runCatching {
            context.contentResolver.openOutputStream(target.uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
        }.map { }

    /**
     * Make a reserved target visible after a successful copy, resolving a name
     * collision by bumping the pending row's DISPLAY_NAME and retrying: the
     * un-pend is where MediaStore finalizes `_data`, so a colliding name throws
     * UNIQUE(files._data) here. Uniqueness is proven by a *successful* commit,
     * never assumed — a pre-query can't see pending/orphaned rows.
     *
     * Only a UNIQUE violation is retried. Any other error (IO, provider dead), or
     * an update that affected 0 rows (the row vanished), fails immediately so the
     * caller keeps staging instead of deleting bytes it never published. SAF
     * targets are already visible → returned as-is.
     *
     * Returns the URI and the name it actually landed under.
     */
    fun commit(
        context: Context,
        target: Reserved,
    ): Result<PublishOutcome> {
        if (!target.mediaStorePending) {
            return Result.success(PublishOutcome(target.uri, target.displayName))
        }
        val resolver = context.contentResolver
        for ((attempt, candidate) in nameSequence(target.displayName).take(NAME_ATTEMPTS).withIndex()) {
            // The reserved row already carries the first candidate; later ones
            // need a rename (still pending, no `_data` yet) before the un-pend.
            if (attempt > 0) {
                val renamed =
                    runCatching {
                        resolver.update(
                            target.uri,
                            ContentValues().apply { put(MediaStore.Downloads.DISPLAY_NAME, candidate) },
                            null,
                            null,
                        )
                    }
                when {
                    renamed.isFailure && isUniqueViolation(renamed.exceptionOrNull()) -> continue
                    renamed.isFailure -> return Result.failure(renamed.exceptionOrNull()!!)
                    renamed.getOrThrow() != 1 -> return Result.failure(rowVanished("rename", renamed.getOrThrow()))
                }
            }
            val unpended =
                runCatching {
                    resolver.update(
                        target.uri,
                        ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                        null,
                        null,
                    )
                }
            when {
                unpended.isSuccess && unpended.getOrThrow() == 1 ->
                    return Result.success(PublishOutcome(target.uri, candidate))
                unpended.isSuccess -> return Result.failure(rowVanished("un-pend", unpended.getOrThrow()))
                isUniqueViolation(unpended.exceptionOrNull()) -> continue
                else -> return Result.failure(unpended.exceptionOrNull()!!)
            }
        }
        return Result.failure(IllegalStateException("publish exhausted $NAME_ATTEMPTS candidate names"))
    }

    /** Delete a target by URI (recovery: drop a half-written candidate). A
     *  MediaStore `content://media/...` row is NOT a document URI, so
     *  `DocumentFile.delete()` (DocumentsContract) fails on it — and because
     *  `fromSingleUri` is non-null for any URI, an Elvis fallback would never
     *  run. Branch on the URI kind so pending MediaStore rows are actually
     *  removed via `ContentResolver.delete`. */
    fun delete(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching {
            if (DocumentFile.isDocumentUri(context, uri)) {
                DocumentFile.fromSingleUri(context, uri)?.delete() ?: false
            } else {
                context.contentResolver.delete(uri, null, null) > 0
            }
        }.getOrDefault(false)

    /** Does [uri] still resolve to an openable document? (User may have deleted it.) */
    fun resolves(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching {
            // openFileDescriptor is nullable; a null means the target isn't
            // openable, so return the real result — never an unconditional true
            // (recovery adopts on true and would then delete staging).
            context.contentResolver.openFileDescriptor(uri, "r")?.use { true } ?: false
        }.getOrDefault(false)

    /** Reserve + copy + commit in one shot — the non-journaled convenience path. */
    fun saveReceived(
        context: Context,
        source: File,
        displayName: String,
        treeUri: String,
        folder: String,
    ): Uri? {
        val target = reserve(context, displayName, treeUri, folder) ?: return null
        return if (copyInto(context, source, target).isSuccess && commit(context, target).isSuccess) {
            target.uri
        } else {
            delete(context, target.uri)
            null
        }
    }

    private fun reserveInTree(
        context: Context,
        displayName: String,
        treeUri: Uri,
    ): Reserved? {
        val tree = DocumentFile.fromTreeUri(context, treeUri)?.takeIf { it.canWrite() } ?: return null
        // Never delete a same-named file (it may be the user's, or an earlier
        // receive): uniquify like the Downloads path does. The created document's
        // real name is what the record must show.
        val doc = tree.createFile("application/octet-stream", uniqueName(tree, displayName)) ?: return null
        return Reserved(doc.uri, mediaStorePending = false, displayName = doc.name ?: displayName)
    }

    private fun reserveInDownloads(
        context: Context,
        displayName: String,
        folder: String,
    ): Reserved? {
        val sub = folder.trim().ifBlank { "Envoix" }
        // Insert the raw requested name; `commit` resolves any collision at the
        // un-pend (the ground-truth collision point) by bumping + retrying.
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
                put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$sub/")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri =
            context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending)
                ?: return null
        return Reserved(uri, mediaStorePending = true, displayName = displayName)
    }

    /** [name], then "name (1)"…"name (99)", then random-suffixed candidates — an
     *  endless sequence consumed one-per-collision by [commit] (bounded there by
     *  [NAME_ATTEMPTS]). `internal` so the deterministic prefix is unit-testable. */
    internal fun nameSequence(name: String): Sequence<String> =
        sequence {
            val dot = name.lastIndexOf('.')
            val base = if (dot > 0) name.substring(0, dot) else name
            val ext = if (dot > 0) name.substring(dot) else ""
            yield(name)
            for (i in 1..99) yield("$base ($i)$ext")
            while (true) yield("$base (${randomSuffix()})$ext")
        }

    /** True if [t]'s cause chain holds a SQLite UNIQUE-constraint violation. The
     *  provider/Binder can wrap it, so walk `.cause`; match the UNIQUE wording so
     *  a different constraint (NOT NULL, …) is never mistaken for a name clash. */
    internal fun isUniqueViolation(t: Throwable?): Boolean {
        var e = t
        while (e != null) {
            if (e is SQLiteConstraintException &&
                e.message?.contains("UNIQUE", ignoreCase = true) == true
            ) {
                return true
            }
            e = e.cause
        }
        return false
    }

    private fun randomSuffix(): String = (1..8).map { ALNUM[kotlin.random.Random.nextInt(ALNUM.length)] }.joinToString("")

    private fun rowVanished(
        step: String,
        rows: Int,
    ) = IllegalStateException("publish $step affected $rows rows (expected 1); target row vanished")

    /** [name] if free in [tree], else "name (1)", "name (2)", … before the extension. */
    private fun uniqueName(
        tree: DocumentFile,
        name: String,
    ): String {
        if (tree.findFile(name) == null) return name
        val dot = name.lastIndexOf('.')
        val (base, ext) = if (dot > 0) name.substring(0, dot) to name.substring(dot) else name to ""
        for (i in 1..99) {
            val candidate = "$base ($i)$ext"
            if (tree.findFile(candidate) == null) return candidate
        }
        return "$base (${System.currentTimeMillis()})$ext"
    }

    private const val NAME_ATTEMPTS = 200
    private const val ALNUM = "abcdefghijklmnopqrstuvwxyz0123456789"
}
