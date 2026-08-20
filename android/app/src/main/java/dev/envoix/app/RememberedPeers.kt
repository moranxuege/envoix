package dev.envoix.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import dev.envoix.app.ffi.FfiApplicationEngine
import dev.envoix.app.ffi.FfiApplicationVault
import dev.envoix.app.ffi.FfiApplicationVaultException
import dev.envoix.app.ffi.FfiRememberedRelationship
import dev.envoix.app.ffi.envoixApplicationBindingInfo
import dev.envoix.app.ffi.envoixCoreInfo
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import java.io.Closeable
import java.io.File
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

data class RememberedPeerSummary(
    val relationshipId: String,
    val label: String,
    val generation: Long,
    val previousGeneration: Long?,
    val broker: String,
    val relay: String,
)

internal data class PendingRememberedPeer(
    val relationshipId: String,
    val label: String,
    val broker: String,
    val relay: String,
)

internal data class LoadedRememberedPeer(
    val summary: RememberedPeerSummary,
    val opaqueCredential: ByteArray,
)

/**
 * Thin Android host for the Engine-owned Relationship state. The Engine owns
 * labels, endpoints, generations, and durable references; Android owns only
 * the protected credential bytes and transient in-process session leases.
 */
internal class RememberedPeerStore private constructor(
    private val engine: FfiApplicationEngine,
) : Closeable {
    private val activeRelationships = mutableSetOf<String>()
    private val mutableChanges =
        MutableSharedFlow<Unit>(
            replay = 1,
            onBufferOverflow = BufferOverflow.DROP_OLDEST,
        ).apply { tryEmit(Unit) }
    val changes: SharedFlow<Unit> = mutableChanges.asSharedFlow()

    @Synchronized
    fun prepare(
        label: String,
        broker: String,
        relay: String,
    ): PendingRememberedPeer {
        val normalized = label.trim()
        require(normalized.isNotEmpty()) { "Device label is required" }
        val prepared = engine.prepareRelationship(normalized, broker, relay)
        return PendingRememberedPeer(
            relationshipId = prepared.relationshipId,
            label = prepared.label,
            broker = broker,
            relay = relay,
        )
    }

    @Synchronized
    fun discard(pending: PendingRememberedPeer) {
        engine.discardPreparedRelationship(pending.relationshipId)
    }

    @Synchronized
    fun peers(): List<RememberedPeerSummary> =
        engine
            .relationships()
            .map { it.summary() }
            .sortedBy { it.label.lowercase() }

    @Synchronized
    fun load(relationshipId: String): LoadedRememberedPeer? =
        engine.loadRelationship(relationshipId)?.let {
            LoadedRememberedPeer(
                summary = it.relationship.summary(),
                opaqueCredential = it.opaqueCredential,
            )
        }

    @Synchronized
    fun create(
        pending: PendingRememberedPeer,
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean {
        val ffiGeneration = generation.asFfiGeneration() ?: return false
        return runCatching {
            engine.commitRelationship(pending.relationshipId, opaqueCredential, ffiGeneration)
            true
        }.getOrDefault(false).also { created ->
            if (created) mutableChanges.tryEmit(Unit)
        }
    }

    @Synchronized
    fun rotate(
        relationshipId: String,
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean {
        val ffiGeneration = generation.asFfiGeneration() ?: return false
        return runCatching {
            engine.rotateRelationship(relationshipId, opaqueCredential, ffiGeneration)
            true
        }.getOrDefault(false).also { persisted ->
            if (persisted) mutableChanges.tryEmit(Unit)
        }
    }

    /**
     * Commits the generation consumed by an authenticated remembered control
     * attempt. A retry on the retained previous generation is an idempotent
     * catch-up, never a reason to regress the current generation.
     */
    @Synchronized
    fun advanceAfterPeerAuthentication(
        relationshipId: String,
        opaqueCredential: ByteArray,
        authenticatedGeneration: Long,
    ): Boolean {
        val record = load(relationshipId)?.summary ?: return false
        val nextGeneration =
            runCatching { Math.addExact(authenticatedGeneration, 1L) }
                .getOrNull()
                ?: return false
        return when {
            authenticatedGeneration == record.generation ->
                rotate(relationshipId, opaqueCredential, nextGeneration)
            authenticatedGeneration == record.previousGeneration &&
                nextGeneration == record.generation -> true
            else -> false
        }
    }

    @Synchronized
    fun rename(
        relationshipId: String,
        label: String,
    ): Boolean {
        val normalized = label.trim()
        if (normalized.isEmpty()) return false
        return runCatching {
            engine.renameRelationship(relationshipId, normalized)
            true
        }.getOrDefault(false).also { persisted ->
            if (persisted) mutableChanges.tryEmit(Unit)
        }
    }

    @Synchronized
    fun delete(relationshipId: String) {
        check(relationshipId !in activeRelationships) {
            "This remembered room is still active"
        }
        if (engine.relationships().none { it.relationshipId == relationshipId }) return
        engine.revokeRelationship(relationshipId)
        mutableChanges.tryEmit(Unit)
    }

    @Synchronized
    fun acquireSession(relationshipId: String): Boolean = activeRelationships.add(relationshipId)

    @Synchronized
    fun releaseSession(relationshipId: String) {
        activeRelationships.remove(relationshipId)
    }

    override fun close() = engine.close()

    companion object {
        private const val ENGINE_DIRECTORY = "application-engine-v2"
        private const val VAULT_DIRECTORY = "application-vault-v2"
        private const val KEY_ALIAS = "dev.envoix.application-vault.v2"

        @Volatile
        private var instance: RememberedPeerStore? = null

        fun get(context: Context): RememberedPeerStore =
            instance ?: synchronized(this) {
                instance ?: openForApplication(context.applicationContext).also { instance = it }
            }

        internal fun openForTesting(
            stateDirectory: File,
            vaultDirectory: File,
            keyAlias: String,
        ): RememberedPeerStore =
            open(stateDirectory, AndroidApplicationVault(vaultDirectory, keyAlias))

        private fun open(
            stateDirectory: File,
            vault: FfiApplicationVault,
        ): RememberedPeerStore {
            validateApplicationBinding(envoixCoreInfo(), envoixApplicationBindingInfo())
            return RememberedPeerStore(
                FfiApplicationEngine.openPersistent(stateDirectory.absolutePath, vault),
            )
        }

        private fun openForApplication(context: Context): RememberedPeerStore {
            val stateDirectory = File(context.noBackupFilesDir, ENGINE_DIRECTORY)
            val legacyMetadata = File(context.filesDir, "remembered-peers/relationships-v1.json")
            val currentState = File(stateDirectory, "engine-state-v2.json")
            if (legacyMetadata.isFile && !currentState.exists()) {
                LogStore.append(
                    "Android Relationship v1 state was retained but is not imported; re-pair for v0.3",
                )
            }
            return open(
                stateDirectory = stateDirectory,
                vault =
                    AndroidApplicationVault(
                        File(context.noBackupFilesDir, VAULT_DIRECTORY),
                        KEY_ALIAS,
                    ),
            )
        }
    }
}

/** AES-GCM envelope storage backed by a non-exportable Android Keystore key. */
internal class AndroidApplicationVault(
    private val directory: File,
    private val keyAlias: String,
) : FfiApplicationVault {
    @Synchronized
    override fun contains(reference: String): Boolean =
        vaultOperation {
            val file = credentialFile(reference)
            if (file.exists() && !file.isFile) {
                throw FfiApplicationVaultException.CorruptData()
            }
            file.isFile
        }

    @Synchronized
    override fun store(
        reference: String,
        opaqueCredential: ByteArray,
    ) {
        vaultOperation {
            if (opaqueCredential.isEmpty() || opaqueCredential.size > MAX_SECRET_BYTES) {
                throw FfiApplicationVaultException.InvalidRequest()
            }
            val encrypted = encrypt(opaqueCredential, aad(reference))
            atomicWrite(credentialFile(reference), encrypted)
        }
    }

    @Synchronized
    override fun load(reference: String): ByteArray? =
        vaultOperation {
            val file = credentialFile(reference)
            if (!file.exists()) return@vaultOperation null
            if (!file.isFile) {
                throw FfiApplicationVaultException.CorruptData()
            }
            if (file.length() !in minEnvelopeBytes..maxEnvelopeBytes) {
                throw FfiApplicationVaultException.CorruptData()
            }
            decrypt(file.readBytes(), aad(reference))
        }

    @Synchronized
    override fun delete(reference: String) {
        vaultOperation {
            val file = credentialFile(reference)
            if (file.exists() && !file.isFile) {
                throw FfiApplicationVaultException.CorruptData()
            }
            Files.deleteIfExists(file.toPath())
        }
    }

    private fun encrypt(
        plaintext: ByteArray,
        aad: ByteArray,
    ): ByteArray =
        vaultOperation {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, encryptionKey())
            val nonce = cipher.iv
            if (nonce.size != NONCE_BYTES) {
                throw FfiApplicationVaultException.Unavailable()
            }
            cipher.updateAAD(aad)
            byteArrayOf(FORMAT_VERSION) + nonce + cipher.doFinal(plaintext)
        }

    private fun decrypt(
        envelope: ByteArray,
        aad: ByteArray,
    ): ByteArray =
        vaultOperation(corruptData = true) {
            if (envelope.size < 1 + NONCE_BYTES + TAG_BITS / 8 || envelope[0] != FORMAT_VERSION) {
                throw FfiApplicationVaultException.CorruptData()
            }
            val nonce = envelope.copyOfRange(1, 1 + NONCE_BYTES)
            val ciphertext = envelope.copyOfRange(1 + NONCE_BYTES, envelope.size)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, decryptionKey(), GCMParameterSpec(TAG_BITS, nonce))
            cipher.updateAAD(aad)
            cipher.doFinal(ciphertext)
        }

    private fun encryptionKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(keyAlias, null) as? SecretKey)?.let { return it }
        return KeyGenerator
            .getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply {
                init(
                    KeyGenParameterSpec
                        .Builder(
                            keyAlias,
                            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                        ).setKeySize(256)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .build(),
                )
            }.generateKey()
    }

    private fun decryptionKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        return store.getKey(keyAlias, null) as? SecretKey
            ?: throw FfiApplicationVaultException.CorruptData()
    }

    private fun credentialFile(reference: String): File {
        if (reference.isEmpty() ||
            reference.length > MAX_REFERENCE_BYTES ||
            reference.any { !it.isAsciiReferenceCharacter() }
        ) {
            throw FfiApplicationVaultException.InvalidRequest()
        }
        return File(directory, "$reference.bin")
    }

    private fun aad(reference: String): ByteArray =
        "$AAD_SCHEMA\u0000$reference".toByteArray(Charsets.UTF_8)

    private fun atomicWrite(
        target: File,
        bytes: ByteArray,
    ) {
        if ((!directory.exists() && !directory.mkdirs()) || !directory.isDirectory) {
            throw FfiApplicationVaultException.Unavailable()
        }
        val temporary = File(directory, "${target.name}.${UUID.randomUUID()}.tmp")
        try {
            temporary.writeBytes(bytes)
            try {
                Files.move(
                    temporary.toPath(),
                    target.toPath(),
                    StandardCopyOption.REPLACE_EXISTING,
                    StandardCopyOption.ATOMIC_MOVE,
                )
            } catch (_: AtomicMoveNotSupportedException) {
                Files.move(
                    temporary.toPath(),
                    target.toPath(),
                    StandardCopyOption.REPLACE_EXISTING,
                )
            }
        } finally {
            temporary.delete()
        }
    }

    private inline fun <T> vaultOperation(
        corruptData: Boolean = false,
        operation: () -> T,
    ): T =
        try {
            operation()
        } catch (error: FfiApplicationVaultException) {
            throw error
        } catch (_: UserNotAuthenticatedException) {
            throw FfiApplicationVaultException.InteractionRequired()
        } catch (_: KeyPermanentlyInvalidatedException) {
            throw FfiApplicationVaultException.CorruptData()
        } catch (_: SecurityException) {
            throw FfiApplicationVaultException.PermissionDenied()
        } catch (_: Throwable) {
            if (corruptData) {
                throw FfiApplicationVaultException.CorruptData()
            }
            throw FfiApplicationVaultException.Unavailable()
        }

    companion object {
        private const val AAD_SCHEMA = "dev.envoix.app/application-vault/v2"
        private const val FORMAT_VERSION: Byte = 2
        private const val NONCE_BYTES = 12
        private const val TAG_BITS = 128
        private const val MAX_REFERENCE_BYTES = 128
        private const val MAX_SECRET_BYTES = 64 * 1024
        private val minEnvelopeBytes = (1 + NONCE_BYTES + TAG_BITS / 8).toLong()
        private val maxEnvelopeBytes = MAX_SECRET_BYTES.toLong() + minEnvelopeBytes
    }
}

private fun FfiRememberedRelationship.summary() =
    RememberedPeerSummary(
        relationshipId = relationshipId,
        label = label,
        generation = generation.asAndroidGeneration(),
        previousGeneration = previousGeneration?.asAndroidGeneration(),
        broker = broker,
        relay = relay,
    )

private fun Long.asFfiGeneration(): ULong? = takeIf { it >= 0 }?.toULong()

private fun ULong.asAndroidGeneration(): Long {
    if (this > Long.MAX_VALUE.toULong()) {
        throw IllegalStateException("Relationship generation exceeds the Android host range")
    }
    return toLong()
}

private fun Char.isAsciiReferenceCharacter(): Boolean =
    this in 'a'..'z' || this in 'A'..'Z' || this in '0'..'9' || this == '-' || this == '_'
