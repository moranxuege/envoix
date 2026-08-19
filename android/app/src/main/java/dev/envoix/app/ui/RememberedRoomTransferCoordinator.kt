package dev.envoix.app.ui

import android.content.Context
import dev.envoix.app.Direction
import dev.envoix.app.InviteCodec
import dev.envoix.app.ManifestV2JobGateway
import dev.envoix.app.ManifestV2JobState
import dev.envoix.app.RememberedPeerStore
import dev.envoix.app.RoomOutboxEntry
import dev.envoix.app.RoomOutboxState
import dev.envoix.app.RoomOutboxStore
import dev.envoix.app.Status
import dev.envoix.app.Transfer
import dev.envoix.app.TransferActivityGroup
import dev.envoix.app.TransferRepository
import dev.envoix.app.TransferService
import dev.envoix.app.deleteManifestJobArtifacts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File

internal data class RememberedRoomTransferState(
    val outbox: List<RoomOutboxEntry> = emptyList(),
    val incomingOffer: RoomTransferOffer? = null,
    val incomingBusy: Boolean = false,
    val latestReceivedTransfer: Transfer? = null,
    val error: String? = null,
)

/**
 * Bridges durable room outbox ownership to the live remembered control link.
 *
 * It intentionally dispatches one outgoing job process-wide. Each send gets a
 * fresh one-time InviteV2; remembered credentials never enter the data-plane
 * transfer and cannot race the control-session generation.
 */
