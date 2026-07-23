package dev.envoix.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.documentfile.provider.DocumentFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.util.UUID

data class ManifestV2Source(
    val uri: Uri,
    val directory: Boolean,
    val displayName: String,
)

data class PreparedManifestV2Source(
    val source: ManifestV2Source,
    val localRoot: File,
    val rootItemId: Long,
    val issueCount: Int,
    val canApprovePartial: Boolean,
    val partialApproved: Boolean,
)

data class ManifestV2ProviderIssue(
    val relativeComponents: List<String>,
    val kind: String,
)

data class ManifestV2StageResult(
    val root: File,
    val issues: List<ManifestV2ProviderIssue>,
)

data class RestoredManifestV2Preparation(
    val jobId: String,
    val summary: JSONObject,
    val sources: List<PreparedManifestV2Source>,
)

/** Android provider adapter. Every content/Photos/Share source is stabilized
 * into a job-owned private root before it enters the canonical Rust job. The
 * source UUID parent lets two same-named roots remain distinct without changing
 * either requested root name. */
object ManifestV2SourceStager {
    suspend fun stage(
        context: Context,
        jobId: String,
        source: ManifestV2Source,
    ): ManifestV2StageResult =
        withContext(Dispatchers.IO) {
            require(validComponent(source.displayName)) { "Source has an unsafe display name" }
            takeReadPermission(context, source.uri)
            val sourceDirectory =
                File(File(context.filesDir, "manifest-v2/source-staging/$jobId"), UUID.randomUUID().toString())
            check(sourceDirectory.mkdirs()) { "Could not create source staging" }
            val root = File(sourceDirectory, source.displayName)
            val issues = mutableListOf<ManifestV2ProviderIssue>()
            try {
                if (source.directory) {
                    check(root.mkdir()) { "Could not create directory staging" }
                    val document = DocumentFile.fromTreeUri(context, source.uri)
                    if (document == null) {
                        issues += ManifestV2ProviderIssue(emptyList(), "unavailable")
                    } else {
                        copyDirectory(context, document, root, emptyList(), issues)
                    }
                } else {
                    copyFile(context, source.uri, root)
                }
                ManifestV2StageResult(root, issues)
            } catch (error: Throwable) {
                sourceDirectory.deleteRecursively()
                throw error
            }
        }

    fun sourceFromUri(
        context: Context,
        uri: Uri,
        directory: Boolean,
    ): ManifestV2Source {
        val rawName =
            if (directory) {
                DocumentFile.fromTreeUri(context, uri)?.name
            } else {
                context.contentResolver
                    .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                    ?.use { cursor ->
                        if (cursor.moveToFirst()) cursor.getString(0) else null
                    }
            }
        val name = rawName?.takeIf(::validComponent) ?: if (directory) "Folder" else "File"
        return ManifestV2Source(uri, directory, name)
    }

    fun parsePreparedSnapshot(
        source: ManifestV2Source,
        localRoot: File,
        response: String,
        expectedRootItemId: Long? = null,
    ): PreparedManifestV2Source {
        val json = JSONObject(response)
        json.optString("error").takeIf(String::isNotEmpty)?.let(::error)
        val selections = json.getJSONArray("selections")
        check(selections.length() > 0) { "Native job did not retain the prepared source" }
        val selection =
            expectedRootItemId?.let { expected ->
                (0 until selections.length())
                    .map(selections::getJSONObject)
                    .firstOrNull { it.getLong("root_item_id") == expected }
            } ?: selections.getJSONObject(selections.length() - 1)
        check(expectedRootItemId == null || selection.getLong("root_item_id") == expectedRootItemId) {
            "Native job did not retain the reauthorized source"
        }
        val issues = selection.getJSONArray("issues")
        return PreparedManifestV2Source(
            source = source.copy(displayName = selection.getString("name")),
            localRoot = localRoot,
            rootItemId = selection.getLong("root_item_id"),
            issueCount = issues.length(),
            canApprovePartial = canApprovePartial(issues),
            partialApproved = selection.optBoolean("partial_approved"),
        )
    }

