package dev.envoix.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Single source of truth for transfer state: the [TransferService] mutates it,
 * the UI observes it. A process-wide singleton so both sides share one list
 * without binding the Service to the Activity.
 */
object TransferRepository {
    /** Max lines kept in a transfer's own log (events + routed core lines). */
    const val LOG_CAP = 200

    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    private var nextId = 1L

    /** Raise the id floor to clear every persisted record. Called at app start
     *  (before any card can be created): a new card allocated before the
     *  service restores old records must not reuse a persisted id - the new
     *  session would silently overwrite that record on its first persist. */
    @Synchronized
    fun seedNextId(min: Long) {
        nextId = maxOf(nextId, min)
    }

    /** Allocate an id + seed a Connecting card; returns the id. */
    @Synchronized
    fun create(
        direction: Direction,
        room: String,
        activityGroupId: String? = null,
        activityGroupLabel: String? = null,
    ): Long {
        val id = nextId++
        _transfers.value = _transfers.value +
            Transfer(
                id = id,
                direction = direction,
                room = room,
                activityGroupId = activityGroupId,
                activityGroupLabel = activityGroupLabel,
            )
        return id
    }

    /** Seed a card with a FIXED id (restoring a persisted record); keeps new
     *  ids from colliding with restored ones. No-op if the id already exists. */
    @Synchronized
    fun restoreCard(
        id: Long,
        direction: Direction,
        room: String,
        qrPayload: String? = null,
        savedUri: String? = null,
        savedName: String? = null,
        savedDestinationLabel: String? = null,
        activityGroupId: String? = null,
        activityGroupLabel: String? = null,
    ): Boolean {
        if (_transfers.value.any { it.id == id }) return false
        nextId = maxOf(nextId, id + 1)
        _transfers.value = _transfers.value +
            Transfer(
                id = id,
                direction = direction,
                room = room,
                qrPayload = qrPayload,
                savedUri = savedUri,
                savedName = savedName,
                savedDestinationLabel = savedDestinationLabel,
                activityGroupId = activityGroupId,
                activityGroupLabel = activityGroupLabel,
            )
        return true
    }

    /**
     * Assign a stable Activity identity without changing the transport room.
     * A higher-level room owner may explicitly replace a provisional identity.
     */
    @Synchronized
    fun assignActivityGroup(
        id: Long,
        groupId: String,
        groupLabel: String?,
        replaceExisting: Boolean = false,
    ): Boolean {
        val normalizedId = groupId.trim()
        require(normalizedId.isNotEmpty()) { "Activity group id is required" }
        val normalizedLabel = groupLabel?.trim()?.takeIf(String::isNotEmpty)
        var matched = false
        _transfers.value =
            _transfers.value.map { transfer ->
                if (transfer.id != id) {
                    transfer
                } else {
                    matched = true
                    transfer.withActivityGroup(normalizedId, normalizedLabel, replaceExisting)
                }
            }
        return matched
    }

    /**
     * Assign by the opaque room reference while the caller still owns that
     * reference. This is used only to bridge synchronous transfer creation to
     * the higher-level room workflow.
     */
    @Synchronized
    fun assignActivityGroupByRoom(
        roomReference: String,
        groupId: String,
        groupLabel: String?,
        replaceExisting: Boolean = false,
    ): Int {
        require(roomReference.isNotBlank()) { "Room reference is required" }
        val normalizedId = groupId.trim()
        require(normalizedId.isNotEmpty()) { "Activity group id is required" }
        val normalizedLabel = groupLabel?.trim()?.takeIf(String::isNotEmpty)
        var matches = 0
        _transfers.value =
            _transfers.value.map { transfer ->
                if (transfer.room != roomReference) {
                    transfer
                } else {
                    matches += 1
                    transfer.withActivityGroup(normalizedId, normalizedLabel, replaceExisting)
                }
            }
        return matches
    }

    @Synchronized
    fun update(
        id: Long,
        transform: (Transfer) -> Transfer,
    ) {
        _transfers.value = _transfers.value.map { if (it.id == id) transform(it) else it }
    }

    /** Append an (already compacted) native-core log line to the newest transfer
     *  whose room matches [roomPrefix], so the core's per-transfer logs show up
     *  in that transfer's detail drawer. No-op if no transfer matches. */
    @Synchronized
    fun appendCoreLog(
        roomId: String,
        line: String,
    ): Long? {
        val id =
            _transfers.value
                .filter { Room(it.room).id == roomId }
                .maxByOrNull { it.id }
                ?.id ?: return null
        _transfers.value =
            _transfers.value.map {
                if (it.id == id) it.copy(log = (it.log + line).takeLast(LOG_CAP)) else it
            }
        return id
    }

    @Synchronized
    fun remove(id: Long) {
        _transfers.value = _transfers.value.filterNot { it.id == id }
    }

    /** Attempts that still own active native work and therefore require the
     * foreground service. Paused records remain durable without a worker. */
    fun activeCount(): Int = _transfers.value.count { !it.status.isTerminal && it.status != Status.Paused }

    private fun Transfer.withActivityGroup(
        groupId: String,
        groupLabel: String?,
        replaceExisting: Boolean,
    ): Transfer {
        if (!replaceExisting && activityGroupId != null && activityGroupId != groupId) return this
        return copy(
            activityGroupId = groupId,
            activityGroupLabel = groupLabel ?: activityGroupLabel,
        )
    }
}

/** Deployed Envoix broker + relay defaults (overridable in Settings later). */
object Endpoints {
    const val BROKER =
        "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    const val RELAY = "https://envoix.chkxwlyh.us:8444"

    /** Per-room log-collection + diagnostic log endpoint on the rdz box (TLS). */
    const val LOG_SERVER = "https://rdz.chkxwlyh.us:8460"
}
