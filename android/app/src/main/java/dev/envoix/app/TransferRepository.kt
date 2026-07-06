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
    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    private var nextId = 1L

    /** Allocate an id + seed a Connecting card; returns the id. */
    @Synchronized
    fun create(direction: Direction, room: String): Long {
        val id = nextId++
        _transfers.value = _transfers.value + Transfer(id = id, direction = direction, room = room)
        return id
    }

    @Synchronized
    fun update(id: Long, transform: (Transfer) -> Transfer) {
        _transfers.value = _transfers.value.map { if (it.id == id) transform(it) else it }
    }

    @Synchronized
    fun remove(id: Long) {
        _transfers.value = _transfers.value.filterNot { it.id == id }
    }

    /** Ids of transfers still in flight (drive the foreground notification). */
    fun activeCount(): Int =
        _transfers.value.count { !it.status.isTerminal }
}

/** Deployed Envoix broker + relay defaults (overridable in Settings later). */
object Endpoints {
    const val BROKER =
        "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    const val RELAY = "https://envoix.chkxwlyh.us:8444"
}