    /** Returns the most recently updated unsent job. The Rust job store owns
     * lifecycle/selection truth; this adapter only reconstructs the bounded
     * native projection from job-owned local roots. */
    fun restoreLatestPreparation(response: String): RestoredManifestV2Preparation? {
        val envelope = JSONObject(response)
        envelope.optString("error").takeIf(String::isNotEmpty)?.let(::error)
        val jobs = envelope.getJSONArray("jobs")
        val job =
            (0 until jobs.length())
                .map(jobs::getJSONObject)
                .maxByOrNull { it.optLong("updated_unix_ms") }
                ?: return null
        val selections = job.getJSONArray("selections")
        val sources =
            (0 until selections.length()).mapNotNull { index ->
                val selection = selections.getJSONObject(index)
                val localPath =
                    selection.optString("local_path").takeIf(String::isNotBlank)
                        ?: return@mapNotNull null
                val root = File(localPath)
                if (!root.exists()) return@mapNotNull null
                val source =
                    ManifestV2Source(
                        uri = Uri.fromFile(root),
                        directory = selection.optBoolean("directory"),
                        displayName = selection.getString("name"),
                    )
                PreparedManifestV2Source(
                    source = source,
                    localRoot = root,
                    rootItemId = selection.getLong("root_item_id"),
                    issueCount = selection.getJSONArray("issues").length(),
                    canApprovePartial = canApprovePartial(selection.getJSONArray("issues")),
                    partialApproved = selection.optBoolean("partial_approved"),
                )
            }
        if (sources.isEmpty()) return null
        return RestoredManifestV2Preparation(
            jobId = job.getString("job_id"),
            summary = job,
            sources = sources,
        )
    }

    fun rootsJson(
        source: ManifestV2Source,
        staged: ManifestV2StageResult,
        origin: String = "content_uri",
    ): String =
        JSONArray()
            .put(
                JSONObject()
                    .put("path", staged.root.absolutePath)
                    .put("requested_name", source.displayName)
                    .put("origin", origin)
                    .put(
                        "issues",
                        JSONArray().apply {
                            staged.issues.forEach { issue ->
                                put(
                                    JSONObject()
                                        .put("relative_components", JSONArray(issue.relativeComponents))
                                        .put("kind", issue.kind),
                                )
                            }
                        },
                    ),
            ).toString()

    private fun copyDirectory(
        context: Context,
        source: DocumentFile,
        destination: File,
        relative: List<String>,
        issues: MutableList<ManifestV2ProviderIssue>,
    ) {
        val children =
            try {
                source.listFiles()
            } catch (_: SecurityException) {
                issues += ManifestV2ProviderIssue(relative, "permission_denied")
                return
            } catch (_: Throwable) {
                issues += ManifestV2ProviderIssue(relative, "unavailable")
                return
            }
        for (child in children) {
            val name = child.name?.takeIf(::validComponent)
            if (name == null) {
                issues += ManifestV2ProviderIssue(relative, "invalid_name")
                continue
            }
            val target = File(destination, name)
            val childPath = relative + name
            try {
                when {
                    child.isDirectory -> {
                        if (!target.mkdir()) error("Could not create local directory")
                        copyDirectory(context, child, target, childPath, issues)
                    }
                    child.isFile -> copyFile(context, child.uri, target)
                    else -> issues += ManifestV2ProviderIssue(childPath, "special_file")
                }
            } catch (_: SecurityException) {
                target.deleteRecursively()
                issues += ManifestV2ProviderIssue(childPath, "permission_denied")
            } catch (_: Throwable) {
                target.deleteRecursively()
                issues += ManifestV2ProviderIssue(childPath, "unavailable")
            }
        }
    }

    private fun copyFile(
        context: Context,
        source: Uri,
        destination: File,
    ) {
        destination.parentFile?.mkdirs()
        val input =
            context.contentResolver.openInputStream(source)
                ?: error("Selected item cannot be opened")
        input.use { from ->
            FileOutputStream(destination).use { to ->
                from.copyTo(to)
                to.fd.sync()
            }
        }
    }

    private fun takeReadPermission(
        context: Context,
        uri: Uri,
    ) {
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
    }

    private fun validComponent(value: String): Boolean =
        value.isNotBlank() &&
            value != "." &&
            value != ".." &&
            '/' !in value &&
            '\\' !in value &&
            '\u0000' !in value

    private fun canApprovePartial(issues: JSONArray): Boolean =
        (0 until issues.length()).none { index ->
            val issue = issues.getJSONObject(index)
            issue.getJSONArray("path").length() == 0 || issue.optString("kind") == "entry_limit"
        }
}
