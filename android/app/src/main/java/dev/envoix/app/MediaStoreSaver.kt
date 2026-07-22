package dev.envoix.app

import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteConstraintException
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import androidx.documentfile.provider.DocumentFile
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.net.URLConnection
import java.nio.charset.StandardCharsets
import java.security.MessageDigest

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
    /** Content evidence for the bytes copied into a public destination. */
    data class PublicationEvidence(
        val size: Long,
        val sha256: String,
    ) {
        fun matches(other: PublicationEvidence): Boolean {
            if (size != other.size || !isSha256(sha256) || !isSha256(other.sha256)) return false
            return MessageDigest.isEqual(
                sha256.toByteArray(StandardCharsets.US_ASCII),
                other.sha256.toByteArray(StandardCharsets.US_ASCII),
            )
        }
    }

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

    /** An existing public artifact proven byte-for-byte identical to staging. */
    data class ExistingPublication(
        val uri: Uri,
        val displayName: String,
        val evidence: PublicationEvidence,
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
    ): Result<PublicationEvidence> =
        runCatching {
            val output =
                context.contentResolver.openOutputStream(target.uri)
                    ?: throw java.io.IOException("publish target is not writable")
            output.use { out ->
                source.inputStream().use { input -> copyAndHash(input, out) }
            }
        }

    /** Hash the current public bytes. Used only by crash recovery and manual
     *  delivery-proof reconciliation, both exceptional paths. */
    fun inspect(
        context: Context,
        uri: Uri,
    ): Result<PublicationEvidence> =
        runCatching {
            val input =
                context.contentResolver.openInputStream(uri)
                    ?: throw java.io.FileNotFoundException("published file is not readable")
            input.use { hash(it) }
        }

    /**
     * Find the exact requested public name and reuse it only after a full
     * content comparison. A missing, unreadable, or changed artifact is never
     * accepted on filename/size alone.
     */
    fun findIdentical(
        context: Context,
        source: File,
        displayName: String,
        treeUri: String,
        folder: String,
    ): Result<ExistingPublication?> =
        runCatching {
            val candidates =
                treeUri
                    .takeIf { it.isNotBlank() }
                    ?.let { identicalTreeCandidates(context, Uri.parse(it), displayName) }
                    ?: identicalDownloadCandidates(context, folder, displayName)
            var sourceEvidence: PublicationEvidence? = null
            for (candidate in candidates) {
                if (candidate.size >= 0 && candidate.size != source.length()) continue
                val publicEvidence = inspect(context, candidate.uri).getOrNull() ?: continue
                if (publicEvidence.size != source.length()) continue
                val expected = sourceEvidence ?: source.inputStream().use(::hash).also { sourceEvidence = it }
                if (expected.matches(publicEvidence)) {
                    return@runCatching ExistingPublication(candidate.uri, candidate.displayName, expected)
                }
            }
            null
        }

    internal fun hash(input: InputStream): PublicationEvidence = copyAndHash(input, null)

    /** One pass over the verified staging file both copies and records evidence. */
    internal fun copyAndHash(
        input: InputStream,
        output: OutputStream?,
    ): PublicationEvidence {
        val digest = MessageDigest.getInstance(SHA256_ALGORITHM)
        val buffer = ByteArray(COPY_BUFFER_BYTES)
        var size = 0L
        while (true) {
            val read = input.read(buffer)
            if (read < 0) break
            if (read == 0) continue
            output?.write(buffer, 0, read)
            digest.update(buffer, 0, read)
            size += read
        }
        return PublicationEvidence(size, digest.digest().joinToString("") { "%02x".format(it) })
    }

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
                unpended.isSuccess && unpended.getOrThrow() == 1 -> {
                    val actual =
                        queryDisplayName(context, target.uri)
                            ?: return Result.failure(IllegalStateException("published row has no display name"))
                    if (actual == candidate) return Result.success(PublishOutcome(target.uri, candidate))

                    // Some MediaStore providers silently append " (1)" after
                    // the extension during insert/un-pend. Rename the row back
                    // to our already extension-safe candidate and verify what
                    // the provider actually committed.
                    val corrected =
                        runCatching {
                            resolver.update(
                                target.uri,
                                ContentValues().apply { put(MediaStore.Downloads.DISPLAY_NAME, candidate) },
                                null,
                                null,
                            )
                        }
                    when {
                        corrected.isFailure && isUniqueViolation(corrected.exceptionOrNull()) -> continue
                        corrected.isFailure -> return Result.failure(corrected.exceptionOrNull()!!)
                        corrected.getOrThrow() != 1 ->
                            return Result.failure(rowVanished("correct name", corrected.getOrThrow()))
                        queryDisplayName(context, target.uri) == candidate ->
                            return Result.success(PublishOutcome(target.uri, candidate))
                        else -> return Result.failure(IllegalStateException("provider changed published display name"))
                    }
                }
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
        val candidate = uniqueName(tree, displayName)
        val doc = tree.createFile(mimeTypeFor(candidate), candidate) ?: return null
        if (doc.name != candidate && (!doc.renameTo(candidate) || doc.name != candidate)) {
            doc.delete()
            return null
        }
        return Reserved(doc.uri, mediaStorePending = false, displayName = candidate)
    }

    private fun reserveInDownloads(
        context: Context,
        displayName: String,
        folder: String,
    ): Reserved? {
        val relativePath = downloadRelativePath(folder)
        val existing = runCatching { downloadNames(context, relativePath) }.getOrDefault(emptySet())
        val candidate = availableName(displayName, existing)
        // Pick an extension-safe name before insert. Several MediaStore
        // providers silently resolve a collision as "photo.jpg (1)" instead of
        // throwing the UNIQUE error that [commit] is prepared to handle.
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, candidate)
                put(MediaStore.Downloads.MIME_TYPE, mimeTypeFor(candidate))
                put(MediaStore.Downloads.RELATIVE_PATH, relativePath)
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri =
            context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending)
                ?: return null
        val actual = queryDisplayName(context, uri)
        if (actual != candidate) {
            delete(context, uri)
            return null
        }
        return Reserved(uri, mediaStorePending = true, displayName = candidate)
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

    /** First extension-safe candidate absent from [existing]. */
    internal fun availableName(
        name: String,
        existing: Set<String>,
    ): String = nameSequence(name).first { it !in existing }

    internal fun mimeTypeFor(name: String): String = URLConnection.guessContentTypeFromName(name) ?: DEFAULT_MIME_TYPE

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

    private data class PublicCandidate(
        val uri: Uri,
        val displayName: String,
        /** -1 when a provider does not expose a trustworthy size. */
        val size: Long,
    )

    private fun identicalTreeCandidates(
        context: Context,
        treeUri: Uri,
        displayName: String,
    ): List<PublicCandidate> {
        val tree = DocumentFile.fromTreeUri(context, treeUri) ?: return emptyList()
        val document = tree.findFile(displayName) ?: return emptyList()
        return listOf(PublicCandidate(document.uri, document.name ?: displayName, document.length()))
    }

    private fun identicalDownloadCandidates(
        context: Context,
        folder: String,
        displayName: String,
    ): List<PublicCandidate> {
        val collection = MediaStore.Downloads.EXTERNAL_CONTENT_URI
        val projection =
            arrayOf(
                MediaStore.Downloads._ID,
                MediaStore.Downloads.DISPLAY_NAME,
                MediaStore.Downloads.SIZE,
            )
        val result = mutableListOf<PublicCandidate>()
        val cursor =
            context.contentResolver.query(
                collection,
                projection,
                "${MediaStore.Downloads.RELATIVE_PATH} = ? AND ${MediaStore.Downloads.DISPLAY_NAME} = ?",
                arrayOf(downloadRelativePath(folder), displayName),
                null,
            )
        cursor?.use {
            val idColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads._ID)
            val nameColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads.DISPLAY_NAME)
            val sizeColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads.SIZE)
            while (cursor.moveToNext()) {
                result +=
                    PublicCandidate(
                        ContentUris.withAppendedId(collection, cursor.getLong(idColumn)),
                        cursor.getString(nameColumn),
                        if (cursor.isNull(sizeColumn)) -1L else cursor.getLong(sizeColumn),
                    )
            }
        }
        return result
    }

    private fun downloadNames(
        context: Context,
        relativePath: String,
    ): Set<String> {
        val names = mutableSetOf<String>()
        val cursor =
            context.contentResolver.query(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                arrayOf(MediaStore.Downloads.DISPLAY_NAME),
                "${MediaStore.Downloads.RELATIVE_PATH} = ?",
                arrayOf(relativePath),
                null,
            )
        cursor?.use {
            val column = cursor.getColumnIndexOrThrow(MediaStore.Downloads.DISPLAY_NAME)
            while (cursor.moveToNext()) names += cursor.getString(column)
        }
        return names
    }

    private fun queryDisplayName(
        context: Context,
        uri: Uri,
    ): String? =
        runCatching {
            context.contentResolver
                .query(uri, arrayOf(MediaStore.Downloads.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (!cursor.moveToFirst()) return@use null
                    cursor.getString(cursor.getColumnIndexOrThrow(MediaStore.Downloads.DISPLAY_NAME))
                }
        }.getOrNull()

    private fun downloadRelativePath(folder: String): String {
        val sub = folder.trim().ifBlank { "Envoix" }
        return "${Environment.DIRECTORY_DOWNLOADS}/$sub/"
    }

    private const val NAME_ATTEMPTS = 200
    private const val ALNUM = "abcdefghijklmnopqrstuvwxyz0123456789"
    private const val SHA256_ALGORITHM = "SHA-256"
    private const val SHA256_HEX_LENGTH = 64
    private const val COPY_BUFFER_BYTES = 64 * 1024
    private const val DEFAULT_MIME_TYPE = "application/octet-stream"

    private fun isSha256(value: String): Boolean = value.length == SHA256_HEX_LENGTH && value.all { it in '0'..'9' || it in 'a'..'f' }
}
