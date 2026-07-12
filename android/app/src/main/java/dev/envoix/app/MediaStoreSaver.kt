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

    /** Copy [source] into a reserved target. */
    fun copyInto(
        context: Context,
        source: File,
        target: Reserved,
    ): Boolean =
        runCatching {
            context.contentResolver.openOutputStream(target.uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
        }.isSuccess

    /** Make a reserved target visible after a successful copy (MediaStore only). */
    fun commit(
        context: Context,
        target: Reserved,
    ) {
        if (target.mediaStorePending) {
            context.contentResolver.update(
                target.uri,
                ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                null,
                null,
            )
        }
    }

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
        return if (copyInto(context, source, target)) {
            commit(context, target)
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
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
                put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$sub")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri =
            context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending)
                ?: return null
        return Reserved(uri, mediaStorePending = true)
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
