package dev.envoix.app

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import androidx.documentfile.provider.DocumentFile
import java.io.File
import java.io.InputStream

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
        if (treeUri.isNotBlank()) {
            saveToTree(context, source, displayName, Uri.parse(treeUri))
        } else {
            saveToDownloads(context, source, displayName, folder)
        }

    /** Save into a user-picked SAF tree via DocumentFile; null on failure. */
    private fun saveToTree(
        context: Context,
        source: File,
        displayName: String,
        treeUri: Uri,
    ): Uri? {
        val tree = DocumentFile.fromTreeUri(context, treeUri)?.takeIf { it.canWrite() } ?: return null
        tree.findFile(displayName)?.let { existing ->
            return existing.uri.takeIf { existing.length() == source.length() && contentEquals(context, source, it) }
        }
        val doc = tree.createFile("application/octet-stream", displayName) ?: return null
        return runCatching {
            context.contentResolver.openOutputStream(doc.uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
            check(doc.length() == source.length()) { "published SAF file size mismatch" }
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
        val relativePath = "${Environment.DIRECTORY_DOWNLOADS}/$sub"
        val existing = findExistingDownload(context, source, displayName, relativePath)
        existing.uri?.let { return it }
        if (existing.conflict) return null
        val pending =
            ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
                put(MediaStore.Downloads.RELATIVE_PATH, relativePath)
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, pending) ?: return null
        return runCatching {
            resolver.openOutputStream(uri)!!.use { out ->
                source.inputStream().use { it.copyTo(out) }
            }
            resolver.openAssetFileDescriptor(uri, "r")?.use { descriptor ->
                check(descriptor.length < 0 || descriptor.length == source.length()) {
                    "published MediaStore file size mismatch"
                }
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

    private fun findExistingDownload(
        context: Context,
        source: File,
        displayName: String,
        relativePath: String,
    ): ExistingDownload {
        val resolver = context.contentResolver
        val projection =
            arrayOf(
                MediaStore.Downloads._ID,
                MediaStore.Downloads.SIZE,
                MediaStore.Downloads.IS_PENDING,
            )
        val selection =
            "${MediaStore.Downloads.DISPLAY_NAME} = ? AND " +
                "${MediaStore.Downloads.RELATIVE_PATH} IN (?, ?)"
        val args = arrayOf(displayName, relativePath, "$relativePath/")
        resolver
            .query(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                projection,
                selection,
                args,
                null,
            )?.use { cursor ->
                val idColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads._ID)
                val sizeColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads.SIZE)
                val pendingColumn = cursor.getColumnIndexOrThrow(MediaStore.Downloads.IS_PENDING)
                while (cursor.moveToNext()) {
                    val uri =
                        Uri.withAppendedPath(
                            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                            cursor.getLong(idColumn).toString(),
                        )
                    val size = cursor.getLong(sizeColumn)
                    val isPending = cursor.getInt(pendingColumn) != 0
                    if (size == source.length() && contentEquals(context, source, uri)) {
                        if (isPending) {
                            resolver.update(
                                uri,
                                ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                                null,
                                null,
                            )
                        }
                        return ExistingDownload(uri = uri)
                    }
                    if (isPending) resolver.delete(uri, null, null) else return ExistingDownload(conflict = true)
                }
            }
        return ExistingDownload()
    }

    private fun contentEquals(
        context: Context,
        source: File,
        uri: Uri,
    ): Boolean =
        runCatching {
            source.inputStream().use { left ->
                context.contentResolver.openInputStream(uri)?.use { right -> streamsEqual(left, right) } ?: false
            }
        }.getOrDefault(false)

    private fun streamsEqual(
        left: InputStream,
        right: InputStream,
    ): Boolean {
        val leftBuffer = ByteArray(1024 * 1024)
        val rightBuffer = ByteArray(leftBuffer.size)
        while (true) {
            val leftCount = left.read(leftBuffer)
            val rightCount = right.read(rightBuffer)
            if (leftCount != rightCount) return false
            if (leftCount < 0) return true
            for (index in 0 until leftCount) {
                if (leftBuffer[index] != rightBuffer[index]) return false
            }
        }
    }

    private data class ExistingDownload(
        val uri: Uri? = null,
        val conflict: Boolean = false,
    )
}
