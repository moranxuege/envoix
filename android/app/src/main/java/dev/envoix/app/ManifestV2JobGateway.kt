package dev.envoix.app

import dev.envoix.app.ffi.FfiCompressionPolicyV2
import dev.envoix.app.ffi.FfiProviderSourceIssueKindV2
import dev.envoix.app.ffi.FfiProviderSourceIssueV2
import dev.envoix.app.ffi.FfiSourceDecisionV2
import dev.envoix.app.ffi.FfiSourceIssueKindV2
import dev.envoix.app.ffi.FfiSourceOriginV2
import dev.envoix.app.ffi.FfiStagedProviderRootV2
import dev.envoix.app.ffi.FfiTransferJobSnapshotV2
import dev.envoix.app.ffi.FfiTransferJobStateV2
import dev.envoix.app.ffi.FfiTransferJobV2
import dev.envoix.app.ffi.createTransferJobV2
import dev.envoix.app.ffi.envoixCoreInfo
import dev.envoix.app.ffi.restoreTransferJobV2

internal enum class ManifestV2JobState {
    Preparing,
    NeedsSourceDecision,
    ReadyToSend,
    Sealed,
    Canceled,
}

internal enum class ManifestV2SourceOrigin {
    Photos,
    Share,
    ContentUri,
    FileProvider,
}

internal enum class ManifestV2ProviderIssueKind {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SpecialFile,
}

internal enum class ManifestV2JobIssueKind {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SymbolicLink,
    SpecialFile,
    SourceChanged,
    DepthLimit,
    EntryLimit,
}

internal enum class ManifestV2SourceDecision {
    ApprovePartial,
    RemoveSelection,
    CancelJob,
}

internal data class ManifestV2ProviderIssue(
    val relativeComponents: List<String>,
    val kind: ManifestV2ProviderIssueKind,
)

internal data class ManifestV2StagedProviderRoot(
    val path: String,
    val requestedName: String,
    val origin: ManifestV2SourceOrigin,
    val issues: List<ManifestV2ProviderIssue>,
)

internal data class ManifestV2JobIssue(
    val relativeComponents: List<String>,
    val kind: ManifestV2JobIssueKind,
)

internal data class ManifestV2JobSelection(
    val rootItemId: Long,
    val requestedName: String,
    val partialApproved: Boolean,
    val issues: List<ManifestV2JobIssue>,
)

internal data class ManifestV2JobInventory(
    val rootCount: Int,
    val fileCount: Int,
    val directoryCount: Int,
    val totalBytes: Long,
    val warningCount: Int,
)

internal data class ManifestV2JobSnapshot(
    val jobId: String,
    val state: ManifestV2JobState,
    val inventory: ManifestV2JobInventory,
    val selections: List<ManifestV2JobSelection>,
)

internal interface ManifestV2JobNativeCore {
    suspend fun create(
        storeDirectory: String,
        compressionPolicy: FfiCompressionPolicyV2,
    ): ManifestV2NativeJob

    suspend fun restore(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2NativeJob
}

internal interface ManifestV2NativeJob : AutoCloseable {
    suspend fun snapshot(): FfiTransferJobSnapshotV2

    suspend fun addStagedProviderRoots(roots: List<FfiStagedProviderRootV2>): FfiTransferJobSnapshotV2

    suspend fun resolveSourceIssue(
        rootItemId: ULong,
        decision: FfiSourceDecisionV2,
    ): FfiTransferJobSnapshotV2

    suspend fun reauthorizeStagedProviderSource(
        rootItemId: ULong,
        sourcePath: String,
        issues: List<FfiProviderSourceIssueV2>,
    ): FfiTransferJobSnapshotV2

    suspend fun cancelJob(): FfiTransferJobSnapshotV2

