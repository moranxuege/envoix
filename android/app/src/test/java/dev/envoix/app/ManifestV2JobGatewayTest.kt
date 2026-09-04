package dev.envoix.app

import dev.envoix.app.ffi.FfiCompressionPolicyV2
import dev.envoix.app.ffi.FfiInventorySummaryV2
import dev.envoix.app.ffi.FfiProviderSourceIssueKindV2
import dev.envoix.app.ffi.FfiProviderSourceIssueV2
import dev.envoix.app.ffi.FfiSourceDecisionV2
import dev.envoix.app.ffi.FfiSourceOriginV2
import dev.envoix.app.ffi.FfiStagedProviderRootV2
import dev.envoix.app.ffi.FfiTransferJobSnapshotV2
import dev.envoix.app.ffi.FfiTransferJobStateV2
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ManifestV2JobGatewayTest {
    @Test
    fun `create projects the typed snapshot and closes its native handle`() =
        runTest {
            val job = FakeManifestV2NativeJob(snapshot(FfiTransferJobStateV2.READY_TO_SEND))
            val native = FakeManifestV2JobNativeCore(job)

            val result = ManifestV2JobGateway(native).create("/tmp/jobs", "smart")

            assertEquals(ManifestV2JobState.ReadyToSend, result.state)
            assertEquals(2, result.inventory.fileCount)
            assertEquals(FfiCompressionPolicyV2.SMART, native.createdCompressionPolicy)
            assertEquals(1, job.closeCount)
        }

    @Test
    fun `restored mutation preserves provider facts and closes after failure`() =
        runTest {
            val job = FakeManifestV2NativeJob(snapshot(FfiTransferJobStateV2.PREPARING))
            val native = FakeManifestV2JobNativeCore(job)
            job.addStagedProviderRoots = { roots ->
                job.receivedRoots = roots
                error("native preparation failed")
            }

            val failure =
                runCatching {
                    ManifestV2JobGateway(native).addStagedProviderRoot(
                        storeDirectory = "/tmp/jobs",
                        jobId = "00112233445566778899aabbccddeeff",
                        root =
                            ManifestV2StagedProviderRoot(
                                path = "/tmp/staging/photos",
                                requestedName = "Photos",
                                origin = ManifestV2SourceOrigin.Photos,
                                issues =
                                    listOf(
                                        ManifestV2ProviderIssue(
                                            relativeComponents = listOf("private.jpg"),
                                            kind = ManifestV2ProviderIssueKind.PermissionDenied,
                                        ),
                                    ),
                            ),
                    )
                }

            assertTrue(failure.exceptionOrNull() is IllegalStateException)
            assertEquals("00112233445566778899aabbccddeeff", native.restoredJobId)
            assertEquals(FfiSourceOriginV2.PHOTOS, job.receivedRoots.single().origin)
            assertEquals(
                FfiProviderSourceIssueKindV2.PERMISSION_DENIED,
                job
                    .receivedRoots
                    .single()
                    .issues
                    .single()
                    .kind,
            )
            assertEquals(1, job.closeCount)
        }
}

private class FakeManifestV2JobNativeCore(
    private val job: ManifestV2NativeJob,
) : ManifestV2JobNativeCore {
    var createdCompressionPolicy: FfiCompressionPolicyV2? = null
    var restoredJobId: String? = null

    override suspend fun create(
        storeDirectory: String,
        compressionPolicy: FfiCompressionPolicyV2,
    ): ManifestV2NativeJob {
        createdCompressionPolicy = compressionPolicy
        return job
    }

    override suspend fun restore(
        storeDirectory: String,
        jobId: String,
    ): ManifestV2NativeJob {
        restoredJobId = jobId
        return job
    }
}

private class FakeManifestV2NativeJob(
    private val currentSnapshot: FfiTransferJobSnapshotV2,
) : ManifestV2NativeJob {
    var closeCount = 0
    var receivedRoots = emptyList<FfiStagedProviderRootV2>()
    var addStagedProviderRoots: suspend (List<FfiStagedProviderRootV2>) -> FfiTransferJobSnapshotV2 =
        { currentSnapshot }

    override suspend fun snapshot(): FfiTransferJobSnapshotV2 = currentSnapshot

    override suspend fun addStagedProviderRoots(roots: List<FfiStagedProviderRootV2>): FfiTransferJobSnapshotV2 =
        addStagedProviderRoots.invoke(roots)

    override suspend fun resolveSourceIssue(
        rootItemId: ULong,
        decision: FfiSourceDecisionV2,
    ): FfiTransferJobSnapshotV2 = currentSnapshot

    override suspend fun reauthorizeStagedProviderSource(
        rootItemId: ULong,
        sourcePath: String,
        issues: List<FfiProviderSourceIssueV2>,
    ): FfiTransferJobSnapshotV2 = currentSnapshot

    override suspend fun cancelJob(): FfiTransferJobSnapshotV2 = currentSnapshot

    override suspend fun sealForSend() = Unit

    override fun close() {
        closeCount += 1
    }
}

private fun snapshot(state: FfiTransferJobStateV2) =
    FfiTransferJobSnapshotV2(
        jobId = "00112233445566778899aabbccddeeff",
        selectionRevision = 3uL,
        state = state,
        compressionPolicy = FfiCompressionPolicyV2.SMART,
        createdUnixMs = 10uL,
        updatedUnixMs = 11uL,
        inventory =
            FfiInventorySummaryV2(
                rootCount = 1u,
                fileCount = 2u,
                directoryCount = 1u,
                totalPlaintextBytes = 42uL,
                warningCount = 0u,
            ),
        selections = emptyList(),
    )
