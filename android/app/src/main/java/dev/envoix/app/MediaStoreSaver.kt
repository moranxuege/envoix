package dev.envoix.app

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import androidx.documentfile.provider.DocumentFile
import java.io.File

/**
 * Publishes a received file. By default it goes into the public Downloads/<folder>
 * via MediaStore (no storage permission on Android 10+). If the user picked a
 * folder via SAF, it goes there instead. Returns the content Uri, or null.
 */
object MediaStoreSaver {
    /** SAF folder ([treeUri]) if set, else Downloads/[folder] via MediaStore. */
    fun saveReceived(
        context: Context,
        source: File,
        displayName: String,
        treeUri: String,
        folder: String,
    ): Uri? =
        treeUri
            .takeIf { it.isNotBlank() }
            ?.let { saveToTree(context, source, displayName, Uri.parse(it)) }
            ?: saveToDownloads(context, source, displayName, folder)

    /** Save into a user-picked SAF tree via DocumentFile; null on failure. */
    private fun saveToTree(
        context: Context,
        source: File,
        displayName: String,
        treeUri: Uri,
    ): Uri? {
        val tree = DocumentFile.fromTreeUri(context, treeUri)?.takeIf { it.canWrite() } ?: return null
        // Never delete a same-named file (it may be the user's, or an earlier
        // receive): uniquify like the Downloads/MediaStore path does.
        val doc = tree.createFile("application/octet-stream", uniqueName(tree, displayName)) ?: return null
        return runCatching {
            context.contentResolver.openOutputStream(doc.uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
            doc.uri
        }.getOrElse {
            doc.delete()
            null
        }
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

    fun saveToDownloads(
        context: Context,
        source: File,
        displayName: String,
        folder: String,
    ): Uri? {
        val resolver = context.contentResolver
        val sub = folder.trim().ifBlank { "Envoix" }
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
                put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$sub")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending) ?: return null
        return runCatching {
            resolver.openOutputStream(uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
            resolver.update(
                uri,
                ContentValues().apply {
                    put(MediaStore.Downloads.IS_PENDING, 0)
                },
                null,
                null,
            )
            uri
        }.getOrElse {
            resolver.delete(uri, null, null)
            null
        }
    }
}
