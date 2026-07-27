package dev.envoix.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.KeyStore
import java.util.UUID
import javax.crypto.AEADBadTagException
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
    val credentialReference: String,
    val label: String,
    val broker: String,
    val relay: String,
)

internal data class LoadedRememberedPeer(
    val summary: RememberedPeerSummary,
    val opaqueCredential: ByteArray,
)

private data class RememberedPeerRecord(
    val relationshipId: String,
    val credentialReference: String,
    val label: String,
    val generation: Long,
    val previousGeneration: Long?,
    val broker: String,
    val relay: String,
) {
    fun summary() =
        RememberedPeerSummary(
            relationshipId,
            label,
            generation,
            previousGeneration,
            broker,
            relay,
        )

    fun toJson() =
        JSONObject()
            .put("relationship_id", relationshipId)
            .put("credential_reference", credentialReference)
            .put("label", label)
            .put("generation", generation)
            .put("previous_generation", previousGeneration ?: JSONObject.NULL)
            .put("broker", broker)
            .put("relay", relay)

    companion object {
        fun fromJson(value: JSONObject) =
            RememberedPeerRecord(
                relationshipId = value.getString("relationship_id"),
                credentialReference = value.getString("credential_reference"),
                label = value.getString("label"),
                generation = value.getLong("generation"),
                previousGeneration =
                    value.optLong("previous_generation", -1).takeIf { it >= 0 },
                broker = value.getString("broker"),
                relay = value.optString("relay"),
            )
    }
}

/**
 * Android's protected remembered-credential backend. Metadata contains only a
 * random reference; opaque Rust bytes are AES-GCM wrapped by a non-exportable
 * Android Keystore key and stored under noBackupFilesDir.
 */
