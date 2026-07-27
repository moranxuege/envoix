package dev.envoix.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

class ManifestArtifactCleanupTest {
    @Test
    fun removesExactPrivateJobArtifactsAndEveryGenerationJournal() {
        val root = Files.createTempDirectory("envoix-artifact-cleanup").toFile()
        val jobId = "0123456789abcdef0123456789abcdef"
        val otherJobId = "fedcba9876543210fedcba9876543210"
        try {
            val owned =
                listOf(
                    root.resolve("manifest-v2/source-staging/$jobId/source"),
                    root.resolve("manifest-v2/jobs/.envoix-staging/$jobId/source"),
                    root.resolve("manifest-v2/jobs/job-$jobId.json"),
                    root.resolve("manifest-v2/jobs/.job-$jobId.tmp"),
                    root.resolve("manifest-v2/destination-save/$jobId-0.json"),
                    root.resolve("manifest-v2/destination-save/$jobId-2.json.tmp"),
                )
            val unrelated =
                listOf(
                    root.resolve("manifest-v2/jobs/job-$otherJobId.json"),
                    root.resolve("manifest-v2/destination-save/$otherJobId-0.json"),
                )
            (owned + unrelated).forEach { file ->
                requireNotNull(file.parentFile).mkdirs()
                file.writeText("test")
            }

            deleteManifestJobArtifacts(root, jobId)

            owned.forEach { assertFalse("owned artifact remained: $it", it.exists()) }
            unrelated.forEach { assertTrue("unrelated artifact was deleted: $it", it.exists()) }
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun malformedJobIdCannotEscapeTheManifestDirectory() {
        val root = Files.createTempDirectory("envoix-artifact-cleanup-invalid").toFile()
        val sentinel = root.resolve("sentinel").apply { writeText("keep") }
        try {
            deleteManifestJobArtifacts(root, "../../sentinel")
            assertTrue(sentinel.exists())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun sharedRetryJobIsRetainedUntilEveryOwnerReleasesIt() {
        val jobId = "0123456789abcdef0123456789abcdef"

        assertTrue(
            manifestJobHasRemainingOwner(
                jobId,
                otherTransferJobIds = listOf(null, jobId),
                roomOutboxJobIds = emptyList(),
            ),
        )
        assertTrue(
            manifestJobHasRemainingOwner(
                jobId,
                otherTransferJobIds = emptyList(),
                roomOutboxJobIds = listOf(jobId),
            ),
        )
        assertFalse(
            manifestJobHasRemainingOwner(
                jobId,
                otherTransferJobIds = listOf(null, "fedcba9876543210fedcba9876543210"),
                roomOutboxJobIds = emptyList(),
            ),
        )
    }
}
