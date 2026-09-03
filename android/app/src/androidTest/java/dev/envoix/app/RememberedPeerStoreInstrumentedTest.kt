package dev.envoix.app

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.envoix.app.ffi.FfiApplicationErrorCode
import dev.envoix.app.ffi.FfiApplicationException
import dev.envoix.app.ffi.FfiApplicationVaultException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.KeyStore
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class RememberedPeerStoreInstrumentedTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()

    @Test
    fun engineOwnsRoundTripRotationRestartAndRevocation() {
        val environment = TestEnvironment(context)
        var store = environment.open()
        val first = store.prepare("first", "broker", "relay")
        val second = store.prepare("second", "broker", "relay")
        val initialCredential = opaqueCredential(0x21)
        val rotatedCredential = opaqueCredential(0x42)
        try {
            assertTrue(store.create(first, initialCredential, 0))
            val firstFile = environment.credentialFiles().single()
            val firstEnvelope = firstFile.readBytes()

            assertTrue(store.create(second, initialCredential, 0))
            val secondFile = environment.credentialFiles().single { it != firstFile }
            assertNotEquals(
                firstEnvelope.copyOfRange(1, 13).toList(),
                secondFile.readBytes().copyOfRange(1, 13).toList(),
            )

            store.close()
            store = environment.open()
            assertArrayEquals(
                initialCredential,
                store.load(first.relationshipId)?.opaqueCredential,
            )

            assertTrue(store.rotate(first.relationshipId, rotatedCredential, 1))
            val rotated = requireNotNull(store.load(first.relationshipId))
            assertArrayEquals(rotatedCredential, rotated.opaqueCredential)
            assertEquals(0L, rotated.summary.previousGeneration)
            assertNotEquals(
                firstEnvelope.copyOfRange(1, 13).toList(),
                firstFile.readBytes().copyOfRange(1, 13).toList(),
            )

            store.delete(first.relationshipId)
            assertNull(store.load(first.relationshipId))
            assertFalse(firstFile.exists())
            assertTrue(secondFile.exists())
        } finally {
            store.close()
            environment.cleanup()
        }
    }

    @Test
    fun authenticatedGenerationAdvancesOnceAndPreviousRetryIsIdempotent() {
        val environment = TestEnvironment(context)
        val store = environment.open()
        val pending = store.prepare("control", "broker", "relay")
        try {
            assertTrue(store.create(pending, opaqueCredential(0x11), 7))
            assertTrue(
                store.advanceAfterPeerAuthentication(
                    pending.relationshipId,
                    opaqueCredential(0x22),
                    7,
                ),
            )
            val advanced = requireNotNull(store.load(pending.relationshipId))
            assertEquals(8L, advanced.summary.generation)
            assertEquals(7L, advanced.summary.previousGeneration)

            assertTrue(
                store.advanceAfterPeerAuthentication(
                    pending.relationshipId,
                    opaqueCredential(0x33),
                    7,
                ),
            )
            assertEquals(
                8L,
                requireNotNull(store.load(pending.relationshipId)).summary.generation,
            )
            assertFalse(
                store.advanceAfterPeerAuthentication(
                    pending.relationshipId,
                    opaqueCredential(0x44),
                    6,
                ),
            )
            assertFalse(
                store.advanceAfterPeerAuthentication(
                    pending.relationshipId,
                    opaqueCredential(0x55),
                    Long.MAX_VALUE,
                ),
            )
        } finally {
            store.close()
            environment.cleanup()
        }
    }

    @Test
    fun modifiedCiphertextFailsClosedUntilExplicitDelete() {
        val environment = TestEnvironment(context)
        val store = environment.open()
        val pending = store.prepare("corrupt", "broker", "relay")
        try {
            assertTrue(store.create(pending, opaqueCredential(0x31), 0))
            val file = environment.credentialFiles().single()
            val envelope = file.readBytes()
            envelope[envelope.lastIndex] = (envelope.last().toInt() xor 1).toByte()
            file.writeBytes(envelope)

            assertApplicationError(FfiApplicationErrorCode.VAULT_CORRUPT) {
                store.load(pending.relationshipId)
            }
            assertTrue(store.peers().any { it.relationshipId == pending.relationshipId })
            assertTrue(file.exists())
            store.delete(pending.relationshipId)
            assertFalse(store.peers().any { it.relationshipId == pending.relationshipId })
            assertFalse(file.exists())
        } finally {
            store.close()
            environment.cleanup()
        }
    }

    @Test
    fun missingCiphertextIsTypedUntilExplicitDelete() {
        val environment = TestEnvironment(context)
        val store = environment.open()
        val pending = store.prepare("missing", "broker", "relay")
        try {
            assertTrue(store.create(pending, opaqueCredential(0x51), 0))
            assertTrue(environment.credentialFiles().single().delete())

            assertApplicationError(FfiApplicationErrorCode.VAULT_CORRUPT) {
                store.load(pending.relationshipId)
            }
            assertTrue(store.peers().any { it.relationshipId == pending.relationshipId })
            store.delete(pending.relationshipId)
            assertFalse(store.peers().any { it.relationshipId == pending.relationshipId })
        } finally {
            store.close()
            environment.cleanup()
        }
    }

    @Test
    fun persistentEngineRejectsSecondOwnerAndLegacyStateWithoutMutation() {
        val owned = TestEnvironment(context)
        val owner = owned.open()
        try {
            assertApplicationError(FfiApplicationErrorCode.STATE_ALREADY_OWNED) {
                owned.open()
            }
        } finally {
            owner.close()
            owned.cleanup()
        }

        val legacy = TestEnvironment(context)
        val legacyFile = File(legacy.stateDirectory.apply { mkdirs() }, "engine-state-v1.json")
        val bytes = "legacy bytes are not decoded".toByteArray()
        legacyFile.writeBytes(bytes)
        try {
            assertApplicationError(FfiApplicationErrorCode.UNSUPPORTED_PERSISTENT_STATE) {
                legacy.open()
            }
            assertArrayEquals(bytes, legacyFile.readBytes())
        } finally {
            legacy.cleanup()
        }
    }

    @Test
    fun vaultRejectsUnsafeReferencesAndOversizedValuesBeforeCryptography() {
        val environment = TestEnvironment(context)
        val vault = AndroidApplicationVault(environment.vaultDirectory, environment.keyAlias)
        try {
            assertThrows(FfiApplicationVaultException.InvalidRequest::class.java) {
                vault.contains("../credential")
            }
            assertThrows(FfiApplicationVaultException.InvalidRequest::class.java) {
                vault.store("credential", ByteArray(64 * 1024 + 1))
            }
            environment.vaultDirectory.mkdirs()
            File(environment.vaultDirectory, "credential.bin").writeBytes(ByteArray(64 * 1024 + 30))
            assertThrows(FfiApplicationVaultException.CorruptData::class.java) {
                vault.load("credential")
            }
        } finally {
            environment.cleanup()
        }
    }

    private fun assertApplicationError(
        code: FfiApplicationErrorCode,
        operation: () -> Unit,
    ) {
        val error = assertThrows(FfiApplicationException.Failed::class.java) { operation() }
        assertEquals(code, error.code)
    }

    private fun opaqueCredential(seed: Int): ByteArray =
        byteArrayOf(
            'E'.code.toByte(),
            'N'.code.toByte(),
            'V'.code.toByte(),
            'R'.code.toByte(),
            1,
        ) +
            ByteArray(32) { (seed + it).toByte() }
}

private class TestEnvironment(
    context: Context,
) {
    private val root =
        File(context.noBackupFilesDir, "relationship-engine-tests/${UUID.randomUUID()}")
    val stateDirectory = File(root, "state")
    val vaultDirectory = File(root, "vault")
    val keyAlias = "dev.envoix.application-vault.test.${UUID.randomUUID()}"

    fun open(): RememberedPeerStore = RememberedPeerStore.openForTesting(stateDirectory, vaultDirectory, keyAlias)

    fun credentialFiles(): List<File> = vaultDirectory.listFiles()?.filter { it.isFile && it.extension == "bin" }.orEmpty()

    fun cleanup() {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (keyStore.containsAlias(keyAlias)) keyStore.deleteEntry(keyAlias)
        root.deleteRecursively()
    }
}
