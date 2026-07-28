package dev.envoix.app.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.envoix.app.RememberedPeerStore
import dev.envoix.app.RememberedPeerSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal data class RememberedRoomsUiState(
    val peers: List<RememberedPeerSummary> = emptyList(),
    val connections: Map<String, RememberedRoomConnectionState> = emptyMap(),
    val transfers: Map<String, RememberedRoomTransferState> = emptyMap(),
    val loading: Boolean = true,
    val error: String? = null,
)

internal class RememberedRoomsViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private val store = RememberedPeerStore.get(application)
    private val connectionManager = RememberedRoomConnectionManager.get(application)
    private val transferCoordinator = RememberedRoomTransferCoordinator.get(application)
    private val mutableUiState = MutableStateFlow(RememberedRoomsUiState())
    val uiState: StateFlow<RememberedRoomsUiState> = mutableUiState.asStateFlow()

    init {
        viewModelScope.launch {
            store.changes.collectLatest {
                reloadPeers()
            }
        }
        viewModelScope.launch {
            connectionManager.states.collect { connections ->
                mutableUiState.update { it.copy(connections = connections) }
            }
        }
        viewModelScope.launch {
            transferCoordinator.states.collect { transfers ->
                mutableUiState.update { it.copy(transfers = transfers) }
            }
        }
    }

    fun rename(
        relationshipId: String,
        label: String,
        onSuccess: () -> Unit,
    ) {
        viewModelScope.launch {
            val renamed =
                withContext(Dispatchers.IO) {
                    runCatching { store.rename(relationshipId, label) }
                }
            if (renamed.getOrDefault(false)) {
                mutableUiState.update { it.copy(error = null) }
                onSuccess()
            } else {
                mutableUiState.update {
                    it.copy(error = renamed.exceptionOrNull()?.message ?: "The room could not be renamed.")
                }
            }
        }
    }

    fun forget(
        relationshipId: String,
        onSuccess: () -> Unit,
    ) {
        viewModelScope.launch {
            val cleaned =
                runCatching {
                    transferCoordinator.removeAllForRelationship(relationshipId)
                }
            if (cleaned.isFailure) {
                mutableUiState.update {
                    it.copy(
                        error =
                            cleaned.exceptionOrNull()?.message
                                ?: "Local room files could not be removed.",
                    )
                }
                return@launch
            }
            val deleted =
                try {
                    runCatching {
                        connectionManager.forgetRelationship(relationshipId)
                    }
                } finally {
                    withContext(NonCancellable) {
                        transferCoordinator.completeRelationshipForget(relationshipId)
                    }
                }
            if (deleted.isSuccess) {
                mutableUiState.update { it.copy(error = null) }
                onSuccess()
            } else {
                mutableUiState.update {
                    it.copy(error = deleted.exceptionOrNull()?.message ?: "The room could not be forgotten.")
                }
            }
        }
    }

    fun retry(relationshipId: String) {
        mutableUiState.update { it.copy(error = null) }
        connectionManager.retry(relationshipId)
    }

    fun enqueuePrepared(
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
        completion: (String?) -> Unit,
    ) {
        viewModelScope.launch {
            completion(
                transferCoordinator.enqueuePrepared(
                    relationshipId,
                    jobId,
                    rootNames,
                    itemCount,
                    directoryCount,
                    totalBytes,
                ),
            )
        }
    }

    fun retryOutbox(id: String) = transferCoordinator.retryOutbox(id)

    fun removeOutbox(id: String) = transferCoordinator.removeOutbox(id)

    fun acceptIncoming(relationshipId: String) = transferCoordinator.acceptIncoming(relationshipId)

    fun rejectIncoming(relationshipId: String) = transferCoordinator.rejectIncoming(relationshipId)

    fun clearTransferError(relationshipId: String) = transferCoordinator.clearError(relationshipId)

    fun clearError() {
        mutableUiState.update { it.copy(error = null) }
    }

    private suspend fun reloadPeers() {
        val loaded =
            withContext(Dispatchers.IO) {
                runCatching { store.peers() }
            }
        mutableUiState.update { state ->
            loaded.fold(
                onSuccess = { peers ->
                    state.copy(
                        peers = peers,
                        loading = false,
                        error = null,
                    )
                },
                onFailure = { error ->
                    state.copy(
                        loading = false,
                        error = error.message ?: "Saved rooms could not be loaded.",
                    )
                },
            )
        }
    }
}
