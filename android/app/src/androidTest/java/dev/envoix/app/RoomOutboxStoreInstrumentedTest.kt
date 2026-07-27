package dev.envoix.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.file.Files

@RunWith(AndroidJUnit4::class)
class RoomOutboxStoreInstrumentedTest {
    @Test
    fun claimIsDurableAndStaleAttemptsRequireExplicitRetry() {
        val root = Files.createTempDirectory("envoix-room-outbox").toFile()
        var now = 1_000L
        try {
            val file = root.resolve("outbox.json")
            val store = RoomOutboxStore.forTesting(file) { now }
            val queued =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = "0123456789abcdef0123456789abcdef",
                    rootNames = listOf("Photos"),
                    itemCount = 2,
                    directoryCount = 0,
                    totalBytes = 4096,
                )

            now = 2_000L
            val claimed = store.claimNext("relationship-a")
            assertNotNull(claimed)
            assertEquals(queued.id, claimed?.id)
            assertEquals(RoomOutboxState.Offering, claimed?.state)
            assertNull(store.claimNext("relationship-a"))

            val restored = RoomOutboxStore.forTesting(file) { 3_000L }
            assertEquals(1, restored.reconcileInterruptedAttempts())
            val interrupted = restored.entries().single()
            assertEquals(RoomOutboxState.NeedsAttention, interrupted.state)
            assertTrue(interrupted.lastError.orEmpty().contains("interrupted"))

            assertTrue(restored.retry(interrupted.id))
            assertEquals(RoomOutboxState.Queued, restored.entries().single().state)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun offerIdentityPreventsStaleCallbacksFromChangingANewerAttempt() {
        val root = Files.createTempDirectory("envoix-room-outbox-offer").toFile()
        try {
            val store = RoomOutboxStore.forTesting(root.resolve("outbox.json")) { 1_000L }
            val queued =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    rootNames = listOf("one"),
                    itemCount = 1,
                    directoryCount = 0,
                    totalBytes = 1,
                )
            val first = requireNotNull(store.claimNext("relationship-a"))

            assertFalse(store.requeue(queued.id, "stale-offer"))
            assertTrue(store.requeue(queued.id, requireNotNull(first.offerId)))
            val second = requireNotNull(store.claimNext("relationship-a"))
            assertFalse(store.markTransferring(queued.id, requireNotNull(first.offerId), 4))
            assertTrue(store.markTransferring(queued.id, requireNotNull(second.offerId), 5))
            assertEquals(5L, store.entries().single().transferId)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun sameManifestJobCannotBeQueuedForTwoRooms() {
        val root = Files.createTempDirectory("envoix-room-outbox-dedup").toFile()
        try {
            val store = RoomOutboxStore.forTesting(root.resolve("outbox.json")) { 1_000L }
            val first =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    rootNames = listOf("first"),
                    itemCount = 1,
                    directoryCount = 0,
                    totalBytes = 2,
                )
            val duplicate =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = first.jobId,
                    rootNames = listOf("different projection"),
                    itemCount = 9,
                    directoryCount = 3,
                    totalBytes = 99,
                )

            assertEquals(first.id, duplicate.id)
            assertEquals(first.relationshipId, duplicate.relationshipId)
            assertEquals(first.jobId, duplicate.jobId)
            assertEquals(first.rootNames, duplicate.rootNames)
            assertEquals(first.itemCount, duplicate.itemCount)
            assertEquals(first.directoryCount, duplicate.directoryCount)
            assertEquals(first.totalBytes, duplicate.totalBytes)
            assertEquals(first.state, duplicate.state)
            assertEquals(first.createdAtEpochMs, duplicate.createdAtEpochMs)
            assertEquals(first.updatedAtEpochMs, duplicate.updatedAtEpochMs)
            assertEquals(1, store.entries().size)
            val secondOwner =
                runCatching {
                    store.enqueue(
                        relationshipId = "relationship-b",
                        jobId = first.jobId,
                        rootNames = listOf("first"),
                        itemCount = 1,
                        directoryCount = 0,
                        totalBytes = 2,
                    )
                }
            assertTrue(secondOwner.isFailure)
            assertEquals(1, store.entries().size)
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun relationshipCleanupIsAllOrNothingWhileAnyEntryIsActive() {
        val root = Files.createTempDirectory("envoix-room-outbox-cleanup").toFile()
        try {
            val store = RoomOutboxStore.forTesting(root.resolve("outbox.json")) { 1_000L }
            val active =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = "cccccccccccccccccccccccccccccccc",
                    rootNames = listOf("active"),
                    itemCount = 1,
                    directoryCount = 0,
                    totalBytes = 3,
                )
            val queued =
                store.enqueue(
                    relationshipId = "relationship-a",
                    jobId = "dddddddddddddddddddddddddddddddd",
                    rootNames = listOf("queued"),
                    itemCount = 1,
                    directoryCount = 0,
                    totalBytes = 4,
                )
            requireNotNull(store.claimNext("relationship-a"))

            assertTrue(runCatching { store.removeAllInactive("relationship-a") }.isFailure)
            assertEquals(2, store.entries("relationship-a").size)

            assertTrue(store.requeue(active.id, requireNotNull(store.entries().first { it.id == active.id }.offerId)))
            assertTrue(store.markNeedsAttention(queued.id, "review"))
            assertEquals(2, store.removeAllInactive("relationship-a").size)
            assertTrue(store.entries("relationship-a").isEmpty())
        } finally {
            root.deleteRecursively()
        }
    }
}