    suspend fun sealForSend()
}

internal class ManifestV2JobGateway(
    private val native: ManifestV2JobNativeCore = UniFfiManifestV2JobNativeCore,
) {
    suspend fun create(
        storeDirectory: String,
        compressionPolicy: String,
    ): ManifestV2JobSnapshot {
        val job = native.create(storeDirectory, compressionPolicy.toFfiCompressionPolicy())
        return try {
            job.snapshot().project()
        } finally {
            job.close()
        }
    }

    suspend fun addStagedProviderRoot(
        storeDirectory: String,
        jobId: String,
        root: ManifestV2StagedProviderRoot,
    ): ManifestV2JobSnapshot =
        withRestoredJob(storeDirectory, jobId) { job ->
            job.addStagedProviderRoots(listOf(root.toFfi())).project()
        }

    suspend fun resolveSourceIssue(
        storeDirectory: String,
        jobId: String,
        rootItemId: Long,
        decision: ManifestV2SourceDecision,
    ): ManifestV2JobSnapshot =
        withRestoredJob(storeDirectory, jobId) { job ->
            job.resolveSourceIssue(rootItemId.toNativeId(), decision.toFfi()).project()
        }

    suspend fun reauthorizeStagedProviderSource(
        storeDirectory: String,
        jobId: String,
        rootItemId: Long,
        root: ManifestV2StagedProviderRoot,
    ): ManifestV2JobSnapshot =
        withRestoredJob(storeDirectory, jobId) { job ->
            job
                .reauthorizeStagedProviderSource(
                    rootItemId.toNativeId(),
                    root.path,
                    root.issues.map(ManifestV2ProviderIssue::toFfi),
                ).project()
        }

    suspend fun cancel(
        storeDirectory: String,
        jobId: String,
    ) {
        withRestoredJob(storeDirectory, jobId) { job ->
            job.cancelJob()
            Unit
        }
    }

    suspend fun seal(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2JobSnapshot =
        withRestoredJob(storeDirectory, jobId) { job ->
            job.sealForSend()
            job.snapshot().project()
        }

    private suspend fun <T> withRestoredJob(
        storeDirectory: String,
        jobId: String,
        operation: suspend (ManifestV2NativeJob) -> T,
    ): T {
        val job = native.restore(storeDirectory, jobId)
        return try {
            operation(job)
        } finally {
            job.close()
        }
    }

    companion object {
        val shared = ManifestV2JobGateway()
    }
}

private object UniFfiManifestV2JobNativeCore : ManifestV2JobNativeCore {
    private val compatibleBinding by lazy {
        val info = envoixCoreInfo()
        check(
            info.ffiApiVersion == EXPECTED_FFI_API_VERSION &&
                MANIFEST_JOB_CAPABILITY in info.capabilities &&
                STAGED_PROVIDER_CAPABILITY in info.capabilities,
        ) {
            "Unsupported Envoix Manifest v2 job binding: FFI ${info.ffiApiVersion}"
        }
        true
    }

    override suspend fun create(
        storeDirectory: String,
        compressionPolicy: FfiCompressionPolicyV2,
    ): ManifestV2NativeJob {
        requireCompatibleBinding()
        return UniFfiManifestV2NativeJob(
            createTransferJobV2(storeDirectory, compressionPolicy),
        )
    }

    override suspend fun restore(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2NativeJob {
        requireCompatibleBinding()
        return UniFfiManifestV2NativeJob(restoreTransferJobV2(storeDirectory, jobId))
    }

    private fun requireCompatibleBinding() {
        check(compatibleBinding)
    }

    private const val MANIFEST_JOB_CAPABILITY = "canonical_transfer_job_v2"
    private const val STAGED_PROVIDER_CAPABILITY = "typed_staged_provider_job_v1"
}

private class UniFfiManifestV2NativeJob(
    private val value: FfiTransferJobV2,
) : ManifestV2NativeJob {
    override suspend fun snapshot(): FfiTransferJobSnapshotV2 = value.snapshot()

    override suspend fun addStagedProviderRoots(roots: List<FfiStagedProviderRootV2>): FfiTransferJobSnapshotV2 =
        value.addStagedProviderRoots(roots)

    override suspend fun resolveSourceIssue(
        rootItemId: ULong,
        decision: FfiSourceDecisionV2,
    ): FfiTransferJobSnapshotV2 = value.resolveSourceIssue(rootItemId, decision, null)

    override suspend fun reauthorizeStagedProviderSource(
        rootItemId: ULong,
        sourcePath: String,
        issues: List<FfiProviderSourceIssueV2>,
    ): FfiTransferJobSnapshotV2 = value.reauthorizeStagedProviderSource(rootItemId, sourcePath, issues)

    override suspend fun cancelJob(): FfiTransferJobSnapshotV2 = value.cancelJob()

    override suspend fun sealForSend() {
        value.sealForSend()
    }

    override fun close() = value.close()
}

private fun String.toFfiCompressionPolicy(): FfiCompressionPolicyV2 =
    when (this) {
        "never" -> FfiCompressionPolicyV2.NEVER
        "always" -> FfiCompressionPolicyV2.ALWAYS
        "smart" -> FfiCompressionPolicyV2.SMART
        else -> error("Unknown compression policy")
    }

private fun ManifestV2StagedProviderRoot.toFfi(): FfiStagedProviderRootV2 =
    FfiStagedProviderRootV2(
        path = path,
        requestedName = requestedName,
        origin =
            when (origin) {
                ManifestV2SourceOrigin.Photos -> FfiSourceOriginV2.PHOTOS
                ManifestV2SourceOrigin.Share -> FfiSourceOriginV2.SHARE
                ManifestV2SourceOrigin.ContentUri -> FfiSourceOriginV2.CONTENT_URI
                ManifestV2SourceOrigin.FileProvider -> FfiSourceOriginV2.FILE_PROVIDER
            },
        issues = issues.map(ManifestV2ProviderIssue::toFfi),
    )

private fun ManifestV2ProviderIssue.toFfi(): FfiProviderSourceIssueV2 =
    FfiProviderSourceIssueV2(
        relativeComponents = relativeComponents,
        kind =
            when (kind) {
                ManifestV2ProviderIssueKind.PermissionDenied ->
                    FfiProviderSourceIssueKindV2.PERMISSION_DENIED
                ManifestV2ProviderIssueKind.Unavailable ->
                    FfiProviderSourceIssueKindV2.UNAVAILABLE
                ManifestV2ProviderIssueKind.InvalidName ->
                    FfiProviderSourceIssueKindV2.INVALID_NAME
                ManifestV2ProviderIssueKind.SpecialFile ->
                    FfiProviderSourceIssueKindV2.SPECIAL_FILE
            },
    )

private fun ManifestV2SourceDecision.toFfi(): FfiSourceDecisionV2 =
    when (this) {
        ManifestV2SourceDecision.ApprovePartial -> FfiSourceDecisionV2.APPROVE_PARTIAL
        ManifestV2SourceDecision.RemoveSelection -> FfiSourceDecisionV2.REMOVE_SELECTION
        ManifestV2SourceDecision.CancelJob -> FfiSourceDecisionV2.CANCEL_JOB
    }

private fun FfiTransferJobSnapshotV2.project(): ManifestV2JobSnapshot =
    ManifestV2JobSnapshot(
        jobId = jobId.also { require(it.isNotBlank()) { "Native job ID is missing" } },
        state =
            when (state) {
                FfiTransferJobStateV2.PREPARING -> ManifestV2JobState.Preparing
                FfiTransferJobStateV2.NEEDS_SOURCE_DECISION ->
                    ManifestV2JobState.NeedsSourceDecision
                FfiTransferJobStateV2.READY_TO_SEND -> ManifestV2JobState.ReadyToSend
                FfiTransferJobStateV2.SEALED -> ManifestV2JobState.Sealed
                FfiTransferJobStateV2.CANCELED -> ManifestV2JobState.Canceled
            },
        inventory =
            ManifestV2JobInventory(
                rootCount = inventory.rootCount.toPlatformInt(),
                fileCount = inventory.fileCount.toPlatformInt(),
                directoryCount = inventory.directoryCount.toPlatformInt(),
                totalBytes = inventory.totalPlaintextBytes.toPlatformLong(),
                warningCount = inventory.warningCount.toPlatformInt(),
            ),
        selections =
            selections.map { selection ->
                ManifestV2JobSelection(
                    rootItemId = selection.rootItemId.toPlatformLong(),
                    requestedName = selection.requestedName,
                    partialApproved = selection.partialApproved,
                    issues =
                        selection.issues.map { issue ->
                            ManifestV2JobIssue(
                                relativeComponents = issue.relativeComponents,
                                kind = issue.kind.project(),
                            )
                        },
                )
            },
    )

private fun FfiSourceIssueKindV2.project(): ManifestV2JobIssueKind =
    when (this) {
        FfiSourceIssueKindV2.PERMISSION_DENIED -> ManifestV2JobIssueKind.PermissionDenied
        FfiSourceIssueKindV2.UNAVAILABLE -> ManifestV2JobIssueKind.Unavailable
        FfiSourceIssueKindV2.INVALID_NAME -> ManifestV2JobIssueKind.InvalidName
        FfiSourceIssueKindV2.SYMBOLIC_LINK -> ManifestV2JobIssueKind.SymbolicLink
        FfiSourceIssueKindV2.SPECIAL_FILE -> ManifestV2JobIssueKind.SpecialFile
        FfiSourceIssueKindV2.SOURCE_CHANGED -> ManifestV2JobIssueKind.SourceChanged
        FfiSourceIssueKindV2.DEPTH_LIMIT -> ManifestV2JobIssueKind.DepthLimit
        FfiSourceIssueKindV2.ENTRY_LIMIT -> ManifestV2JobIssueKind.EntryLimit
    }

private fun Long.toNativeId(): ULong {
    require(this >= 0) { "Root item ID must be non-negative" }
    return toULong()
}

private fun UInt.toPlatformInt(): Int {
    require(this <= Int.MAX_VALUE.toUInt()) { "Native count exceeds platform range" }
    return toInt()
}

private fun ULong.toPlatformLong(): Long {
    require(this <= Long.MAX_VALUE.toULong()) { "Native value exceeds platform range" }
    return toLong()
}
