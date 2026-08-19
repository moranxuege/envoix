package dev.envoix.app

import android.content.Context
import android.net.Uri
import android.system.Os
import android.system.OsConstants
import androidx.documentfile.provider.DocumentFile
import dev.envoix.app.ffi.FfiDestinationCommitReplyV2
import dev.envoix.app.ffi.FfiDestinationCommitRequestV2
import dev.envoix.app.ffi.FfiDestinationPlanReplyV2
import dev.envoix.app.ffi.FfiDestinationPlanRequestV2
import dev.envoix.app.ffi.FfiDestinationPlannedRootV2
import dev.envoix.app.ffi.FfiDestinationSavedRootV2
import dev.envoix.app.ffi.FfiManifestEntryKindV2
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.URLConnection
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.Locale

/**
 * Owns Android's public destination plan and the post-verification save.
 *
 * Public root names are frozen and journaled before native sends Accept. Save
 * therefore uses exact names only: a late collision fails the transfer instead
 * of silently producing a second keep-both name.
 */
class ManifestV2DestinationWriter(
    private val context: Context,
) {
    internal fun plan(request: FfiDestinationPlanRequestV2): FfiDestinationPlanReplyV2 {
        val response = JSONObject(plan(request.toJson().toString()))
        val roots = response.getJSONArray("roots")
        return FfiDestinationPlanReplyV2(
            roots =
                (0 until roots.length()).map { index ->
                    val root = roots.getJSONObject(index)
                    FfiDestinationPlannedRootV2(
                        rootId = root.getLong("root_id").checkedUInt("root ID"),
                        plannedName = root.getString("planned_name"),
                    )
                },
        )
    }

    fun plan(requestJson: String): String {
        val request = JSONObject(requestJson)
        val jobId = request.getString("job_id")
        val generation = request.getLong("generation")
        val requestedRoots = request.getJSONArray("roots")
        val journal = journalFile(jobId, generation)
        journal.parentFile?.mkdirs()
        val settings = SettingsStore.settings.value
        val tree = destinationTree(settings.saveTreeUri)
        val destinationId = destinationId(settings.saveTreeUri, settings.saveFolder)

        loadJournal(journal)?.let { existing ->
            validateJournalIdentity(existing, jobId, generation, destinationId, requestedRoots)
            return planReply(existing.getJSONArray("roots")).toString()
        }

        val batchNames =
            request
                .optJSONArray("reserved_names")
                ?.let { names ->
                    (0 until names.length()).mapTo(mutableSetOf()) { nameKey(names.getString(it)) }
                } ?: mutableSetOf()
        val roots = JSONArray()
        for (index in 0 until requestedRoots.length()) {
            val root = requestedRoots.getJSONObject(index)
            val requestedName = root.getString("requested_name")
            val isFile = root.getString("kind") == KIND_FILE
            check(tree != null || isFile) {
                "Choose a save folder before receiving one or more directories"
            }
            val plannedName =
                if (tree != null) {
                    allocateTreeName(tree, requestedName, isFile, batchNames)
                } else {
                    MediaStoreSaver.planDownloadName(
                        context,
                        requestedName,
                        settings.saveFolder,
                        batchNames,
                    )
                }
            batchNames += nameKey(plannedName)
            roots.put(
                JSONObject()
                    .put("root_id", root.getInt("root_id"))
                    .put("requested_name", requestedName)
                    .put("planned_name", plannedName)
                    .put("kind", root.getString("kind"))
                    .put("state", STATE_PLANNED),
            )
        }
        val value = newJournal(jobId, generation, destinationId, STATE_PLANNED, roots)
        writeJournal(journal, value)
        return planReply(roots).toString()
    }

    fun save(requestJson: String): String = saveWithDestination(requestJson).responseJson

    /**
     * Commits the public roots and returns presentation metadata derived from
     * the same immutable settings snapshot that selected the destination.
     */
    internal fun saveWithDestination(requestJson: String): ManifestV2SaveResult {
        val request = JSONObject(requestJson)
        val jobId = request.getString("job_id")
        val generation = request.getLong("generation")
        val requestedRoots = request.getJSONArray("roots")
        val journal = journalFile(jobId, generation)
        val value = loadJournal(journal) ?: error("Public destination plan is missing")
        val settings = SettingsStore.settings.value
        validateJournalIdentity(
            value,
            jobId,
            generation,
            destinationId(settings.saveTreeUri, settings.saveFolder),
            requestedRoots,
        )
        val roots = value.getJSONArray("roots")
        val tree = destinationTree(settings.saveTreeUri)

        recoverInterruptedSave(value, roots, requestedRoots, tree, journal)
        for (index in 0 until roots.length()) {
            val planned = roots.getJSONObject(index)
            if (planned.getString("state") == STATE_COMMITTED) continue
            val requested = requestedRoot(requestedRoots, planned.getInt("root_id"))
            val source = File(requested.getString("local_path"))
            check(source.exists()) { "Verified staging root disappeared before save" }
            check(requested.getString("planned_name") == planned.getString("planned_name")) {
                "Native save request changed the frozen public name"
            }
            val target =
                createExactTarget(
                    source,
                    planned.getString("planned_name"),
                    tree,
                    settings.saveFolder,
                    value,
                    planned,
                    journal,
                )
            try {
                if (source.isDirectory) {
                    copyDirectory(source, target.document!!)
                } else {
                    MediaStoreSaver.copyInto(context, source, target.reserved).getOrThrow()
                }
                val outcome =
                    if (target.document != null) {
                        Outcome(target.document.name ?: target.plannedName, target.document.uri)
                    } else {
                        val published = MediaStoreSaver.commit(context, target.reserved).getOrThrow()
                        check(published.displayName == target.plannedName) {
                            "MediaStore changed the frozen public name"
                        }
                        Outcome(published.displayName, published.uri)
                    }
                planned
                    .put("state", STATE_COMMITTED)
                    .put("final_name", outcome.name)
                    .put("uri", outcome.uri.toString())
                    .remove("media_store_pending")
                value.put("state", if (allCommitted(roots)) STATE_COMMITTED else STATE_SAVING)
                writeJournal(journal, value)
            } catch (error: Throwable) {
                target.delete(context)
                planned.put("state", STATE_PLANNED)
                planned.remove("uri")
                planned.remove("media_store_pending")
                value.put("state", STATE_SAVING)
                writeJournal(journal, value)
                throw IllegalStateException(
                    "Could not save root ${planned.getInt("root_id")} using its accepted name",
                    error,
                )
            }
        }
        value.put("state", STATE_COMMITTED)
        writeJournal(journal, value)
        return ManifestV2SaveResult(
            responseJson = committedReply(roots).toString(),
            destinationLabel = destinationLabel(settings, tree),
        )
    }

    internal fun saveWithDestination(request: FfiDestinationCommitRequestV2): ManifestV2TypedSaveResult {
        val result = saveWithDestination(request.toJson().toString())
        val response = JSONObject(result.responseJson).getJSONArray("roots")
        return ManifestV2TypedSaveResult(
            reply =
                FfiDestinationCommitReplyV2(
                    roots =
                        (0 until response.length()).map { index ->
                            val root = response.getJSONObject(index)
                            FfiDestinationSavedRootV2(
                                rootId = root.getLong("root_id").checkedUInt("root ID"),
                                finalName = root.getString("final_name"),
                                uri = root.getString("uri"),
                            )
                        },
                ),
            destinationLabel = result.destinationLabel,
        )
    }

    private data class Outcome(
        val name: String,
        val uri: Uri,
    )

    private data class ExactTarget(
        val plannedName: String,
        val reserved: MediaStoreSaver.Reserved,
        val document: DocumentFile?,
    ) {
        fun delete(context: Context) {
            if (document != null) document.delete() else MediaStoreSaver.delete(context, reserved.uri)
        }
    }

    private fun createExactTarget(
        source: File,
        plannedName: String,
        tree: DocumentFile?,
        folder: String,
        journalValue: JSONObject,
        rootValue: JSONObject,
        journal: File,
    ): ExactTarget {
        rootValue
            .put("state", STATE_CREATING)
            .remove("uri")
        journalValue.put("state", STATE_SAVING)
        writeJournal(journal, journalValue)

        if (tree != null) {
            check(tree.findFile(plannedName) == null) {
                "The destination namespace changed after this transfer was accepted"
            }
            val document =
                if (source.isDirectory) {
                    tree.createDirectory(plannedName)
                } else {
                    tree.createFile(mimeType(plannedName), plannedName)
                } ?: error("Could not create the accepted destination name")
            check(document.name == plannedName) {
                document.delete()
                "The destination provider changed the accepted root name"
            }
            rootValue
                .put("state", STATE_COPYING)
                .put("uri", document.uri.toString())
                .put("media_store_pending", false)
            writeJournal(journal, journalValue)
            return ExactTarget(
                plannedName,
                MediaStoreSaver.Reserved(document.uri, false, plannedName),
                document,
            )
        }

        val reserved =
            MediaStoreSaver.reserveDownloadExact(context, plannedName, folder)
                ?: error("Could not reserve the accepted Downloads name")
        rootValue
            .put("state", STATE_COPYING)
            .put("uri", reserved.uri.toString())
            .put("media_store_pending", true)
        writeJournal(journal, journalValue)
        return ExactTarget(plannedName, reserved, null)
    }

    private fun recoverInterruptedSave(
        journalValue: JSONObject,
        plannedRoots: JSONArray,
        requestedRoots: JSONArray,
        tree: DocumentFile?,
        journal: File,
    ) {
        for (index in 0 until plannedRoots.length()) {
            val planned = plannedRoots.getJSONObject(index)
            val requested = requestedRoot(requestedRoots, planned.getInt("root_id"))
            val source = File(requested.getString("local_path"))
            when (planned.getString("state")) {
                STATE_COMMITTED -> {
                    check(savedRootMatches(source, Uri.parse(planned.getString("uri")))) {
                        "A previously committed Android destination changed"
                    }
                }
                STATE_COPYING -> {
                    val uri = Uri.parse(planned.getString("uri"))
                    if (savedRootMatches(source, uri)) {
                        val pending = planned.optBoolean("media_store_pending")
                        if (pending) {
                            val outcome =
                                MediaStoreSaver
                                    .commit(
                                        context,
                                        MediaStoreSaver.Reserved(
                                            uri,
                                            true,
                                            planned.getString("planned_name"),
                                        ),
                                    ).getOrThrow()
                            check(outcome.displayName == planned.getString("planned_name"))
                        }
                        planned
                            .put("state", STATE_COMMITTED)
                            .put("final_name", planned.getString("planned_name"))
                            .remove("media_store_pending")
                    } else {
                        check(!destinationExists(uri) || MediaStoreSaver.delete(context, uri)) {
                            "Could not remove an incomplete Android destination"
                        }
                        planned.put("state", STATE_PLANNED)
                        planned.remove("uri")
                        planned.remove("media_store_pending")
                    }
                }
                STATE_CREATING -> {
                    val exact = tree?.findFile(planned.getString("planned_name"))
                    check(exact == null) {
                        "The final save outcome is unknown; remove the incomplete item and resume"
                    }
                    planned.put("state", STATE_PLANNED)
                }
            }
        }
        journalValue.put("state", if (allCommitted(plannedRoots)) STATE_COMMITTED else STATE_SAVING)
        writeJournal(journal, journalValue)
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
            check(target.name == child.name) {
                target.delete()
                "The destination provider changed the internal name ${child.name}"
            }
            if (child.isDirectory) copyDirectory(child, target) else copyFile(child, target)
        }
    }

    private fun copyFile(
        source: File,
        destination: DocumentFile,
    ) {
        val output =
            context.contentResolver.openOutputStream(destination.uri, "wt")
                ?: error("Destination cannot be opened")
        source.inputStream().use { input -> output.use { input.copyTo(it) } }
    }

    private fun allocateTreeName(
        parent: DocumentFile,
        requested: String,
        preserveExtension: Boolean,
        batchNames: Set<String>,
    ): String {
        for (suffix in 0 until MAX_NAME_ATTEMPTS) {
            val candidate =
                if (suffix == 0) {
                    requested
                } else {
                    manifestV2KeepBothName(requested, suffix, preserveExtension)
                }
            if (nameKey(candidate) !in batchNames && parent.findFile(candidate) == null) return candidate
        }
        error("Could not allocate a non-conflicting destination name")
    }

    private fun savedRootMatches(
        source: File,
        uri: Uri,
    ): Boolean =
        runCatching {
            if (source.isDirectory) {
                val destination = DocumentFile.fromSingleUri(context, uri) ?: return@runCatching false
                directoryMatches(source, destination)
            } else {
                source.isFile &&
                    source.inputStream().use(MediaStoreSaver::hash).matches(
                        MediaStoreSaver.inspect(context, uri).getOrThrow(),
                    )
            }
        }.getOrDefault(false)

    private fun directoryMatches(
        source: File,
        destination: DocumentFile,
    ): Boolean {
        if (!destination.isDirectory) return false
        val sourceChildren = source.listFiles() ?: return false
        val destinationChildren = destination.listFiles()
        if (sourceChildren.size != destinationChildren.size) return false
        val destinationByName = destinationChildren.groupBy { it.name }
        return sourceChildren.all { child ->
            val target = destinationByName[child.name]?.singleOrNull() ?: return@all false
            if (child.isDirectory) {
                directoryMatches(child, target)
            } else {
                child.isFile &&
                    target.isFile &&
                    child.inputStream().use(MediaStoreSaver::hash).matches(
                        MediaStoreSaver.inspect(context, target.uri).getOrThrow(),
                    )
            }
        }
    }

    private fun validateJournalIdentity(
        journal: JSONObject,
        jobId: String,
        generation: Long,
        destinationId: String,
        requestedRoots: JSONArray,
    ) {
        check(journal.getInt("schema_version") == JOURNAL_SCHEMA_VERSION)
        check(journal.getString("job_id") == jobId && journal.getLong("generation") == generation)
        check(journal.getString("destination_id") == destinationId) {
            "The selected Android destination changed after this transfer was accepted"
        }
        val roots = journal.getJSONArray("roots")
        check(roots.length() == requestedRoots.length())
        for (index in 0 until roots.length()) {
            val planned = roots.getJSONObject(index)
            val requested = requestedRoot(requestedRoots, planned.getInt("root_id"))
            requested.optString("requested_name").takeIf(String::isNotEmpty)?.let {
                check(it == planned.getString("requested_name"))
            }
            requested.optString("planned_name").takeIf(String::isNotEmpty)?.let {
                check(it == planned.getString("planned_name"))
            }
        }
    }

    private fun requestedRoot(
        roots: JSONArray,
        rootId: Int,
    ): JSONObject =
        (0 until roots.length())
            .map { roots.getJSONObject(it) }
            .singleOrNull { it.getInt("root_id") == rootId }
            ?: error("Android destination request omitted root $rootId")

    private fun destinationTree(encoded: String): DocumentFile? =
        encoded.takeIf(String::isNotBlank)?.let {
            DocumentFile
                .fromTreeUri(context, Uri.parse(it))
                ?.takeIf(DocumentFile::canWrite)
                ?: error("The selected save folder is unavailable")
        }

    private fun journalFile(
        jobId: String,
        generation: Long,
    ): File = File(context.filesDir, "manifest-v2/destination-save/$jobId-$generation.json")

    private fun loadJournal(file: File): JSONObject? {
        if (!file.isFile) return null
        return JSONObject(file.readText())
    }

    private fun writeJournal(
        journal: File,
        value: JSONObject,
    ) {
        val parent = requireNotNull(journal.parentFile)
        parent.mkdirs()
        val temporary = File(parent, "${journal.name}.tmp")
        FileOutputStream(temporary).use { output ->
            output.write(value.toString().toByteArray(Charsets.UTF_8))
            output.fd.sync()
        }
        try {
            Files.move(
                temporary.toPath(),
                journal.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
                StandardCopyOption.ATOMIC_MOVE,
            )
        } catch (_: Throwable) {
            Files.move(
                temporary.toPath(),
                journal.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
        val descriptor = Os.open(parent.absolutePath, OsConstants.O_RDONLY, 0)
        try {
            Os.fsync(descriptor)
        } finally {
            Os.close(descriptor)
        }
    }

    private fun newJournal(
        jobId: String,
        generation: Long,
        destinationId: String,
        state: String,
        roots: JSONArray,
    ): JSONObject =
        JSONObject()
            .put("schema_version", JOURNAL_SCHEMA_VERSION)
            .put("job_id", jobId)
            .put("generation", generation)
            .put("destination_id", destinationId)
            .put("state", state)
            .put("roots", roots)

    private fun planReply(roots: JSONArray): JSONObject =
        JSONObject().put(
            "roots",
            JSONArray().apply {
                for (index in 0 until roots.length()) {
                    val root = roots.getJSONObject(index)
                    put(
                        JSONObject()
                            .put("root_id", root.getInt("root_id"))
                            .put("planned_name", root.getString("planned_name")),
                    )
                }
            },
        )

    private fun committedReply(roots: JSONArray): JSONObject =
        JSONObject().put(
            "roots",
            JSONArray().apply {
                for (index in 0 until roots.length()) {
                    val root = roots.getJSONObject(index)
                    check(root.getString("state") == STATE_COMMITTED)
                    put(
                        JSONObject()
                            .put("root_id", root.getInt("root_id"))
                            .put("final_name", root.getString("final_name"))
                            .put("uri", root.getString("uri")),
                    )
                }
            },
        )

    private fun allCommitted(roots: JSONArray): Boolean =
        (0 until roots.length()).all { roots.getJSONObject(it).getString("state") == STATE_COMMITTED }

    private fun destinationExists(uri: Uri): Boolean =
        if (DocumentFile.isDocumentUri(context, uri)) {
            DocumentFile.fromSingleUri(context, uri)?.exists() == true
        } else {
            MediaStoreSaver.resolves(context, uri)
        }

    private fun mimeType(name: String): String = URLConnection.guessContentTypeFromName(name) ?: "application/octet-stream"

    private fun nameKey(name: String): String = name.lowercase(Locale.ROOT)

    private fun destinationId(
        treeUri: String,
        folder: String,
    ): String = if (treeUri.isNotBlank()) "tree:$treeUri" else "downloads:$folder"

    private fun destinationLabel(
        settings: Settings,
        tree: DocumentFile?,
    ): String =
        tree
            ?.name
            ?.trim()
            ?.takeIf(String::isNotEmpty)
            ?: "Downloads / ${settings.saveFolder}"

    private fun FfiDestinationPlanRequestV2.toJson(): JSONObject =
        JSONObject()
            .put("job_id", jobId)
            .put("generation", generation.toLong())
            .put("reserved_names", JSONArray(reservedNames))
            .put(
                "roots",
                JSONArray().apply {
                    roots.forEach { root ->
                        put(
                            JSONObject()
                                .put("root_id", root.rootId.toLong())
                                .put("requested_name", root.requestedName)
                                .put("kind", root.kind.wireName()),
                        )
                    }
                },
            )

    private fun FfiDestinationCommitRequestV2.toJson(): JSONObject =
        JSONObject()
            .put("job_id", jobId)
            .put("generation", generation.toLong())
            .put(
                "roots",
                JSONArray().apply {
                    roots.forEach { root ->
                        put(
                            JSONObject()
                                .put("root_id", root.rootId.toLong())
                                .put("local_path", root.localPath)
                                .put("planned_name", root.plannedName)
                                .put("kind", root.kind.wireName()),
                        )
                    }
                },
            )

    private fun FfiManifestEntryKindV2.wireName(): String =
        when (this) {
            FfiManifestEntryKindV2.FILE -> KIND_FILE
            FfiManifestEntryKindV2.DIRECTORY -> "directory"
        }

    private companion object {
        const val JOURNAL_SCHEMA_VERSION = 2
        const val MAX_NAME_ATTEMPTS = 10_000
        const val KIND_FILE = "file"
        const val STATE_PLANNED = "planned"
        const val STATE_CREATING = "creating"
        const val STATE_COPYING = "copying"
        const val STATE_SAVING = "saving"
        const val STATE_COMMITTED = "committed"
    }
}

internal data class ManifestV2SaveResult(
    val responseJson: String,
    val destinationLabel: String,
)

internal data class ManifestV2TypedSaveResult(
    val reply: FfiDestinationCommitReplyV2,
    val destinationLabel: String,
)

private fun Long.checkedUInt(name: String): UInt {
    require(this in 0..UInt.MAX_VALUE.toLong()) { "$name exceeded the Android range" }
    return toUInt()
}

internal fun manifestV2KeepBothName(
    name: String,
    suffix: Int,
    preserveExtension: Boolean,
): String {
    val dot = if (preserveExtension) name.lastIndexOf('.').takeIf { it > 0 } else null
    return if (dot == null) {
        "$name ($suffix)"
    } else {
        "${name.substring(0, dot)} ($suffix)${name.substring(dot)}"
    }
}
