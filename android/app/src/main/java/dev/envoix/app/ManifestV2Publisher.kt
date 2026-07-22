package dev.envoix.app

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.URLConnection
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** Saves verified private roots into the actual user destination. This object
 * is called by the native result gate, so returning is the receiver's durable
 * Saved boundary—not merely a background publish request. */
class ManifestV2Publisher(
    private val context: Context,
) {
    fun save(requestJson: String): String {
        val request = JSONObject(requestJson)
        val jobId = request.getString("job_id")
        val roots = request.getJSONArray("roots")
        val journal = File(context.filesDir, "manifest-v2/publication/$jobId.json")
        journal.parentFile?.mkdirs()
        val recovered = recoverJournal(journal)
        if (recovered.length() == roots.length()) {
            writeJournal(journal, "committed", recovered, null)
            return JSONObject().put("roots", recovered).toString()
        }

        val settings = SettingsStore.settings.value
        val tree =
            settings.saveTreeUri.takeIf(String::isNotBlank)?.let {
                DocumentFile.fromTreeUri(context, Uri.parse(it))
                    ?: error("The selected save folder is unavailable")
            }
        val outcomes = recovered
        val committedRootIds =
            (0 until outcomes.length())
                .map { outcomes.getJSONObject(it).getInt("root_id") }
                .toMutableSet()
        for (index in 0 until roots.length()) {
            val root = roots.getJSONObject(index)
            val source = File(root.getString("local_path"))
            check(source.exists()) { "Verified staging root disappeared before save" }
            val requestedName = root.getString("requested_name")
            val rootId = root.getInt("root_id")
            if (rootId in committedRootIds) continue
            val outcome =
                if (tree != null) {
                    saveToTree(source, requestedName, tree, journal, outcomes, rootId)
                } else {
                    check(source.isFile) {
                        "Choose a save folder before receiving one or more directories"
                    }
                    saveFileToDownloads(source, requestedName, settings.saveFolder, journal, outcomes, rootId)
                }
            outcomes.put(
                JSONObject()
                    .put("root_id", rootId)
                    .put("final_name", outcome.name)
                    .put("uri", outcome.uri.toString()),
            )
            committedRootIds += rootId
            writeJournal(journal, "committing", outcomes, null)
        }
        writeJournal(journal, "committed", outcomes, null)
        return JSONObject().put("roots", outcomes).toString()
    }

    private data class Outcome(
        val name: String,
        val uri: Uri,
    )

    private fun saveFileToDownloads(
        source: File,
        requestedName: String,
        folder: String,
        journal: File,
        committedRoots: JSONArray,
        rootId: Int,
    ): Outcome {
        val reserved =
            MediaStoreSaver.reserve(context, requestedName, "", folder)
                ?: error("Could not reserve a Downloads destination")
        writeJournal(journal, "copying", committedRoots, reserved.uri.toString())
        try {
            MediaStoreSaver.copyInto(context, source, reserved).getOrThrow()
            val committed = MediaStoreSaver.commit(context, reserved).getOrThrow()
            return Outcome(committed.displayName, committed.uri)
        } catch (error: Throwable) {
            MediaStoreSaver.delete(context, reserved.uri)
            throw IllegalStateException("Could not save root $rootId to Downloads", error)
        }
    }

    private fun saveToTree(
        source: File,
        requestedName: String,
        tree: DocumentFile,
        journal: File,
        committedRoots: JSONArray,
        rootId: Int,
    ): Outcome {
        val finalName = allocateName(tree, requestedName)
        val target =
            if (source.isDirectory) {
                tree.createDirectory(finalName)
            } else {
                tree.createFile(mimeType(finalName), finalName)
            } ?: error("Could not reserve destination for $finalName")
        writeJournal(journal, "copying", committedRoots, target.uri.toString())
        try {
            if (source.isDirectory) {
                copyDirectory(source, target)
            } else {
                copyFile(source, target)
            }
            return Outcome(target.name ?: finalName, target.uri)
        } catch (error: Throwable) {
            target.delete()
            throw IllegalStateException("Could not save root $rootId to the selected folder", error)
        }
    }

    private fun copyDirectory(
        source: File,
        destination: DocumentFile,
    ) {
        val children = source.listFiles() ?: error("Could not enumerate verified directory")
        for (child in children) {
            val target =
                if (child.isDirectory) {
                    destination.createDirectory(child.name)
                } else {
                    destination.createFile(mimeType(child.name), child.name)
                } ?: error("Could not create ${child.name}")
            if (child.isDirectory) copyDirectory(child, target) else copyFile(child, target)
        }
    }

    private fun copyFile(
        source: File,
        destination: DocumentFile,
    ) {
        val output = context.contentResolver.openOutputStream(destination.uri, "wt")
            ?: error("Destination cannot be opened")
        source.inputStream().use { input ->
            output.use { input.copyTo(it) }
        }
    }

    private fun allocateName(
        parent: DocumentFile,
        requested: String,
    ): String {
        for (suffix in 0 until MAX_NAME_ATTEMPTS) {
            val candidate = if (suffix == 0) requested else keepBothName(requested, suffix)
            if (parent.findFile(candidate) == null) return candidate
        }
        error("Could not allocate a non-conflicting destination name")
    }

    private fun keepBothName(
        name: String,
        suffix: Int,
    ): String {
        val dot = name.lastIndexOf('.').takeIf { it > 0 }
        return if (dot == null) {
            "$name ($suffix)"
        } else {
            "${name.substring(0, dot)} ($suffix)${name.substring(dot)}"
        }
    }

    private fun mimeType(name: String): String =
        URLConnection.guessContentTypeFromName(name) ?: "application/octet-stream"

    private fun recoverJournal(journal: File): JSONArray {
        val value = runCatching { JSONObject(journal.readText()) }.getOrNull() ?: return JSONArray()
        value.optString("pending_uri").takeIf(String::isNotEmpty)?.let {
            MediaStoreSaver.delete(context, Uri.parse(it))
        }
        val roots = value.optJSONArray("roots") ?: return JSONArray()
        val retained = JSONArray()
        for (index in 0 until roots.length()) {
            val root = roots.getJSONObject(index)
            val uri = Uri.parse(root.getString("uri"))
            if (MediaStoreSaver.resolves(context, uri)) retained.put(root)
        }
        writeJournal(journal, "committing", retained, null)
        return retained
    }

    private fun writeJournal(
        journal: File,
        state: String,
        roots: JSONArray,
        pendingUri: String?,
    ) {
        val value = JSONObject().put("state", state).put("roots", roots)
        pendingUri?.let { value.put("pending_uri", it) }
        val temporary = File(journal.parentFile, "${journal.name}.tmp")
        temporary.writeText(value.toString())
        runCatching {
            Files.move(
                temporary.toPath(),
                journal.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
                StandardCopyOption.ATOMIC_MOVE,
            )
        }.getOrElse {
            Files.move(
                temporary.toPath(),
                journal.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    private companion object {
        const val MAX_NAME_ATTEMPTS = 10_000
    }
}
