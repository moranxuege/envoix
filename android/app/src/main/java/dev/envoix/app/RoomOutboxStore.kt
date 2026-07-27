package dev.envoix.app

import android.content.Context
import android.util.AtomicFile
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

internal enum class RoomOutboxState(
    val wire: String,
) {
    Preparing("preparing"),
    Queued("queued"),
    Offering("offering"),
    Transferring("transferring"),
    NeedsAttention("needs_attention"),
    ;

    companion object {
        fun fromWire(value: String): RoomOutboxState =
            entries.firstOrNull { it.wire == value }
                ?: throw IllegalStateException("Unknown room outbox state")
    }
}

/**
 * Durable ownership record for one already-prepared Manifest-v2 sender job.
 *
 * The canonical Rust job remains the source of truth for files and inventory.
 * This record only binds that job to a remembered relationship and records
 * whether an automatic offer may safely be attempted.
 */
internal data class RoomOutboxEntry(
    val id: String,
    val relationshipId: String,
    val jobId: String,
    val rootNames: List<String>,
    val itemCount: Int,
    val directoryCount: Int,
    val totalBytes: Long,
    val state: RoomOutboxState,
    val offerId: String?,
    val transferId: Long?,
    val lastError: String?,
    val createdAtEpochMs: Long,
    val updatedAtEpochMs: Long,
)

