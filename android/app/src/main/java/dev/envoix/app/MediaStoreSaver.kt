package dev.envoix.app

import android.content.ContentValues
import android.content.Context
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

    /** Make a reserved target visible after a successful copy (MediaStore only).
     *  Returns the failure cause instead of throwing: a colliding `_data` (a
     *  same-named file already published) surfaces as UNIQUE(files._data) here,
     *  and must not crash the app mid-publish. */
    fun commit(
        context: Context,
        target: Reserved,
    ): Result<Unit> =
        runCatching {
            if (target.mediaStorePending) {
                context.contentResolver.update(
                    target.uri,
                    ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                    null,
                    null,
                )
            }
        }.map { }

    /** Delete a target by URI (recovery: drop a half-written candidate). */
    fun delete(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching {
            DocumentFile.fromSingleUri(context, uri)?.delete()
                ?: (context.contentResolver.delete(uri, null, null) > 0)
        }.getOrDefault(false)

    /** Does [uri] still resolve to an openable document? (User may have deleted it.) */
    fun resolves(
        context: Context,
        uri: Uri,
    ): Boolean =
        runCatching {
            context.contentResolver.openFileDescriptor(uri, "r")?.use {}
            true
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
        // receive): uniquify like the Downloads/MediaStore path does.
        val doc = tree.createFile("application/octet-stream", uniqueName(tree, displayName)) ?: return null
        return Reserved(doc.uri, mediaStorePending = false)
    }

    private fun reserveInDownloads(
        context: Context,
        displayName: String,
        folder: String,
    ): Reserved? {
        val sub = folder.trim().ifBlank { "Envoix" }
        // MediaStore stores RELATIVE_PATH with a trailing slash; match it so the
        // collision query lines up with already-committed rows.
        val relPath = "${Environment.DIRECTORY_DOWNLOADS}/$sub/"
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, uniqueDownloadName(context, displayName, relPath))
                put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
                put(MediaStore.Downloads.RELATIVE_PATH, relPath)
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri =
            context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending)
                ?: return null
        return Reserved(uri, mediaStorePending = true)
    }

    /** [name] if free under [relPath] in Downloads, else "name (1)", "name (2)",
     *  …. A pending MediaStore insert is NOT auto-uniquified when un-pended, so
     *  committing a colliding name throws UNIQUE(files._data); uniquify up front,
     *  mirroring the SAF path. [relPath] is MediaStore's stored form (trailing slash). */
    private fun uniqueDownloadName(
        context: Context,
        name: String,
        relPath: String,
    ): String {
        fun taken(candidate: String): Boolean =
            context.contentResolver
                .query(
                    MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                    arrayOf(MediaStore.Downloads._ID),
                    "${MediaStore.Downloads.RELATIVE_PATH} = ? AND ${MediaStore.Downloads.DISPLAY_NAME} = ?",
                    arrayOf(relPath, candidate),
                    null,
                )?.use { it.count > 0 } ?: false
        if (!taken(name)) return name
        val dot = name.lastIndexOf('.')
        val (base, ext) = if (dot > 0) name.substring(0, dot) to name.substring(dot) else name to ""
        for (i in 1..99) {
            val candidate = "$base ($i)$ext"
            if (!taken(candidate)) return candidate
        }
        return "$base (${System.currentTimeMillis()})$ext"
    }

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
}
