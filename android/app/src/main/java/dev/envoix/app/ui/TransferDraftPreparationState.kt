package dev.envoix.app.ui

import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import dev.envoix.app.Native
import dev.envoix.app.PreparedManifestV2Source
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject
import java.io.File

/**
 * Rotation-stable, room-scoped ownership for an unstarted Manifest v2 job.
 * It is deliberately memory-only and never searches the global job store.
 */
internal class TransferDraftPreparationState(
    initialRole: String = "send",
    showQrInitially: Boolean = false,
    private val onDiscard: (TransferDraftPreparationState) -> Unit = { it.launchCleanup() },
) {
    val preparedSources = mutableStateListOf<PreparedManifestV2Source>()
    val preparedJobId = mutableStateOf<String?>(null)
    val jobStoreDirectory = mutableStateOf<String?>(null)
    val stagingRootDirectory = mutableStateOf<String?>(null)
    val summary = mutableStateOf<JSONObject?>(null)
    val preparingCount = mutableStateOf(0)
    val error = mutableStateOf<String?>(null)
    val sourceAwaitingReauthorization = mutableStateOf<PreparedManifestV2Source?>(null)
    val startSubmitted = mutableStateOf(false)
    val role = mutableStateOf(initialRole)
    val typedCode = mutableStateOf("")
    val generatedInvite = mutableStateOf<Pair<String, String>?>(null)
    val generatedInviteRole = mutableStateOf<String?>(null)
    val scannedBroker = mutableStateOf<String?>(null)
    val scannedRelay = mutableStateOf<String?>(null)
    val roleChangeNotice = mutableStateOf<String?>(null)
    val topMode = mutableStateOf(if (showQrInitially) "show" else "closed")
    val rendezvousBusy = mutableStateOf(false)
    val rendezvousError = mutableStateOf<String?>(null)
    val initialPairingInputApplied = mutableStateOf(false)
    val mutex = Mutex()

    private val cleanupScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var disposition = DraftDisposition.Active

    /**
     * Atomically moves staged data to TransferService ownership exactly once.
     * A duplicate delivery completion or a discarded draft cannot start again.
     */
    @Synchronized
    fun transferOwnership(): Boolean =
        when (disposition) {
            DraftDisposition.Active -> {
                disposition = DraftDisposition.Transferred
                true
            }
            DraftDisposition.Transferred -> false
            DraftDisposition.Discarded -> false
        }

    @Synchronized
    fun ownershipWasTransferred(): Boolean = disposition == DraftDisposition.Transferred

    /**
     * Returns a claimed receive draft to editable state after the exact
     * receiver attempt has been canceled because its room Accept failed.
     */
    @Synchronized
    fun rollbackTransferredOwnership(): Boolean =
        if (disposition == DraftDisposition.Transferred) {
            disposition = DraftDisposition.Active
            true
        } else {
            false
        }

    /** Prevents queued picker work from recreating staging after disposition. */
    @Synchronized
    fun acceptsPreparationChanges(): Boolean = disposition == DraftDisposition.Active

    /** Schedules exact, serialized cleanup once. Started drafts are retained. */
    fun discard(): Boolean {
        val accepted =
            synchronized(this) {
                if (disposition != DraftDisposition.Active) {
                    false
                } else {
                    disposition = DraftDisposition.Discarded
                    true
                }
            }
        if (accepted) onDiscard(this)
        return accepted
    }

    private fun launchCleanup() {
        cleanupScope.launch {
            mutex.withLock {
                val jobId = preparedJobId.value
                val store = jobStoreDirectory.value
                if (!jobId.isNullOrBlank() && !store.isNullOrBlank()) {
                    runCatching { Native.cancelManifestV2Job(store, jobId) }
                }
                val stagingRoot = stagingRootDirectory.value?.let(::File)
                if (jobId != null &&
                    stagingRoot?.name == jobId &&
                    stagingRoot.parentFile?.name == "source-staging"
                ) {
                    stagingRoot.deleteRecursively()
                }
            }
        }
    }

    private enum class DraftDisposition {
        Active,
        Transferred,
        Discarded,
    }
}
