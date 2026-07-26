package dev.envoix.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
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
    fun peers(): List<RememberedPeerSummary> {
        val records = readRecords()
        val usable = mutableListOf<RememberedPeerRecord>()
        for (record in records) {
            if (loadCredential(record) != null) {
                usable += record
            } else {
                deleteCredentialFiles(record.credentialReference)
            }
        }
        if (usable.size != records.size) writeRecords(usable)
        reclaimOrphans(usable.mapTo(mutableSetOf()) { it.credentialReference })
        return usable.sortedBy { it.label.lowercase() }.map(RememberedPeerRecord::summary)
    }

    @Synchronized
    fun load(relationshipId: String): LoadedRememberedPeer? {
        val records = readRecords().toMutableList()
        val index = records.indexOfFirst { it.relationshipId == relationshipId }
        if (index < 0) return null
        val record = records[index]
        val credential = loadCredential(record)
        if (credential == null) {
            records.removeAt(index)
            writeRecords(records)
            deleteCredentialFiles(record.credentialReference)
            return null
        }
        return LoadedRememberedPeer(record.summary(), credential)
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
        }.getOrDefault(false)
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
        }
    }

    @Synchronized
    fun delete(relationshipId: String) {
        val records = readRecords().toMutableList()
        val record = records.firstOrNull { it.relationshipId == relationshipId } ?: return
        records.remove(record)
        writeRecords(records)
        deleteCredentialFiles(record.credentialReference)
    }

    @Synchronized
    fun acquireSession(relationshipId: String): Boolean = activeRelationships.add(relationshipId)

    @Synchronized
    fun releaseSession(relationshipId: String) {
        activeRelationships.remove(relationshipId)
    }

    private fun loadCredential(record: RememberedPeerRecord): ByteArray? {
        val generations = listOfNotNull(record.generation, record.previousGeneration)
        for (generation in generations) {
            val bytes =
                runCatching {
                    decrypt(
                        credentialFile(record.credentialReference, generation).readBytes(),
                        aad(record, generation),
                    )
                }.getOrNull()
            if (bytes != null) return bytes
        }
        return null
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
        cipher.init(Cipher.ENCRYPT_MODE, key())
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
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, nonce))
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

    private fun key(): SecretKey {
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

    private fun readRecords(): List<RememberedPeerRecord> {
        val file = metadataFile()
        if (!file.exists()) return emptyList()
        return try {
            val values = JSONArray(metadataFile().readText())
            (0 until values.length()).map {
                RememberedPeerRecord.fromJson(values.getJSONObject(it))
            }
        } catch (_: Throwable) {
            file.delete()
            emptyList()
        }
    }

    private fun writeRecords(records: List<RememberedPeerRecord>) {
        val values = JSONArray()
        records.forEach { values.put(it.toJson()) }
        atomicWrite(metadataFile(), values.toString().toByteArray(Charsets.UTF_8))
    }

    private fun reclaimOrphans(liveReferences: Set<String>) {
        credentialDirectory().listFiles()?.forEach { file ->
            val reference = file.name.substringBeforeLast('-', "")
            if (reference !in liveReferences) file.delete()
        }
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
