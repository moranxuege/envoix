package dev.envoix.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.documentfile.provider.DocumentFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
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

internal data class ManifestV2StageResult(
    val root: File,
    val issues: List<ManifestV2ProviderIssue>,
)

/** Android provider adapter. Every content/Photos/Share source is stabilized
 * into a job-owned private root before it enters the canonical Rust job. The
 * source UUID parent lets two same-named roots remain distinct without changing
 * either requested root name. */
internal object ManifestV2SourceStager {
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
                        issues +=
                            ManifestV2ProviderIssue(
                                emptyList(),
                                ManifestV2ProviderIssueKind.Unavailable,
                            )
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
        snapshot: ManifestV2JobSnapshot,
        expectedRootItemId: Long? = null,
    ): PreparedManifestV2Source {
        check(snapshot.selections.isNotEmpty()) { "Native job did not retain the prepared source" }
        val selection =
            expectedRootItemId?.let { expected ->
                snapshot.selections.firstOrNull { it.rootItemId == expected }
            } ?: snapshot.selections.last()
        check(expectedRootItemId == null || selection.rootItemId == expectedRootItemId) {
            "Native job did not retain the reauthorized source"
        }
        return PreparedManifestV2Source(
            source = source.copy(displayName = selection.requestedName),
            localRoot = localRoot,
            rootItemId = selection.rootItemId,
            issueCount = selection.issues.size,
            canApprovePartial = canApprovePartial(selection.issues),
            partialApproved = selection.partialApproved,
        )
    }

    fun stagedProviderRoot(
        source: ManifestV2Source,
        staged: ManifestV2StageResult,
        origin: ManifestV2SourceOrigin = ManifestV2SourceOrigin.ContentUri,
    ): ManifestV2StagedProviderRoot =
        ManifestV2StagedProviderRoot(
            path = staged.root.absolutePath,
            requestedName = source.displayName,
            origin = origin,
            issues = staged.issues,
        )

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
                issues +=
                    ManifestV2ProviderIssue(
                        relative,
                        ManifestV2ProviderIssueKind.PermissionDenied,
                    )
                return
            } catch (_: Throwable) {
                issues +=
                    ManifestV2ProviderIssue(
                        relative,
                        ManifestV2ProviderIssueKind.Unavailable,
                    )
                return
            }
        for (child in children) {
            val name = child.name?.takeIf(::validComponent)
            if (name == null) {
                issues +=
                    ManifestV2ProviderIssue(
                        relative,
                        ManifestV2ProviderIssueKind.InvalidName,
                    )
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
                    else ->
                        issues +=
                            ManifestV2ProviderIssue(
                                childPath,
                                ManifestV2ProviderIssueKind.SpecialFile,
                            )
                }
            } catch (_: SecurityException) {
                target.deleteRecursively()
                issues +=
                    ManifestV2ProviderIssue(
                        childPath,
                        ManifestV2ProviderIssueKind.PermissionDenied,
                    )
            } catch (_: Throwable) {
                target.deleteRecursively()
                issues +=
                    ManifestV2ProviderIssue(
                        childPath,
                        ManifestV2ProviderIssueKind.Unavailable,
                    )
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

    private fun canApprovePartial(issues: List<ManifestV2JobIssue>): Boolean =
        issues.none { issue ->
            issue.relativeComponents.isEmpty() || issue.kind == ManifestV2JobIssueKind.EntryLimit
        }
}
