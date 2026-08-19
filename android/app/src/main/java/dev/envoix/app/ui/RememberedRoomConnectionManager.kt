package dev.envoix.app.ui

import android.content.Context
import android.os.SystemClock
import dev.envoix.app.LoadedRememberedPeer
import dev.envoix.app.RememberedPeerStore
import dev.envoix.app.SettingsStore
import dev.envoix.app.ffi.registerProtectedRememberedCredential
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max
import kotlin.math.min
import kotlin.random.Random

internal enum class RememberedRoomConnectionPhase {
    Offline,
    Waiting,
    Connecting,
    Connected,
    NeedsAttention,
}

internal data class RememberedRoomConnectionState(
    val relationshipId: String,
    val phase: RememberedRoomConnectionPhase = RememberedRoomConnectionPhase.Offline,
    val role: RememberedRoomConnectRole? = null,
    val peerName: String? = null,
    val error: String? = null,
)

internal data class RememberedRoomGatewayEvent(
    val relationshipId: String,
    val event: RoomControlEvent,
)

internal fun shouldKeepRememberedRoomLinks(
    foreground: Boolean,
    externalActivityLeases: Int,
): Boolean {
    require(externalActivityLeases >= 0) {
        "External activity lease count cannot be negative"
    }
    return foreground || externalActivityLeases > 0
}

/**
 * Process-scoped control links for remembered relationships.
 *
 * Connector/Responder are transient rendezvous roles. A successful link is
 * equal-member and never acquires persistent creator ownership. Broker starts
 * are globally bounded; adding saved peers cannot create an unbounded retry
 * storm.
 */
