package dev.envoix.app

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import java.io.File

/**
 * Publishes a received file into the public Downloads/Envoix folder via
 * MediaStore, so it is reachable from any file manager (no storage permission
 * needed on Android 10+). Returns the content Uri, or null on failure.
 */
object MediaStoreSaver {
    fun saveToDownloads(context: Context, source: File, displayName: String, folder: String): Uri? {
        val resolver = context.contentResolver
        val sub = folder.trim().ifBlank { "Envoix" }
        val pending = ContentValues().apply {
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
            resolver.update(uri, ContentValues().apply {
                put(MediaStore.Downloads.IS_PENDING, 0)
            }, null, null)
            uri
        }.getOrElse {
            resolver.delete(uri, null, null)
            null
        }
    }
}