@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
internal class RememberedRoomTransferCoordinator private constructor(
    context: Context,
) {
    private val appContext = context.applicationContext
    private val manager = RememberedRoomConnectionManager.get(appContext)
    private val peers = RememberedPeerStore.get(appContext)
    private val outbox = RoomOutboxStore.get(appContext)
    private val scope =
        CoroutineScope(
            SupervisorJob() + Dispatchers.Default.limitedParallelism(1),
        )
    private val dispatchLock = Mutex()
    private val relationshipMutationLock = Mutex()
    private val forgettingRelationships = mutableSetOf<String>()
    private val mutableStates =
        MutableStateFlow<Map<String, RememberedRoomTransferState>>(emptyMap())
    val states: StateFlow<Map<String, RememberedRoomTransferState>> =
        mutableStates.asStateFlow()

    private var cachedOutbox = emptyList<RoomOutboxEntry>()
    private val pendingOutgoing = mutableMapOf<String, PendingOutgoing>()
    private val transferRelationships = mutableMapOf<Long, String>()
    private val incomingExpiryJobs = mutableMapOf<String, Job>()
    private var activeRelationships = emptySet<String>()

    init {
        scope.launch {
            runCatching { outbox.reconcileInterruptedAttempts() }
            reloadOutbox()
            launch {
                outbox.changes.collect {
                    reloadOutbox()
                }
            }
            launch {
                manager.states.collect {
                    dispatchIfPossible()
                }
            }
            launch {
                manager.events.collect(::handleGatewayEvent)
            }
            launch {
                TransferRepository.transfers.collect(::handleTransfers)
            }
        }
    }

    suspend fun enqueuePrepared(
        relationshipId: String,
        jobId: String,
        rootNames: List<String>,
        itemCount: Int,
        directoryCount: Int,
        totalBytes: Long,
    ): String? =
        relationshipMutationLock.withLock {
            if (relationshipId in forgettingRelationships) {
                return@withLock "This room is being forgotten."
            }
            withContext(Dispatchers.IO) {
                var sealed = false
                runCatching {
                    check(
                        peers.peers().any {
                            it.relationshipId == relationshipId
                        },
                    ) {
                        "This remembered room no longer exists."
                    }
                    val reserved =
                        outbox.reserveForSeal(
                            relationshipId = relationshipId,
                            jobId = jobId,
                            rootNames = rootNames,
                            itemCount = itemCount,
                            directoryCount = directoryCount,
                            totalBytes = totalBytes,
                        )
                    if (reserved.state != RoomOutboxState.Preparing) {
                        return@runCatching
                    }
                    sealAndValidate(reserved)
                    sealed = true
                    check(outbox.confirmSealed(reserved.id)) {
                        "The queued transfer changed before it was sealed"
                    }
                }.fold(
                    onSuccess = {
                        scope.launch { reloadOutbox() }
                        null
                    },
                    onFailure = { error ->
                        val reserved =
                            runCatching {
                                outbox.entries(relationshipId).firstOrNull { it.jobId == jobId }
                            }.getOrNull()
                        if (sealed) {
                            reserved?.let {
                                outbox.markNeedsAttention(
                                    it.id,
                                    "The files were sealed, but queue confirmation was interrupted.",
                                )
                            }
                            // The canonical job is immutable now. Keep ownership
                            // out of the editor and surface it through the outbox.
                            return@fold null
                        }
                        if (reserved?.state == RoomOutboxState.Preparing) {
                            outbox.remove(reserved.id)
                        }
                        error.message ?: "The prepared transfer could not be queued."
                    },
                )
            }
        }

    fun retryOutbox(id: String) {
        scope.launch {
            val retried =
                withContext(Dispatchers.IO) {
                    runCatching {
                        val entry =
                            outbox.entries().firstOrNull { it.id == id }
                                ?: error("The queued transfer no longer exists.")
                        check(entry.state == RoomOutboxState.NeedsAttention) {
                            "This queued transfer is not ready to retry."
                        }
                        // Sealing is idempotent. Recheck it here because a
                        // process may have died while a Preparing record was
                        // crossing the Rust sealing boundary.
                        sealAndValidate(entry)
                        check(outbox.retry(id)) {
                            "The queued transfer changed before retry."
                        }
                    }
                }
            retried.exceptionOrNull()?.let { error ->
                runCatching {
                    outbox.markNeedsAttention(
                        id,
                        error.message ?: "The prepared files could not be reopened.",
                    )
                }
            }
            reloadOutbox()
        }
    }

    fun removeOutbox(id: String) {
        scope.launch {
            val removed = runCatching { outbox.remove(id) }.getOrNull() ?: return@launch
            deleteManifestJobArtifacts(appContext.filesDir, removed.jobId)
            reloadOutbox()
        }
    }

    suspend fun removeAllForRelationship(relationshipId: String) {
        val completion = CompletableDeferred<Result<Unit>>()
        scope.launch {
            completion.complete(
                runCatching {
                    relationshipMutationLock.withLock {
                        check(forgettingRelationships.add(relationshipId)) {
                            "This room is already being forgotten."
                        }
                        try {
                            check(
                                pendingOutgoing.values.none {
                                    it.entry.relationshipId == relationshipId
                                },
                            ) {
                                "This room is still offering files."
                            }
                            val currentTransfers =
                                TransferRepository.transfers.value.associateBy { it.id }
                            val entries = outbox.entries(relationshipId)
                            entries.forEach { entry ->
                                val transfer =
                                    entry.transferId?.let(currentTransfers::get)
                                check(transfer == null || transfer.status.isTerminalState()) {
                                    "This room still has an active file transfer."
                                }
                            }
                            // Keep the durable rows until every app-owned job
                            // artifact is gone, so a storage failure remains
                            // retryable instead of orphaning hidden data.
                            entries.forEach(::deleteOwnedManifestArtifacts)
                            outbox.removeAllInactive(relationshipId)
                            Unit
                        } catch (error: Throwable) {
                            forgettingRelationships.remove(relationshipId)
                            throw error
                        }
                    }
                },
            )
        }
        try {
            completion.await().getOrThrow()
        } catch (cancelled: CancellationException) {
            val result =
                withContext(NonCancellable) {
                    completion.await()
                }
            if (result.isSuccess) {
                withContext(NonCancellable) {
                    completeRelationshipForget(relationshipId)
                }
            }
            throw cancelled
        }
    }

    suspend fun completeRelationshipForget(relationshipId: String) {
        relationshipMutationLock.withLock {
            forgettingRelationships.remove(relationshipId)
        }
    }

    fun clearError(relationshipId: String) {
        scope.launch {
            updateState(relationshipId) { it.copy(error = null) }
        }
    }

    fun acceptIncoming(relationshipId: String) {
        scope.launch {
            val offer = mutableStates.value[relationshipId]?.incomingOffer ?: return@launch
            if (mutableStates.value[relationshipId]?.incomingBusy == true) return@launch
            updateState(relationshipId) {
                it.copy(incomingBusy = true, error = null)
            }
            incomingExpiryJobs.remove(relationshipId)?.cancel()
            val peer =
                runCatching {
                    peers.peers().firstOrNull { it.relationshipId == relationshipId }
                }.getOrNull()
            val invitation =
                runCatching {
                    InviteCodec.parseForRole(offer.transferInvite, "receive")
                }.getOrNull()
            val reference = invitation?.reference
            if (peer == null ||
                invitation == null ||
                reference.isNullOrBlank() ||
                invitation.broker != peer.broker ||
                invitation.relay.orEmpty() != peer.relay
            ) {
                rejectUnusableIncoming(
                    relationshipId,
                    offer,
                    "This file offer does not belong to the connected room.",
                )
                return@launch
            }

            val receiveId =
                runCatching {
                    TransferService.startReceive(
                        appContext,
                        reference,
                        invitation.broker,
                        invitation.relay.orEmpty(),
                        qrPayload = null,
                        destinationCopyApproved = true,
                        rememberLabel = null,
                        rememberedRelationshipId = null,
                    )
                }.getOrElse { error ->
                    rejectUnusableIncoming(
                        relationshipId,
                        offer,
                        error.message ?: "The receiver could not start.",
                    )
                    return@launch
                }
            TransferRepository.assignActivityGroup(
                id = receiveId,
                groupId = TransferActivityGroup.remembered(relationshipId),
                groupLabel = peer.label,
            )
            val ready =
                withTimeoutOrNull(RECEIVER_START_TIMEOUT_MS) {
                    TransferRepository.transfers
                        .mapNotNull { values -> values.firstOrNull { it.id == receiveId } }
                        .first { it.status != Status.Connecting }
                }
            if (ready == null ||
                ready.status == Status.Failed ||
                ready.status == Status.Canceled
            ) {
                TransferService.cancel(appContext, receiveId)
                rejectUnusableIncoming(
                    relationshipId,
                    offer,
                    ready?.error ?: "The receiver did not become ready.",
                )
                return@launch
            }
            val accepted =
                runCatching {
                    manager.respondToOffer(relationshipId, offer.id, true)
                }
            if (accepted.isFailure) {
                TransferService.cancel(appContext, receiveId)
                updateState(relationshipId) {
                    it.copy(
                        incomingBusy = false,
                        error =
                            accepted.exceptionOrNull()?.message
                                ?: "The file offer could not be accepted.",
                    )
                }
                return@launch
            }
            transferRelationships[receiveId] = relationshipId
            publishLatestReceivedTransfers(TransferRepository.transfers.value)
            updateState(relationshipId) {
                it.copy(
                    incomingOffer = null,
                    incomingBusy = false,
                    error = null,
                )
            }
        }
    }

    private suspend fun rejectUnusableIncoming(
        relationshipId: String,
        offer: RoomTransferOffer,
        message: String,
    ) {
        val response =
            runCatching {
                manager.respondToOffer(relationshipId, offer.id, false)
            }
        updateState(relationshipId) { state ->
            if (state.incomingOffer?.id != offer.id) {
                state
            } else {
                state.copy(
                    incomingOffer = null,
                    incomingBusy = false,
                    error = response.exceptionOrNull()?.message ?: message,
                )
            }
        }
    }

    fun rejectIncoming(relationshipId: String) {
        scope.launch {
            val offer = mutableStates.value[relationshipId]?.incomingOffer ?: return@launch
            incomingExpiryJobs.remove(relationshipId)?.cancel()
            runCatching {
                manager.respondToOffer(relationshipId, offer.id, false)
            }.fold(
                onSuccess = {
                    updateState(relationshipId) {
                        it.copy(incomingOffer = null, incomingBusy = false, error = null)
                    }
                },
                onFailure = { error ->
                    updateState(relationshipId) {
                        it.copy(
                            incomingBusy = false,
                            error = error.message ?: "The file offer could not be declined.",
                        )
                    }
                },
            )
        }
    }

    private suspend fun reloadOutbox() {
        val loaded =
            runCatching { outbox.entries() }
                .getOrElse { error ->
                    mutableStates.value =
                        mutableStates.value.mapValues { (_, state) ->
                            state.copy(
                                error =
                                    error.message
                                        ?: "Queued room transfers are temporarily unavailable.",
                            )
                        }
                    return
                }
        cachedOutbox = loaded
        val grouped = loaded.groupBy(RoomOutboxEntry::relationshipId)
        val relationships = mutableStates.value.keys + grouped.keys
        mutableStates.value =
            relationships.associateWith { relationshipId ->
                (mutableStates.value[relationshipId] ?: RememberedRoomTransferState())
                    .copy(outbox = grouped[relationshipId].orEmpty())
            }
        val queuedRelationships =
            loaded
                .filter {
                    it.state == RoomOutboxState.Queued ||
                        it.state == RoomOutboxState.Offering
                }.mapTo(mutableSetOf(), RoomOutboxEntry::relationshipId)
        (relationships + queuedRelationships).forEach { relationshipId ->
            manager.setQueuedWork(
                relationshipId,
                relationshipId in queuedRelationships,
            )
        }
        dispatchIfPossible()
    }

    private suspend fun handleGatewayEvent(value: RememberedRoomGatewayEvent) {
        val relationshipId = value.relationshipId
        when (val event = value.event) {
            is RoomControlEvent.Connected -> dispatchIfPossible()
            is RoomControlEvent.IncomingOffer -> {
                val previous = mutableStates.value[relationshipId]?.incomingOffer
                if (previous != null && previous.id != event.offer.id) {
                    runCatching {
                        manager.respondToOffer(relationshipId, event.offer.id, false)
                    }
                    return
                }
                updateState(relationshipId) {
                    it.copy(
                        incomingOffer = event.offer,
                        incomingBusy = false,
                        error = null,
                    )
                }
                scheduleIncomingExpiry(relationshipId, event.offer.id)
            }
            is RoomControlEvent.OfferAccepted ->
                completeAcceptedOffer(relationshipId, event.offerId)
            is RoomControlEvent.OfferRejected ->
                finishRejectedOffer(
                    relationshipId,
                    event.offerId,
                    event.reason ?: "The other device declined this transfer.",
                )
            is RoomControlEvent.CommandFailed -> {
                if (event.command == "offer" && event.offerId != null) {
                    finishRejectedOffer(relationshipId, event.offerId, event.message)
                }
            }
            is RoomControlEvent.Closed -> {
                interruptPendingOffer(
                    relationshipId,
                    "The room disconnected before the peer answered.",
                )
                clearIncoming(relationshipId)
            }
            is RoomControlEvent.Failed -> {
                interruptPendingOffer(relationshipId, event.message)
                clearIncoming(relationshipId)
            }
            else -> Unit
        }
    }

    private suspend fun dispatchIfPossible() {
        dispatchLock.withLock {
            if (pendingOutgoing.isNotEmpty()) return
            if (cachedOutbox.any {
                    it.state == RoomOutboxState.Offering ||
                        it.state == RoomOutboxState.Transferring
                }
            ) {
                return
            }
            val connections = manager.states.value
            val candidate =
                cachedOutbox
                    .asSequence()
                    .filter { it.state == RoomOutboxState.Queued }
                    .firstOrNull {
                        connections[it.relationshipId]?.phase ==
                            RememberedRoomConnectionPhase.Connected
                    } ?: return
            val claimed = runCatching { outbox.claimNext(candidate.relationshipId) }.getOrNull() ?: return
            val offerId = claimed.offerId ?: return
            val peer =
                runCatching {
                    peers.peers().firstOrNull {
                        it.relationshipId == claimed.relationshipId
                    }
                }.getOrNull()
            val invitation =
                peer?.let {
                    InviteCodec.generate("send", it.broker, it.relay)
                }
            if (peer == null || invitation == null) {
                outbox.markNeedsAttention(
                    claimed.id,
                    "A fresh transfer invitation could not be created.",
                    expectedOfferId = offerId,
                )
                return
            }
            pendingOutgoing[offerId] =
                PendingOutgoing(
                    entry = claimed,
                    room = invitation.reference,
                    broker = invitation.broker,
                    relay = invitation.relay.orEmpty(),
                )
            val result =
                runCatching {
                    manager.offerTransfer(
                        claimed.relationshipId,
                        RoomTransferOfferDraft(
                            id = offerId,
                            transferInvite = invitation.payload,
                            rootNames = claimed.rootNames,
                            itemCount = claimed.itemCount,
                            directoryCount = claimed.directoryCount,
                            totalBytes = claimed.totalBytes,
                        ),
                    )
                }
            if (result.isFailure) {
                pendingOutgoing.remove(offerId)
                outbox.markNeedsAttention(
                    claimed.id,
                    result.exceptionOrNull()?.message
                        ?: "The file offer could not be delivered.",
                    expectedOfferId = offerId,
                )
            }
        }
    }

    private suspend fun completeAcceptedOffer(
        relationshipId: String,
        offerId: String,
    ) {
        val pending = pendingOutgoing.remove(offerId) ?: return
        if (pending.entry.relationshipId != relationshipId) return
        val transferId =
            runCatching {
                TransferService.startSend(
                    appContext,
                    pending.room,
                    pending.broker,
                    pending.relay,
                    pending.entry.jobId,
                    qrPayload = null,
                    rememberLabel = null,
                    rememberedRelationshipId = null,
                )
            }.getOrElse { error ->
                outbox.markNeedsAttention(
                    pending.entry.id,
                    error.message ?: "The accepted transfer could not start.",
                    expectedOfferId = offerId,
                )
                return
            }
        val currentLabel =
            runCatching {
                peers
                    .peers()
                    .firstOrNull { it.relationshipId == relationshipId }
                    ?.label
            }.getOrNull()
        TransferRepository.assignActivityGroup(
            id = transferId,
            groupId = TransferActivityGroup.remembered(relationshipId),
            groupLabel = currentLabel,
        )
        if (!outbox.markTransferring(pending.entry.id, offerId, transferId)) {
            TransferService.cancel(appContext, transferId)
            return
        }
        transferRelationships[transferId] = relationshipId
        reloadOutbox()
    }

    private suspend fun finishRejectedOffer(
        relationshipId: String,
        offerId: String,
        message: String,
    ) {
        val pending = pendingOutgoing.remove(offerId) ?: return
        if (pending.entry.relationshipId != relationshipId) return
        outbox.markNeedsAttention(
            pending.entry.id,
            message,
            expectedOfferId = offerId,
        )
        reloadOutbox()
    }

    private suspend fun interruptPendingOffer(
        relationshipId: String,
        message: String,
    ) {
        val pending =
            pendingOutgoing.values.firstOrNull {
                it.entry.relationshipId == relationshipId
            } ?: return
        val offerId = pending.entry.offerId ?: return
        pendingOutgoing.remove(offerId)
        outbox.markNeedsAttention(
            pending.entry.id,
            message,
            expectedOfferId = offerId,
        )
        reloadOutbox()
    }

    private suspend fun handleTransfers(transfers: List<dev.envoix.app.Transfer>) {
        val byId = transfers.associateBy { it.id }
        var changed = false
        cachedOutbox
            .filter { it.state == RoomOutboxState.Transferring }
            .forEach { entry ->
                val transferId = entry.transferId ?: return@forEach
                val transfer = byId[transferId] ?: return@forEach
                when (transfer.status) {
                    Status.Delivered -> {
                        if (outbox.remove(entry.id, expectedTransferId = transferId) != null) {
                            changed = true
                        }
                        transferRelationships.remove(transferId)
                    }
                    Status.Failed, Status.Canceled -> {
                        if (
                            outbox.markNeedsAttention(
                                entry.id,
                                transfer.error ?: "The transfer did not finish.",
                                expectedTransferId = transferId,
                            )
                        ) {
                            changed = true
                        }
                        transferRelationships.remove(transferId)
                    }
                    else -> Unit
                }
            }

        publishLatestReceivedTransfers(transfers)
        val nowActive =
            transferRelationships
                .filter { (id, _) ->
                    byId[id]?.let { transfer ->
                        transfer.direction == Direction.Send ||
                            transfer.direction == Direction.Receive
                    } == true &&
                        byId[id]?.status?.isTerminalState() == false
                }.values
                .toSet()
        (activeRelationships + nowActive).forEach { relationshipId ->
            val active = relationshipId in nowActive
            if (active != (relationshipId in activeRelationships)) {
                runCatching {
                    manager.updateTransferActive(relationshipId, active)
                }
            }
        }
        activeRelationships = nowActive
        if (changed) reloadOutbox()
    }

    private fun publishLatestReceivedTransfers(transfers: List<Transfer>) {
        val latestByRelationship =
            latestDeliveredReceivesByRelationship(
                transfers = transfers,
                relationshipByTransferId = transferRelationships,
            )
        val visibleRelationships =
            mutableStates.value
                .filterValues { it.latestReceivedTransfer != null }
                .keys
        (visibleRelationships + latestByRelationship.keys).forEach { relationshipId ->
            updateState(relationshipId) {
                it.copy(latestReceivedTransfer = latestByRelationship[relationshipId])
            }
        }
        val knownTransferIds = transfers.mapTo(mutableSetOf(), Transfer::id)
        transferRelationships.keys
            .filterNot(knownTransferIds::contains)
            .forEach(transferRelationships::remove)
    }

    private fun scheduleIncomingExpiry(
        relationshipId: String,
        offerId: String,
    ) {
        incomingExpiryJobs.remove(relationshipId)?.cancel()
        incomingExpiryJobs[relationshipId] =
            scope.launch {
                delay(INCOMING_OFFER_TIMEOUT_MS)
                val current = mutableStates.value[relationshipId]?.incomingOffer
                if (current?.id == offerId) {
                    runCatching {
                        manager.respondToOffer(relationshipId, offerId, false)
                    }
                    clearIncoming(relationshipId)
                }
            }
    }

    private suspend fun clearIncoming(relationshipId: String) {
        incomingExpiryJobs.remove(relationshipId)?.cancel()
        updateState(relationshipId) {
            it.copy(incomingOffer = null, incomingBusy = false)
        }
    }

    private fun updateState(
        relationshipId: String,
        transform: (RememberedRoomTransferState) -> RememberedRoomTransferState,
    ) {
        mutableStates.value =
            mutableStates.value.toMutableMap().apply {
                put(
                    relationshipId,
                    transform(get(relationshipId) ?: RememberedRoomTransferState()),
                )
            }
    }

    private suspend fun sealAndValidate(entry: RoomOutboxEntry) {
        val snapshot =
            ManifestV2JobGateway.shared.seal(
                TransferService.jobStoreDirectory(appContext).absolutePath,
                entry.jobId,
            )
        check(snapshot.jobId == entry.jobId)
        check(snapshot.state == ManifestV2JobState.Sealed)
        check(
            snapshot.inventory.fileCount +
                snapshot.inventory.directoryCount ==
                entry.itemCount,
        )
        check(snapshot.inventory.directoryCount == entry.directoryCount)
        check(snapshot.inventory.totalBytes == entry.totalBytes)
    }

    private fun deleteOwnedManifestArtifacts(entry: RoomOutboxEntry) {
        deleteManifestJobArtifacts(appContext.filesDir, entry.jobId)
        val direct =
            listOf(
                appContext.filesDir.resolve("manifest-v2/source-staging/${entry.jobId}"),
                appContext.filesDir.resolve("manifest-v2/jobs/.envoix-staging/${entry.jobId}"),
                appContext.filesDir.resolve("manifest-v2/jobs/job-${entry.jobId}.json"),
                appContext.filesDir.resolve("manifest-v2/jobs/.job-${entry.jobId}.tmp"),
            )
        val destinationReceipts =
            appContext.filesDir
                .resolve("manifest-v2/destination-save")
                .listFiles()
                .orEmpty()
                .filter {
                    it.isFile &&
                        it.name.startsWith("${entry.jobId}-") &&
                        (it.name.endsWith(".json") || it.name.endsWith(".json.tmp"))
                }
        check((direct + destinationReceipts).none(File::exists)) {
            "Local files for ${entry.rootNames.firstOrNull() ?: "a queued transfer"} could not be deleted."
        }
    }

    private data class PendingOutgoing(
        val entry: RoomOutboxEntry,
        val room: String,
        val broker: String,
        val relay: String,
    )

    companion object {
        private const val RECEIVER_START_TIMEOUT_MS = 10_000L
        private const val INCOMING_OFFER_TIMEOUT_MS = 60_000L

        @Volatile
        private var instance: RememberedRoomTransferCoordinator? = null

        fun get(context: Context): RememberedRoomTransferCoordinator =
            instance ?: synchronized(this) {
                instance
                    ?: RememberedRoomTransferCoordinator(context).also {
                        instance = it
                    }
            }
    }
}

internal fun latestDeliveredReceivesByRelationship(
    transfers: List<Transfer>,
    relationshipByTransferId: Map<Long, String>,
): Map<String, Transfer> =
    transfers
        .asSequence()
        .filter {
            it.direction == Direction.Receive &&
                it.status == Status.Delivered &&
                relationshipByTransferId.containsKey(it.id)
        }.groupBy { relationshipByTransferId.getValue(it.id) }
        .mapValues { (_, values) -> values.maxBy(Transfer::id) }

private fun Status.isTerminalState(): Boolean =
    this == Status.Delivered ||
        this == Status.Failed ||
        this == Status.Canceled