internal class RememberedPeerStore private constructor(
    private val context: Context,
) {
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
        require(label.trim().isNotEmpty()) { "Device label is required" }
        return PendingRememberedPeer(
            relationshipId = UUID.randomUUID().toString(),
            credentialReference = UUID.randomUUID().toString(),
            label = label.trim(),
            broker = broker,
            relay = relay,
        )
    }

    @Synchronized
    fun peers(): List<RememberedPeerSummary> =
        readRecords()
            .sortedBy { it.label.lowercase() }
            .map(RememberedPeerRecord::summary)

    @Synchronized
    fun load(relationshipId: String): LoadedRememberedPeer? {
        val record = readRecords().firstOrNull { it.relationshipId == relationshipId } ?: return null
        return LoadedRememberedPeer(record.summary(), loadCredential(record))
    }

    @Synchronized
    fun create(
        pending: PendingRememberedPeer,
        opaqueCredential: ByteArray,
        generation: Long,
    ): Boolean {
        val records = readRecords().toMutableList()
        if (records.any { it.relationshipId == pending.relationshipId }) return false
        val record =
            RememberedPeerRecord(
                pending.relationshipId,
                pending.credentialReference,
                pending.label,
                generation,
                null,
                pending.broker,
                pending.relay,
            )
        return runCatching {
            writeCredential(record, opaqueCredential)
            try {
                records += record
                writeRecords(records)
            } catch (error: Throwable) {
                deleteCredentialFiles(record.credentialReference)
                throw error
            }
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
        val records = readRecords().toMutableList()
        val index = records.indexOfFirst { it.relationshipId == relationshipId }
        if (index < 0) return false
        val previous = records[index]
        if (generation <= previous.generation) return generation == previous.generation
        val rotated =
            previous.copy(
                generation = generation,
                previousGeneration = previous.generation,
            )
        return runCatching {
            writeCredential(rotated, opaqueCredential)
            records[index] = rotated
            writeRecords(records)
            previous.previousGeneration?.let {
                credentialFile(previous.credentialReference, it).delete()
            }
            true
        }.getOrElse {
            credentialFile(rotated.credentialReference, rotated.generation).delete()
            false
        }.also { persisted ->
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
        val record = readRecords().firstOrNull { it.relationshipId == relationshipId } ?: return false
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
        val records = readRecords().toMutableList()
        val index = records.indexOfFirst { it.relationshipId == relationshipId }
        if (index < 0) return false
        if (records[index].label == normalized) return true
        records[index] = records[index].copy(label = normalized)
        return runCatching {
            writeRecords(records)
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
        val records = readRecords().toMutableList()
        val record = records.firstOrNull { it.relationshipId == relationshipId } ?: return
        records.remove(record)
        writeRecords(records)
        deleteCredentialFiles(record.credentialReference)
        mutableChanges.tryEmit(Unit)
    }

    @Synchronized
    fun acquireSession(relationshipId: String): Boolean = activeRelationships.add(relationshipId)

    @Synchronized
    fun releaseSession(relationshipId: String) {
        activeRelationships.remove(relationshipId)
    }

    private fun loadCredential(record: RememberedPeerRecord): ByteArray {
        val generations = listOfNotNull(record.generation, record.previousGeneration)
        val failures = mutableListOf<Throwable>()
        for (generation in generations) {
            try {
                return decrypt(
                    credentialFile(record.credentialReference, generation).readBytes(),
                    aad(record, generation),
                )
            } catch (error: Throwable) {
                failures += error
            }
        }
        throw IllegalStateException(
            "remembered credential is temporarily unavailable; the relationship was preserved",
            failures.lastOrNull(),
        ).also { failure ->
            failures.dropLast(1).forEach(failure::addSuppressed)
        }
    }

    private fun writeCredential(
        record: RememberedPeerRecord,
        opaqueCredential: ByteArray,
    ) {
        atomicWrite(
            credentialFile(record.credentialReference, record.generation),
            encrypt(opaqueCredential, aad(record, record.generation)),
        )
    }

    private fun encrypt(
        plaintext: ByteArray,
        aad: ByteArray,
    ): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, encryptionKey())
        val nonce = cipher.iv
        require(nonce.size == NONCE_BYTES)
        cipher.updateAAD(aad)
        return byteArrayOf(FORMAT_VERSION) + nonce + cipher.doFinal(plaintext)
    }

    private fun decrypt(
        envelope: ByteArray,
        aad: ByteArray,
    ): ByteArray {
        require(envelope.size >= 1 + NONCE_BYTES + TAG_BITS / 8)
        require(envelope[0] == FORMAT_VERSION)
        val nonce = envelope.copyOfRange(1, 1 + NONCE_BYTES)
        val ciphertext = envelope.copyOfRange(1 + NONCE_BYTES, envelope.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, decryptionKey(), GCMParameterSpec(TAG_BITS, nonce))
        cipher.updateAAD(aad)
        return try {
            cipher.doFinal(ciphertext)
        } catch (error: AEADBadTagException) {
            throw IllegalStateException("remembered credential authentication failed", error)
        }
    }

    private fun aad(
        record: RememberedPeerRecord,
        generation: Long,
    ): ByteArray =
        listOf(
            AAD_SCHEMA,
            record.credentialReference,
            record.relationshipId,
            generation.toString(),
        ).joinToString("\u0000").toByteArray(Charsets.UTF_8)

    private fun encryptionKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator
            .getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply {
                init(
                    KeyGenParameterSpec
                        .Builder(
                            KEY_ALIAS,
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
        return store.getKey(KEY_ALIAS, null) as? SecretKey
            ?: throw IllegalStateException("remembered credential key is unavailable")
    }

    private fun readRecords(): List<RememberedPeerRecord> {
        val file = metadataFile()
        if (!file.exists()) return emptyList()
        return try {
            val values = JSONArray(file.readText())
            (0 until values.length()).map {
                RememberedPeerRecord.fromJson(values.getJSONObject(it))
            }
        } catch (error: Throwable) {
            throw IllegalStateException(
                "remembered-device metadata is temporarily unavailable; no records were changed",
                error,
            )
        }
    }

    private fun writeRecords(records: List<RememberedPeerRecord>) {
        val values = JSONArray()
        records.forEach { values.put(it.toJson()) }
        atomicWrite(metadataFile(), values.toString().toByteArray(Charsets.UTF_8))
    }

    private fun deleteCredentialFiles(reference: String) {
        credentialDirectory()
            .listFiles()
            ?.filter { it.name.startsWith("$reference-") }
            ?.forEach(File::delete)
    }

    private fun credentialFile(
        reference: String,
        generation: Long,
    ): File {
        UUID.fromString(reference)
        return File(credentialDirectory(), "$reference-$generation.bin")
    }

    private fun credentialDirectory() = File(context.noBackupFilesDir, "remembered-credentials-v1").apply { mkdirs() }

    private fun metadataFile() =
        File(context.filesDir, "remembered-peers/relationships-v1.json").apply {
            parentFile?.mkdirs()
        }

    private fun atomicWrite(
        target: File,
        bytes: ByteArray,
    ) {
        target.parentFile?.mkdirs()
        val temporary = File(target.parentFile, "${target.name}.${UUID.randomUUID()}.tmp")
        temporary.writeBytes(bytes)
        runCatching {
            Files.move(
                temporary.toPath(),
                target.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
                StandardCopyOption.ATOMIC_MOVE,
            )
        }.getOrElse {
            Files.move(
                temporary.toPath(),
                target.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    companion object {
        private const val KEY_ALIAS = "dev.envoix.remembered-credential.v1"
        private const val AAD_SCHEMA = "dev.envoix.app/remembered-credential/v1"
        private const val FORMAT_VERSION: Byte = 1
        private const val NONCE_BYTES = 12
        private const val TAG_BITS = 128

        @Volatile
        private var instance: RememberedPeerStore? = null

        fun get(context: Context): RememberedPeerStore =
            instance ?: synchronized(this) {
                instance ?: RememberedPeerStore(context.applicationContext).also { instance = it }
            }
    }
}
