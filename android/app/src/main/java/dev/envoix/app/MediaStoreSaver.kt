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
        tree.findFile(displayName)?.delete() // overwrite a same-named file
        val doc = tree.createFile("application/octet-stream", displayName) ?: return null
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