@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
internal class RememberedRoomConnectionManager private constructor(
    context: Context,
) {
    private val appContext = context.applicationContext
    private val store = RememberedPeerStore.get(appContext)
    private val scope =
        CoroutineScope(
            SupervisorJob() + Dispatchers.Default.limitedParallelism(1),
        )
    private val links = mutableMapOf<String, RoomLink>()
    private val queuedWorkRelationships = mutableSetOf<String>()
    private val mutableStates =
        MutableStateFlow<Map<String, RememberedRoomConnectionState>>(emptyMap())
    val states: StateFlow<Map<String, RememberedRoomConnectionState>> = mutableStates.asStateFlow()
    private val eventChannel = Channel<RememberedRoomGatewayEvent>(Channel.UNLIMITED)
    val events: Flow<RememberedRoomGatewayEvent> = eventChannel.receiveAsFlow()

    private val connectorSlots = Semaphore(MAX_CONNECTORS)
    private val responderSlots = Semaphore(MAX_RESPONDERS)
    private val schedulerLock = Mutex()
    private val activeAttempts = mutableSetOf<AttemptKey>()
    private val lastAttemptEndElapsedMs = mutableMapOf<AttemptKey, Long>()
    private var nextConnectorStartElapsedMs = 0L
    private var foreground = false
    private var externalActivityLeases = 0

    init {
        scope.launch {
            store.changes.collect {
                if (shouldRunLinks()) reconcile()
            }
        }
    }

    fun setForeground(value: Boolean) {
        scope.launch {
            if (foreground == value) return@launch
            val wasRunning = shouldRunLinks()
            foreground = value
            updateLinkRuntime(wasRunning)
        }
    }

    /**
     * File and folder pickers temporarily stop the Activity, but they are part
     * of the room workflow. Keep authenticated links alive until their result
     * callback releases this lease.
     */
    fun setExternalActivityActive(active: Boolean) {
        scope.launch {
            val wasRunning = shouldRunLinks()
            externalActivityLeases =
                if (active) {
                    externalActivityLeases + 1
                } else {
                    (externalActivityLeases - 1).coerceAtLeast(0)
                }
            updateLinkRuntime(wasRunning)
        }
    }

    private suspend fun updateLinkRuntime(wasRunning: Boolean) {
        val isRunning = shouldRunLinks()
        if (wasRunning == isRunning) return
        if (isRunning) {
            reconcile()
        } else {
            links.values.forEach { it.stop() }
        }
    }

    private fun shouldRunLinks(): Boolean =
        shouldKeepRememberedRoomLinks(
            foreground = foreground,
            externalActivityLeases = externalActivityLeases,
        )

    fun setRoomOpen(
        relationshipId: String,
        open: Boolean,
    ) {
        scope.launch {
            if (!shouldRunLinks()) return@launch
            reconcile()
            links[relationshipId]?.setRoomOpen(open)
        }
    }

    fun setQueuedWork(
        relationshipId: String,
        queued: Boolean,
    ) {
        scope.launch {
            if (queued) {
                queuedWorkRelationships += relationshipId
            } else {
                queuedWorkRelationships -= relationshipId
            }
            if (shouldRunLinks()) {
                reconcile()
                links[relationshipId]?.setQueuedWork(queued)
            }
        }
    }

    fun retry(relationshipId: String) {
        scope.launch {
            if (!shouldRunLinks()) return@launch
            reconcile()
            links[relationshipId]?.retry()
        }
    }

    /** mDNS/BLE presence may accelerate broker reconnect, but is never required. */
    fun accelerate(relationshipId: String) {
        scope.launch {
            if (shouldRunLinks()) links[relationshipId]?.accelerate()
        }
    }

    suspend fun offerTransfer(
        relationshipId: String,
        draft: RoomTransferOfferDraft,
    ) {
        runGatewayOperation(relationshipId) { it.offerTransfer(draft) }
    }

    suspend fun respondToOffer(
        relationshipId: String,
        offerId: String,
        accept: Boolean,
    ) {
        runGatewayOperation(relationshipId) {
            it.respondToOffer(offerId, accept)
        }
    }

    suspend fun updateTransferActive(
        relationshipId: String,
        active: Boolean,
    ) {
        runGatewayOperation(relationshipId) {
            it.updateTransferActive(active)
        }
    }

    /**
     * Stops the native control task before deleting its protected credential.
     *
     * The whole handoff runs on the manager's serial dispatcher, so reconcile
     * cannot restart the relationship between native shutdown and the store
     * deletion. If either step fails, the saved relationship is left intact
     * and normal foreground reconciliation resumes it.
     */
    suspend fun forgetRelationship(relationshipId: String) {
        val completion = CompletableDeferred<Result<Unit>>()
        val job =
            scope.launch {
                val hadQueuedWork = queuedWorkRelationships.remove(relationshipId)
                val link = links[relationshipId]
                val result =
                    runCatching {
                        link?.stopForDeletion()
                        store.delete(relationshipId)
                        links.remove(relationshipId)
                        publish(relationshipId, null)
                    }
                if (result.isFailure) {
                    if (hadQueuedWork) queuedWorkRelationships += relationshipId
                    if (shouldRunLinks()) {
                        reconcile()
                    }
                }
                completion.complete(result)
            }
        try {
            completion.await().getOrThrow()
        } catch (cancelled: CancellationException) {
            job.cancel()
            throw cancelled
        }
    }

    private suspend fun runGatewayOperation(
        relationshipId: String,
        operation: suspend (RoomLink) -> Unit,
    ) {
        val completion = CompletableDeferred<Result<Unit>>()
        val job =
            scope.launch {
                val result =
                    runCatching {
                        val link = links[relationshipId] ?: error("Remembered room is unavailable")
                        check(link.connected) { "Remembered room is not connected" }
                        operation(link)
                    }
                completion.complete(result)
            }
        try {
            completion.await().getOrThrow()
        } catch (cancelled: CancellationException) {
            job.cancel()
            throw cancelled
        }
    }

    private suspend fun reconcile() {
        val peers =
            runCatching { store.peers() }
                .getOrElse { return }
        val liveIds = peers.mapTo(mutableSetOf()) { it.relationshipId }
        links.keys
            .filterNot(liveIds::contains)
            .forEach { relationshipId ->
                links.remove(relationshipId)?.stop()
                queuedWorkRelationships.remove(relationshipId)
                publish(relationshipId, null)
            }
        peers.forEach { peer ->
            val link =
                links.getOrPut(peer.relationshipId) {
                    RoomLink(peer.relationshipId)
                }
            link.setQueuedWork(peer.relationshipId in queuedWorkRelationships)
            link.start()
        }
    }

    private fun publish(
        relationshipId: String,
        state: RememberedRoomConnectionState?,
    ) {
        mutableStates.value =
            mutableStates.value.toMutableMap().apply {
                if (state == null) remove(relationshipId) else put(relationshipId, state)
            }
    }

    private suspend fun acquireAttempt(
        key: AttemptKey,
        role: RememberedRoomConnectRole,
    ): AttemptLease? {
        val slots =
            when (role) {
                RememberedRoomConnectRole.Connector -> connectorSlots
                RememberedRoomConnectRole.Responder -> responderSlots
            }
        slots.acquire()
        var registered = false
        try {
            val startDelayMs =
                schedulerLock.withLock {
                    if (!activeAttempts.add(key)) return@withLock null
                    registered = true
                    val now = SystemClock.elapsedRealtime()
                    val sameLocatorEarliest =
                        lastAttemptEndElapsedMs[key]
                            ?.plus(SAME_LOCATOR_RECREATE_DELAY_MS)
                            ?: now
                    when (role) {
                        RememberedRoomConnectRole.Connector -> {
                            val earliest =
                                max(
                                    max(now, nextConnectorStartElapsedMs),
                                    sameLocatorEarliest,
                                )
                            val jitter =
                                Random.nextLong(
                                    CONNECTOR_JITTER_MIN_MS,
                                    CONNECTOR_JITTER_MAX_MS + 1,
                                )
                            val actualStart = earliest + jitter
                            nextConnectorStartElapsedMs =
                                actualStart + CONNECTOR_START_INTERVAL_MS
                            actualStart - now
                        }
                        RememberedRoomConnectRole.Responder -> {
                            val earliest = max(now, sameLocatorEarliest)
                            earliest - now +
                                Random.nextLong(
                                    RESPONDER_JITTER_MIN_MS,
                                    RESPONDER_JITTER_MAX_MS + 1,
                                )
                        }
                    }
                } ?: run {
                    slots.release()
                    return null
                }
            if (startDelayMs > 0) delay(startDelayMs)
            return AttemptLease(key, slots)
        } catch (cancelled: CancellationException) {
            if (registered) {
                schedulerLock.withLock { activeAttempts.remove(key) }
            }
            slots.release()
            throw cancelled
        } catch (error: Throwable) {
            if (registered) {
                schedulerLock.withLock { activeAttempts.remove(key) }
            }
            slots.release()
            throw error
        }
    }

    private suspend fun releaseAttempt(lease: AttemptLease?) {
        if (lease == null || !lease.released.compareAndSet(false, true)) return
        schedulerLock.withLock {
            activeAttempts.remove(lease.key)
            lastAttemptEndElapsedMs[lease.key] = SystemClock.elapsedRealtime()
        }
        lease.slots.release()
    }

    private inner class RoomLink(
        private val relationshipId: String,
    ) {
        private val gateway =
            NativeRoomControlGateway(
                appContext,
                rememberedRelationshipId = relationshipId,
            )
        private var desired = false
        private var roomOpen = false
        private var queuedWork = false
        private val preferConnector: Boolean
            get() = roomOpen || queuedWork
        private var nextRole = RememberedRoomConnectRole.Responder
        private var usePreviousGeneration = false
        private var failures = 0
        private var attemptGeneration: Long? = null
        private var attemptCredential: ByteArray? = null
        private var credentialReference: String? = null
        private var attemptJob: Job? = null
        private var timeoutJob: Job? = null
        private var retryJob: Job? = null
        private var lease: AttemptLease? = null
        private var relationshipLeaseHeld = false
        private var gatewayStarted = false
        var connected = false
            private set
        private var needsAttention = false
        private var switchRoleAfterClose: RememberedRoomConnectRole? = null
        private var timeoutMessage: String? = null
        private var selectedCollisionFlipAvailable = false
        private var stopCompletion: CompletableDeferred<Unit>? = null

        init {
            scope.launch {
                gateway.events.collect(::handle)
            }
        }

        fun start() {
            if (desired) return
            desired = true
            needsAttention = false
            nextRole =
                if (preferConnector) {
                    RememberedRoomConnectRole.Connector
                } else {
                    RememberedRoomConnectRole.Responder
                }
            scheduleAttempt(0L)
        }

        suspend fun stop() {
            beginStop(RoomCloseReason.Backgrounded)
            publish(
                relationshipId,
                RememberedRoomConnectionState(relationshipId),
            )
        }

        suspend fun stopForDeletion() {
            val terminal = beginStop(RoomCloseReason.UserEnded)
            val stopped =
                terminal == null ||
                    (
                        withTimeoutOrNull(DELETION_CLOSE_TIMEOUT_MS) {
                            terminal.await()
                            true
                        } ?: false
                    )
            if (!stopped) {
                // A rejected/delayed close still owns the protected
                // credential. Resume the saved relationship and let the
                // eventual terminal callback release its lease.
                if (!connected && !gatewayStarted) {
                    desired = false
                    start()
                } else {
                    desired = true
                }
                throw IllegalStateException(
                    "The room is still active. Wait for its transfer or connection to close, then try again.",
                )
            }
            publish(
                relationshipId,
                RememberedRoomConnectionState(relationshipId),
            )
        }

        private suspend fun beginStop(reason: RoomCloseReason): CompletableDeferred<Unit>? {
            desired = false
            cancelScheduledWork()
            switchRoleAfterClose = null
            timeoutMessage = null
            if (!gatewayStarted && !connected) {
                releaseCurrentLease()
                releaseRelationshipLease()
                return null
            }
            stopCompletion?.let { return it }
            val terminal = CompletableDeferred<Unit>()
            stopCompletion = terminal
            try {
                gateway.close(reason)
            } catch (error: Throwable) {
                if (stopCompletion === terminal) stopCompletion = null
                throw error
            }
            return terminal
        }

        suspend fun setRoomOpen(value: Boolean) {
            val previousPreference = preferConnector
            roomOpen = value
            reconcileDemand(previousPreference)
        }

        suspend fun setQueuedWork(value: Boolean) {
            val previousPreference = preferConnector
            queuedWork = value
            reconcileDemand(previousPreference)
        }

        private suspend fun reconcileDemand(previousPreference: Boolean) {
            if (preferConnector == previousPreference) return
            selectedCollisionFlipAvailable = preferConnector
            if (preferConnector) {
                usePreviousGeneration = false
            }
            if (!desired || needsAttention || connected) return
            val target =
                if (preferConnector) {
                    RememberedRoomConnectRole.Connector
                } else {
                    RememberedRoomConnectRole.Responder
                }
            switchTo(target)
        }

        suspend fun retry() {
            if (!desired) return
            needsAttention = false
            failures = 0
            usePreviousGeneration = false
            nextRole =
                if (preferConnector) {
                    RememberedRoomConnectRole.Connector
                } else {
                    RememberedRoomConnectRole.Responder
                }
            switchTo(nextRole)
        }

        suspend fun accelerate() {
            if (!desired || connected || needsAttention || gatewayStarted) return
            if (failures > 0) return
            retryJob?.cancel()
            retryJob = null
            if (preferConnector) nextRole = RememberedRoomConnectRole.Connector
            scheduleAttempt(Random.nextLong(ACCELERATION_JITTER_MIN_MS, ACCELERATION_JITTER_MAX_MS + 1))
        }

        suspend fun offerTransfer(draft: RoomTransferOfferDraft) {
            gateway.offerTransfer(draft)
        }

        suspend fun respondToOffer(
            offerId: String,
            accept: Boolean,
        ) {
            gateway.respondToOffer(offerId, accept)
        }

        suspend fun updateTransferActive(active: Boolean) {
            gateway.updateTransferActive(active)
        }

        private suspend fun switchTo(role: RememberedRoomConnectRole) {
            nextRole = role
            retryJob?.cancel()
            retryJob = null
            attemptJob?.cancel()
            attemptJob = null
            timeoutJob?.cancel()
            timeoutJob = null
            if (gatewayStarted) {
                if (role == RememberedRoomConnectRole.Connector) {
                    selectedCollisionFlipAvailable = false
                }
                switchRoleAfterClose = role
                runCatching { gateway.close(RoomCloseReason.UserEnded) }
            } else {
                releaseCurrentLease()
                releaseRelationshipLease()
                scheduleAttempt(roleSwitchJitter())
            }
        }

        private fun scheduleAttempt(delayMs: Long) {
            if (!desired || connected || needsAttention) return
            if (attemptJob?.isActive == true || retryJob?.isActive == true || gatewayStarted) return
            publish(
                relationshipId,
                RememberedRoomConnectionState(
                    relationshipId = relationshipId,
                    phase = RememberedRoomConnectionPhase.Waiting,
                    role = nextRole,
                ),
            )
            retryJob =
                scope.launch {
                    if (delayMs > 0) delay(delayMs)
                    retryJob = null
                    beginAttempt()
                }
        }

        private suspend fun beginAttempt() {
            if (!desired || connected || needsAttention || gatewayStarted) return
            val summary =
                runCatching {
                    store.peers().firstOrNull { it.relationshipId == relationshipId }
                }.getOrElse {
                    attention(it.message ?: "Remembered credential is unavailable")
                    return
                } ?: run {
                    attention("Remembered relationship is unavailable")
                    return
                }
            val generation =
                if (usePreviousGeneration) {
                    summary.previousGeneration ?: summary.generation
                } else {
                    summary.generation
                }
            val role = nextRole
            val key = AttemptKey(relationshipId, generation)
            attemptJob =
                scope.launch {
                    val acquired =
                        try {
                            acquireAttempt(key, role)
                        } catch (_: CancellationException) {
                            return@launch
                        } catch (error: Throwable) {
                            attemptJob = null
                            attention(error.message ?: "Room scheduler is unavailable")
                            return@launch
                        }
                    if (acquired == null) {
                        releaseRelationshipLease()
                        scheduleAttempt(DEDUPLICATION_RETRY_MS)
                        return@launch
                    }
                    lease = acquired
                    attemptJob = null
                    if (!desired || connected || needsAttention || role != nextRole) {
                        releaseCurrentLease()
                        scheduleAttempt(roleSwitchJitter())
                        return@launch
                    }
                    if (!store.acquireSession(relationshipId)) {
                        releaseCurrentLease()
                        scheduleAttempt(
                            RELATIONSHIP_BUSY_RETRY_MS +
                                Random.nextLong(RETRY_JITTER_MIN_MS, RETRY_JITTER_MAX_MS + 1),
                        )
                        return@launch
                    }
                    relationshipLeaseHeld = true
                    val loaded =
                        runCatching { store.load(relationshipId) }
                            .getOrElse {
                                attention(it.message ?: "Remembered credential is unavailable")
                                return@launch
                            } ?: run {
                            attention("Remembered relationship is unavailable")
                            return@launch
                        }
                    if (selectedGeneration(loaded) != generation) {
                        releaseCurrentLease()
                        releaseRelationshipLease()
                        scheduleAttempt(DEDUPLICATION_RETRY_MS)
                        return@launch
                    }
                    startGateway(loaded, generation, role)
                }
        }

        private suspend fun startGateway(
            loaded: LoadedRememberedPeer,
            generation: Long,
            role: RememberedRoomConnectRole,
        ) {
            val reference = credentialReference(loaded) ?: return
            attemptGeneration = generation
            attemptCredential = loaded.opaqueCredential
            gatewayStarted = true
            publish(
                relationshipId,
                RememberedRoomConnectionState(
                    relationshipId = relationshipId,
                    phase =
                        if (role == RememberedRoomConnectRole.Connector) {
                            RememberedRoomConnectionPhase.Connecting
                        } else {
                            RememberedRoomConnectionPhase.Waiting
                        },
                    role = role,
                ),
            )
            try {
                gateway.connectRemembered(
                    credentialReference = reference,
                    generation = generation,
                    displayName = SettingsStore.settings.value.nearbyDisplayName,
                    role = role,
                    broker = loaded.summary.broker,
                    relay = loaded.summary.relay,
                )
            } catch (error: Throwable) {
                gatewayStarted = false
                releaseCurrentLease()
                releaseRelationshipLease()
                schedulePreAuthenticationRetry(error.message ?: "Room connection failed")
                return
            }
            val watchdogMs =
                when {
                    role == RememberedRoomConnectRole.Connector -> CONNECTOR_WATCHDOG_MS
                    else -> RESPONDER_WATCHDOG_MS
                }
            timeoutJob?.cancel()
            timeoutJob =
                scope.launch {
                    delay(watchdogMs)
                    if (gatewayStarted && !connected && desired && !needsAttention) {
                        timeoutMessage = "Room connection timed out"
                        runCatching { gateway.close(RoomCloseReason.NetworkLost) }
                    }
                }
        }

        private suspend fun credentialReference(loaded: LoadedRememberedPeer): String? {
            credentialReference?.let { return it }
            val reference =
                runCatching {
                    registerProtectedRememberedCredential(loaded.opaqueCredential)
                }.getOrElse {
                    releaseCurrentLease()
                    releaseRelationshipLease()
                    attention(it.message ?: "Remembered credential could not be registered")
                    return null
                }
            return reference.takeIf(String::isNotBlank)?.also {
                credentialReference = it
            } ?: run {
                releaseCurrentLease()
                releaseRelationshipLease()
                attention("Remembered credential could not be registered")
                null
            }
        }

        private suspend fun handle(event: RoomControlEvent) {
            val shouldExpose =
                when (event) {
                    is RoomControlEvent.Connected -> handleConnected(event)
                    is RoomControlEvent.Failed -> {
                        handleFailed(event)
                        true
                    }
                    is RoomControlEvent.Closed -> {
                        handleClosed()
                        true
                    }
                    else -> true
                }
            if (shouldExpose) {
                eventChannel.trySend(
                    RememberedRoomGatewayEvent(
                        relationshipId = relationshipId,
                        event = event,
                    ),
                )
            }
        }

        private suspend fun handleConnected(event: RoomControlEvent.Connected): Boolean {
            timeoutJob?.cancel()
            timeoutJob = null
            gatewayStarted = false
            releaseCurrentLease()
            val generation = event.rememberedGeneration ?: attemptGeneration
            val credential = attemptCredential
            if (
                generation == null ||
                credential == null ||
                !store.advanceAfterPeerAuthentication(relationshipId, credential, generation)
            ) {
                attention("Authenticated room generation could not be saved")
                runCatching { gateway.close(RoomCloseReason.ProtocolFailure) }
                return false
            }
            connected = true
            failures = 0
            if (!desired) {
                // Shutdown may race the manager's consumption of Connected.
                // The native gateway already received close(); do not expose
                // a transient connected room while deletion/backgrounding is
                // waiting for the terminal callback.
                return false
            }
            publish(
                relationshipId,
                RememberedRoomConnectionState(
                    relationshipId = relationshipId,
                    phase = RememberedRoomConnectionPhase.Connected,
                    role = nextRole,
                    peerName = event.peerName,
                ),
            )
            return true
        }

        private suspend fun handleFailed(event: RoomControlEvent.Failed) {
            timeoutJob?.cancel()
            timeoutJob = null
            gatewayStarted = false
            connected = false
            releaseCurrentLease()
            if (event.peerAuthenticated) {
                val generation = event.attemptedRememberedGeneration ?: attemptGeneration
                val credential = attemptCredential
                if (
                    generation == null ||
                    credential == null ||
                    !store.advanceAfterPeerAuthentication(relationshipId, credential, generation)
                ) {
                    if (desired) {
                        attention("Authenticated room generation could not be saved")
                    } else {
                        releaseRelationshipLease()
                    }
                } else {
                    if (desired) {
                        attention(event.message)
                    } else {
                        releaseRelationshipLease()
                    }
                }
            } else {
                releaseRelationshipLease()
                if (desired) {
                    schedulePreAuthenticationRetry(
                        message = event.message,
                        failureCode = event.failureCode,
                        retryAfterSeconds = event.retryAfterSeconds,
                    )
                }
            }
            completeStop()
        }

        private suspend fun handleClosed() {
            timeoutJob?.cancel()
            timeoutJob = null
            gatewayStarted = false
            connected = false
            releaseCurrentLease()
            releaseRelationshipLease()
            completeStop()
            if (!desired) return
            switchRoleAfterClose?.let { role ->
                switchRoleAfterClose = null
                nextRole = role
                scheduleAttempt(roleSwitchJitter())
                return
            }
            val message = timeoutMessage
            timeoutMessage = null
            schedulePreAuthenticationRetry(message)
        }

        private fun selectedGeneration(loaded: LoadedRememberedPeer): Long =
            if (usePreviousGeneration) {
                loaded.summary.previousGeneration ?: loaded.summary.generation
            } else {
                loaded.summary.generation
            }

        private fun schedulePreAuthenticationRetry(
            message: String?,
            failureCode: RoomConnectFailureCode? = null,
            retryAfterSeconds: Long? = null,
        ) {
            if (!desired || needsAttention || connected || retryJob?.isActive == true) return
            failures += 1
            nextRole =
                if (preferConnector) {
                    if (Random.nextBoolean()) {
                        RememberedRoomConnectRole.Connector
                    } else {
                        RememberedRoomConnectRole.Responder
                    }
                } else {
                    RememberedRoomConnectRole.Responder
                }

            val loaded = runCatching { store.load(relationshipId) }.getOrNull()
            val summary = loaded?.summary
            usePreviousGeneration =
                summary != null &&
                attemptGeneration == summary.generation &&
                summary.previousGeneration != null

            val backoffCeiling =
                min(
                    MAX_BACKOFF_MS,
                    BASE_BACKOFF_MS * (1L shl min(failures - 1, MAX_BACKOFF_SHIFT)),
                )
            var delayMs =
                backoffCeiling +
                    Random.nextLong(RETRY_JITTER_MIN_MS, RETRY_JITTER_MAX_MS + 1)
            if (failureCode == RoomConnectFailureCode.RoomExpired) {
                delayMs =
                    ROOM_EXPIRED_COOLDOWN_MS +
                    Random.nextLong(RETRY_JITTER_MIN_MS, RETRY_JITTER_MAX_MS + 1)
            } else if (preferConnector && selectedCollisionFlipAvailable) {
                selectedCollisionFlipAvailable = false
                delayMs = roleSwitchJitter()
            }
            retryAfterSeconds?.takeIf { it > 0 }?.let { seconds ->
                delayMs =
                    max(
                        delayMs,
                        seconds.coerceAtMost(MAX_RETRY_AFTER_SECONDS) * 1_000L,
                    )
            }
            publish(
                relationshipId,
                RememberedRoomConnectionState(
                    relationshipId = relationshipId,
                    phase = RememberedRoomConnectionPhase.Waiting,
                    role = nextRole,
                    error = message,
                ),
            )
            scheduleAttempt(delayMs)
        }

        private suspend fun releaseCurrentLease() {
            val current = lease
            lease = null
            releaseAttempt(current)
        }

        private fun releaseRelationshipLease() {
            if (!relationshipLeaseHeld) return
            relationshipLeaseHeld = false
            store.releaseSession(relationshipId)
        }

        private fun completeStop() {
            stopCompletion?.complete(Unit)
            stopCompletion = null
        }

        private fun cancelScheduledWork() {
            attemptJob?.cancel()
            timeoutJob?.cancel()
            retryJob?.cancel()
            attemptJob = null
            timeoutJob = null
            retryJob = null
        }

        private fun roleSwitchJitter(): Long =
            Random.nextLong(
                ROLE_SWITCH_JITTER_MIN_MS,
                ROLE_SWITCH_JITTER_MAX_MS + 1,
            )

        private suspend fun attention(message: String) {
            cancelScheduledWork()
            releaseCurrentLease()
            releaseRelationshipLease()
            needsAttention = true
            connected = false
            gatewayStarted = false
            publish(
                relationshipId,
                RememberedRoomConnectionState(
                    relationshipId = relationshipId,
                    phase = RememberedRoomConnectionPhase.NeedsAttention,
                    error = message,
                ),
            )
        }
    }

    private data class AttemptKey(
        val relationshipId: String,
        val generation: Long,
    )

    private data class AttemptLease(
        val key: AttemptKey,
        val slots: Semaphore,
        val released: AtomicBoolean = AtomicBoolean(false),
    )

    companion object {
        private const val MAX_CONNECTORS = 1
        private const val MAX_RESPONDERS = 2
        private const val CONNECTOR_START_INTERVAL_MS = 6_000L
        private const val SAME_LOCATOR_RECREATE_DELAY_MS = 6_000L
        private const val CONNECTOR_JITTER_MIN_MS = 750L
        private const val CONNECTOR_JITTER_MAX_MS = 3_000L
        private const val RESPONDER_JITTER_MIN_MS = 500L
        private const val RESPONDER_JITTER_MAX_MS = 4_000L
        private const val ACCELERATION_JITTER_MIN_MS = 350L
        private const val ACCELERATION_JITTER_MAX_MS = 1_500L
        private const val ROLE_SWITCH_JITTER_MIN_MS = 1_000L
        private const val ROLE_SWITCH_JITTER_MAX_MS = 6_000L
        private const val DEDUPLICATION_RETRY_MS = 1_000L
        private const val RELATIONSHIP_BUSY_RETRY_MS = 5_000L
        private const val DELETION_CLOSE_TIMEOUT_MS = 15_000L
        private const val CONNECTOR_WATCHDOG_MS = 75_000L
        private const val RESPONDER_WATCHDOG_MS = 240_000L
        private const val ROOM_EXPIRED_COOLDOWN_MS = 300_000L
        private const val MAX_RETRY_AFTER_SECONDS = 300L
        private const val BASE_BACKOFF_MS = 30_000L
        private const val MAX_BACKOFF_MS = 300_000L
        private const val MAX_BACKOFF_SHIFT = 4
        private const val RETRY_JITTER_MIN_MS = 1_000L
        private const val RETRY_JITTER_MAX_MS = 15_000L

        @Volatile
        private var instance: RememberedRoomConnectionManager? = null

        fun get(context: Context): RememberedRoomConnectionManager =
            instance ?: synchronized(this) {
                instance ?: RememberedRoomConnectionManager(context).also { instance = it }
            }
    }
}