internal class RoomOutboxStore private constructor(
    private val file: File,
    private val clockEpochMs: () -> Long,
) {
    private val mutableChanges =
        MutableSharedFlow<Unit>(
            replay = 1,
            onBufferOverflow = BufferOverflow.DROP_OLDEST,
        ).apply { tryEmit(Unit) }
    val changes: SharedFlow<Unit> = mutableChanges.asSharedFlow()

    @Synchronized
    fun entries(relationshipId: String? = null): List<RoomOutboxEntry> =
        readEntries()
            .asSequence()
            .filter { relationshipId == null || it.relationshipId == relationshipId }
            .sortedWith(compareBy(RoomOutboxEntry::createdAtEpochMs, RoomOutboxEntry::id))
            .toList()

    @Synchronized
    fun enqueue(
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
    ): RoomOutboxEntry =
        create(
            relationshipId,
            jobId,
            rootNames,
            itemCount,
            directoryCount,
            totalBytes,
            RoomOutboxState.Queued,
        )

    @Synchronized
    fun reserveForSeal(
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
    ): RoomOutboxEntry =
        create(
            relationshipId,
            jobId,
            rootNames,
            itemCount,
            directoryCount,
            totalBytes,
            RoomOutboxState.Preparing,
        )

    private fun create(
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
        initialState: RoomOutboxState,
    ): RoomOutboxEntry {
        require(relationshipId.isNotBlank() && relationshipId.length <= MAX_RELATIONSHIP_ID_BYTES)
        require(jobId.length == 32 && jobId.all(Char::isRoomOutboxHexDigit))
        require(itemCount >= 0)
        require(directoryCount in 0..itemCount)
        require(totalBytes >= 0)
        val current = readEntries().toMutableList()
        current.firstOrNull { it.jobId == jobId }?.let { existing ->
            check(existing.relationshipId == relationshipId) {
                "This prepared transfer already belongs to another room"
            }
            return existing
        }
        check(current.size < MAX_GLOBAL_ENTRIES) { "Too many queued room transfers" }
        check(current.count { it.relationshipId == relationshipId } < MAX_RELATIONSHIP_ENTRIES) {
            "Too many transfers are queued for this room"
        }
        val now = clockEpochMs()
        val entry =
            RoomOutboxEntry(
                id = UUID.randomUUID().toString(),
                relationshipId = relationshipId,
                jobId = jobId,
                rootNames =
                    rootNames
                        .asSequence()
                        .map(String::trim)
                        .filter(String::isNotEmpty)
                        .map { it.take(MAX_ROOT_NAME_CHARS) }
                        .take(MAX_ROOT_NAMES)
                        .toList(),
                itemCount = itemCount,
                directoryCount = directoryCount,
                totalBytes = totalBytes,
                state = initialState,
                offerId = null,
                transferId = null,
                lastError = null,
                createdAtEpochMs = now,
                updatedAtEpochMs = now,
            )
        current += entry
        persist(current)
        return entry
    }

    @Synchronized
    fun confirmSealed(id: String): Boolean =
        update(id) { current ->
            if (current.state != RoomOutboxState.Preparing) {
                null
            } else {
                current.copy(
                    state = RoomOutboxState.Queued,
                    updatedAtEpochMs = clockEpochMs(),
                )
            }
        }

    /**
     * Atomically claims the oldest queued job. Only one dispatcher can turn a
     * given job into an outgoing room-control offer.
     */
    @Synchronized
    fun claimNext(relationshipId: String): RoomOutboxEntry? {
        val current = readEntries().toMutableList()
        val index =
            current.indices.firstOrNull { index ->
                val value = current[index]
                value.relationshipId == relationshipId &&
                    value.state == RoomOutboxState.Queued
            } ?: return null
        val claimed =
            current[index].copy(
                state = RoomOutboxState.Offering,
                offerId = UUID.randomUUID().toString(),
                transferId = null,
                lastError = null,
                updatedAtEpochMs = clockEpochMs(),
            )
        current[index] = claimed
        persist(current)
        return claimed
    }

    @Synchronized
    fun markTransferring(
        id: String,
        offerId: String,
        transferId: Long,
    ): Boolean =
        update(id) { current ->
            if (current.state != RoomOutboxState.Offering ||
                current.offerId != offerId ||
                transferId < 0
            ) {
                null
            } else {
                current.copy(
                    state = RoomOutboxState.Transferring,
                    transferId = transferId,
                    lastError = null,
                    updatedAtEpochMs = clockEpochMs(),
                )
            }
        }

    @Synchronized
    fun requeue(
        id: String,
        offerId: String,
        message: String? = null,
    ): Boolean =
        update(id) { current ->
            if (current.state != RoomOutboxState.Offering ||
                current.offerId != offerId
            ) {
                null
            } else {
                current.copy(
                    state = RoomOutboxState.Queued,
                    offerId = null,
                    transferId = null,
                    lastError = message?.take(MAX_ERROR_CHARS),
                    updatedAtEpochMs = clockEpochMs(),
                )
            }
        }

    @Synchronized
    fun markNeedsAttention(
        id: String,
        message: String,
        expectedOfferId: String? = null,
        expectedTransferId: Long? = null,
    ): Boolean =
        update(id) { current ->
            val matches =
                when (current.state) {
                    RoomOutboxState.Offering ->
                        expectedOfferId != null &&
                            current.offerId == expectedOfferId &&
                            expectedTransferId == null
                    RoomOutboxState.Transferring ->
                        expectedTransferId != null &&
                            current.transferId == expectedTransferId
                    RoomOutboxState.Preparing,
                    RoomOutboxState.Queued,
                    RoomOutboxState.NeedsAttention,
                    ->
                        expectedOfferId == null && expectedTransferId == null
                }
            if (!matches) {
                null
            } else {
                current.copy(
                    state = RoomOutboxState.NeedsAttention,
                    lastError = message.take(MAX_ERROR_CHARS),
                    updatedAtEpochMs = clockEpochMs(),
                )
            }
        }

    @Synchronized
    fun retry(id: String): Boolean =
        update(id) { current ->
            if (current.state != RoomOutboxState.NeedsAttention) {
                null
            } else {
                current.copy(
                    state = RoomOutboxState.Queued,
                    offerId = null,
                    transferId = null,
                    lastError = null,
                    updatedAtEpochMs = clockEpochMs(),
                )
            }
        }

    @Synchronized
    fun remove(
        id: String,
        expectedOfferId: String? = null,
        expectedTransferId: Long? = null,
    ): RoomOutboxEntry? {
        val current = readEntries().toMutableList()
        val index = current.indexOfFirst { it.id == id }
        if (index < 0) return null
        val entry = current[index]
        val removable =
            when (entry.state) {
                RoomOutboxState.Offering ->
                    expectedOfferId != null &&
                        entry.offerId == expectedOfferId &&
                        expectedTransferId == null
                RoomOutboxState.Transferring ->
                    expectedTransferId != null &&
                        entry.transferId == expectedTransferId
                RoomOutboxState.Preparing,
                RoomOutboxState.Queued,
                RoomOutboxState.NeedsAttention,
                ->
                    expectedOfferId == null && expectedTransferId == null
            }
        if (!removable) return null
        val removed = current.removeAt(index)
        persist(current)
        return removed
    }

    /**
     * Atomically removes every inactive entry owned by one relationship.
     *
     * An active preparation, offer, or transfer blocks the whole operation;
     * callers never get a partially emptied room.
     */
    @Synchronized
    fun removeAllInactive(relationshipId: String): List<RoomOutboxEntry> {
        val current = readEntries().toMutableList()
        val owned = current.filter { it.relationshipId == relationshipId }
        check(
            owned.none {
                it.state == RoomOutboxState.Preparing ||
                    it.state == RoomOutboxState.Offering ||
                    it.state == RoomOutboxState.Transferring
            },
        ) {
            "This room still has an active queued transfer."
        }
        if (owned.isEmpty()) return emptyList()
        current.removeAll { it.relationshipId == relationshipId }
        persist(current)
        return owned
    }

    /**
     * A process death can occur after the peer accepted an offer. Replaying it
     * automatically could send the same job twice, so interrupted attempts
     * require an explicit retry.
     */
    @Synchronized
    fun reconcileInterruptedAttempts(): Int {
        val current = readEntries().toMutableList()
        var changed = 0
        val now = clockEpochMs()
        current.indices.forEach { index ->
            val entry = current[index]
            if (entry.state == RoomOutboxState.Preparing ||
                entry.state == RoomOutboxState.Offering ||
                entry.state == RoomOutboxState.Transferring
            ) {
                current[index] =
                    entry.copy(
                        state = RoomOutboxState.NeedsAttention,
                        lastError =
                            if (entry.state == RoomOutboxState.Preparing) {
                                "File queueing was interrupted. Review this transfer before retrying."
                            } else {
                                "The previous send was interrupted. Check the peer before retrying."
                            },
                        updatedAtEpochMs = now,
                    )
                changed += 1
            }
        }
        if (changed > 0) persist(current)
        return changed
    }

    private inline fun update(
        id: String,
        transform: (RoomOutboxEntry) -> RoomOutboxEntry?,
    ): Boolean {
        val current = readEntries().toMutableList()
        val index = current.indexOfFirst { it.id == id }
        if (index < 0) return false
        val replacement = transform(current[index]) ?: return false
        current[index] = replacement
        persist(current)
        return true
    }

    private fun readEntries(): List<RoomOutboxEntry> {
        if (!file.exists()) return emptyList()
        return try {
            val envelope = JSONObject(file.readText())
            check(envelope.getInt("version") == FORMAT_VERSION)
            val values = envelope.getJSONArray("entries")
            (0 until values.length()).map { values.getJSONObject(it).toEntry() }
        } catch (error: Throwable) {
            throw IllegalStateException(
                "Queued room transfers are temporarily unavailable; no records were changed",
                error,
            )
        }
    }

    private fun persist(entries: List<RoomOutboxEntry>) {
        val values = JSONArray()
        entries.forEach { values.put(it.toJson()) }
        val envelope =
            JSONObject()
                .put("version", FORMAT_VERSION)
                .put("entries", values)
        atomicWrite(envelope.toString().toByteArray(Charsets.UTF_8))
        mutableChanges.tryEmit(Unit)
    }

    private fun atomicWrite(bytes: ByteArray) {
        file.parentFile?.mkdirs()
        val atomic = AtomicFile(file)
        val output = atomic.startWrite()
        try {
            output.write(bytes)
            output.fd.sync()
            atomic.finishWrite(output)
        } catch (error: Throwable) {
            atomic.failWrite(output)
            throw error
        }
    }

    private fun RoomOutboxEntry.toJson(): JSONObject =
        JSONObject()
            .put("id", id)
            .put("relationship_id", relationshipId)
            .put("job_id", jobId)
            .put("root_names", JSONArray(rootNames))
            .put("item_count", itemCount)
            .put("directory_count", directoryCount)
            .put("total_bytes", totalBytes)
            .put("state", state.wire)
            .put("offer_id", offerId ?: JSONObject.NULL)
            .put("transfer_id", transferId ?: JSONObject.NULL)
            .put("last_error", lastError ?: JSONObject.NULL)
            .put("created_at_epoch_ms", createdAtEpochMs)
            .put("updated_at_epoch_ms", updatedAtEpochMs)

    private fun JSONObject.toEntry(): RoomOutboxEntry {
        val value =
            RoomOutboxEntry(
                id = getString("id"),
                relationshipId = getString("relationship_id"),
                jobId = getString("job_id"),
                rootNames =
                    getJSONArray("root_names").let { values ->
                        (0 until values.length()).map(values::getString)
                    },
                itemCount = getInt("item_count"),
                directoryCount = getInt("directory_count"),
                totalBytes = getLong("total_bytes"),
                state = RoomOutboxState.fromWire(getString("state")),
                offerId = optString("offer_id").takeIf(String::isNotBlank),
                transferId =
                    if (isNull("transfer_id")) {
                        null
                    } else {
                        getLong("transfer_id")
                    },
                lastError = optString("last_error").takeIf(String::isNotBlank),
                createdAtEpochMs = getLong("created_at_epoch_ms"),
                updatedAtEpochMs = getLong("updated_at_epoch_ms"),
            )
        require(value.id.isNotBlank())
        require(value.relationshipId.isNotBlank())
        require(value.jobId.length == 32 && value.jobId.all(Char::isRoomOutboxHexDigit))
        require(value.itemCount >= 0)
        require(value.directoryCount in 0..value.itemCount)
        require(value.totalBytes >= 0)
        return value
    }

    companion object {
        private const val FORMAT_VERSION = 1
        private const val MAX_GLOBAL_ENTRIES = 20
        private const val MAX_RELATIONSHIP_ENTRIES = 10
        private const val MAX_RELATIONSHIP_ID_BYTES = 128
        private const val MAX_ROOT_NAMES = 3
        private const val MAX_ROOT_NAME_CHARS = 255
        private const val MAX_ERROR_CHARS = 512

        @Volatile
        private var instance: RoomOutboxStore? = null

        fun get(context: Context): RoomOutboxStore =
            instance ?: synchronized(this) {
                instance
                    ?: RoomOutboxStore(
                        File(context.filesDir, "room-outbox/outbox-v1.json"),
                        System::currentTimeMillis,
                    ).also { store ->
                        instance = store
                    }
            }

        internal fun forTesting(
            file: File,
            clockEpochMs: () -> Long,
        ) = RoomOutboxStore(file, clockEpochMs)
    }
}

private fun Char.isRoomOutboxHexDigit(): Boolean = this in '0'..'9' || this in 'a'..'f'
