package dev.envoix.app

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.json.JSONArray
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class RememberedPeerStoreInstrumentedTest {
    private val context = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val store = RememberedPeerStore.get(context)

    @Test
    fun roundTripRotationDeletionAndFreshNonces() {
        val first = store.prepare("test-${System.nanoTime()}", "broker", "relay")
        val second = store.prepare("test-${System.nanoTime()}-2", "broker", "relay")
        val opaque = ByteArray(37) { it.toByte() }
        try {
            assertTrue(store.create(first, opaque, 0))
            assertTrue(store.create(second, opaque, 0))
            assertArrayEquals(opaque, store.load(first.relationshipId)?.opaqueCredential)

            val firstEnvelope = credentialFile(first, 0).readBytes()
            val secondEnvelope = credentialFile(second, 0).readBytes()
            assertNotEquals(
                firstEnvelope.copyOfRange(1, 13).toList(),
                secondEnvelope.copyOfRange(1, 13).toList(),
            )

            assertTrue(store.rotate(first.relationshipId, opaque, 1))
            val rotated = requireNotNull(store.load(first.relationshipId))
            assertArrayEquals(opaque, rotated.opaqueCredential)
            assertTrue(rotated.summary.previousGeneration == 0L)
            assertNotEquals(
                firstEnvelope.copyOfRange(1, 13).toList(),
                credentialFile(first, 1).readBytes().copyOfRange(1, 13).toList(),
            )

            store.delete(first.relationshipId)
            assertNull(store.load(first.relationshipId))
            assertFalse(credentialFile(first, 0).exists())
            assertFalse(credentialFile(first, 1).exists())
        } finally {
            store.delete(first.relationshipId)
            store.delete(second.relationshipId)
        }
    }

    @Test
    fun authenticatedControlGenerationAdvancesOnceAndPreviousRetryIsIdempotent() {
        val pending = store.prepare("control-${System.nanoTime()}", "broker", "relay")
        val opaque = ByteArray(37) { (it + 3).toByte() }
        try {
            assertTrue(store.create(pending, opaque, 7))
            assertTrue(store.advanceAfterPeerAuthentication(pending.relationshipId, opaque, 7))
            val advanced = requireNotNull(store.load(pending.relationshipId))
            assertTrue(advanced.summary.generation == 8L)
            assertTrue(advanced.summary.previousGeneration == 7L)

            assertTrue(store.advanceAfterPeerAuthentication(pending.relationshipId, opaque, 7))
            assertTrue(requireNotNull(store.load(pending.relationshipId)).summary.generation == 8L)
            assertFalse(store.advanceAfterPeerAuthentication(pending.relationshipId, opaque, 6))
            assertFalse(store.advanceAfterPeerAuthentication(pending.relationshipId, opaque, Long.MAX_VALUE))
        } finally {
            store.delete(pending.relationshipId)
        }
    }

    @Test
    fun modifiedCiphertextFailsClosed() {
        val pending = store.prepare("corrupt-${System.nanoTime()}", "broker", "relay")
        try {
            assertTrue(store.create(pending, ByteArray(37) { 0x5a }, 0))
            val file = credentialFile(pending, 0)
            val envelope = file.readBytes()
            envelope[envelope.lastIndex] = (envelope.last().toInt() xor 1).toByte()
            file.writeBytes(envelope)

            assertTrue(runCatching { store.load(pending.relationshipId) }.exceptionOrNull() is IllegalStateException)
            assertTrue(store.peers().any { it.relationshipId == pending.relationshipId })
            assertTrue(file.exists())
        } finally {
            store.delete(pending.relationshipId)
        }
    }

    @Test
    fun modifiedAuthenticatedMetadataFailsClosed() {
        val pending = store.prepare("metadata-${System.nanoTime()}", "broker", "relay")
        val tamperedRelationshipId = UUID.randomUUID().toString()
        try {
            assertTrue(store.create(pending, ByteArray(37) { 0x37 }, 0))
            val metadata = metadataFile()
            val records = JSONArray(metadata.readText())
            val index =
                (0 until records.length()).first {
                    records.getJSONObject(it).getString("relationship_id") == pending.relationshipId
                }
            records.getJSONObject(index).put("relationship_id", tamperedRelationshipId)
            metadata.writeText(records.toString())

            assertTrue(runCatching { store.load(tamperedRelationshipId) }.exceptionOrNull() is IllegalStateException)
            assertTrue(store.peers().any { it.relationshipId == tamperedRelationshipId })
            assertTrue(credentialFile(pending, 0).exists())
        } finally {
            store.delete(pending.relationshipId)
            store.delete(tamperedRelationshipId)
        }
    }

    @Test
    fun missingCiphertextFailsClosed() {
        val pending = store.prepare("missing-${System.nanoTime()}", "broker", "relay")
        try {
            assertTrue(store.create(pending, ByteArray(37) { 0x24 }, 0))
            assertTrue(credentialFile(pending, 0).delete())

            assertTrue(runCatching { store.load(pending.relationshipId) }.exceptionOrNull() is IllegalStateException)
            assertTrue(store.peers().any { it.relationshipId == pending.relationshipId })
        } finally {
            store.delete(pending.relationshipId)
        }
    }

    private fun credentialFile(
        pending: PendingRememberedPeer,
        generation: Long,
    ) = File(
        context.noBackupFilesDir,
        "remembered-credentials-v1/${pending.credentialReference}-$generation.bin",
    )

    private fun metadataFile() = File(context.filesDir, "remembered-peers/relationships-v1.json")
}
