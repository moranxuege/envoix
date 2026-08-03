package dev.envoix.app

enum class Direction(
    val wire: String,
) {
    Send("send"),
    Receive("receive"),
    ;

    companion object {
        fun fromWire(wire: String): Direction? = entries.firstOrNull { it.wire == wire }
    }
}

/** Stable, secret-free milestones emitted by one native transfer attempt. */
enum class TransferStage(
    val wire: String,
    internal val order: Int,
) {
    SessionStarted("session_started", 0),
    ConnectionReady("connection_ready", 1),
    AuthenticationStarted("authentication_started", 2),
    AuthenticationComplete("authentication_complete", 3),
    ManifestOffer("manifest_offer", 4),
    ManifestAccepted("manifest_accepted", 5),
    FirstPayload("first_payload", 6),
    PayloadComplete("payload_complete", 7),
    DeliveryComplete("delivery_complete", 8),
    Canceled("canceled", 9),
    Failed("failed", 9),
    ;

    companion object {
        fun fromWire(wire: String): TransferStage? = entries.firstOrNull { it.wire == wire }
    }

    internal val isTerminal: Boolean
        get() = this == DeliveryComplete || this == Canceled || this == Failed
}

/** One structured monotonic timing sample from the Rust transfer engine. */
data class TransferStageTiming(
    val transferId: String?,
    val direction: Direction,
    val attemptId: Long,
    val stage: TransferStage,
    val elapsedUs: Long,
    val deltaUs: Long,
)

internal object TransferStageTimingParser {
    fun parse(
        stageWire: String?,
        directionWire: String?,
        attemptId: Long?,
        transferId: String?,
        elapsedUs: Long?,
        deltaUs: Long?,
    ): TransferStageTiming? {
        val stage = stageWire?.let(TransferStage::fromWire) ?: return null
        val direction = directionWire?.let(Direction::fromWire) ?: return null
        val checkedAttemptId = attemptId?.takeIf { it >= 0L } ?: return null
        val checkedElapsedUs = elapsedUs?.takeIf { it >= 0L } ?: return null
        val checkedDeltaUs =
            deltaUs
                ?.takeIf { it >= 0L && it <= checkedElapsedUs }
                ?: return null
        val checkedTransferId =
            transferId?.trim()?.takeIf(String::isNotEmpty)
                ?: if (transferId == null) null else return null
        return TransferStageTiming(
            transferId = checkedTransferId,
            direction = direction,
            attemptId = checkedAttemptId,
            stage = stage,
            elapsedUs = checkedElapsedUs,
            deltaUs = checkedDeltaUs,
        )
    }
}

internal data class TransferStageTimingAppendResult(
    val samples: List<TransferStageTiming>,
    val accepted: Boolean,
)

internal object TransferStageTimingHistory {
    const val SAMPLE_CAP = 64

    fun append(
        current: List<TransferStageTiming>,
        sample: TransferStageTiming,
        cap: Int = SAMPLE_CAP,
    ): TransferStageTimingAppendResult {
        require(cap > 0) { "Stage timing sample cap must be positive" }
        val previous =
            current.lastOrNull {
                it.attemptId == sample.attemptId && it.direction == sample.direction
            }
        if (previous != null && !sample.follows(previous)) {
            return TransferStageTimingAppendResult(current, accepted = false)
        }
        return TransferStageTimingAppendResult(
            samples = (current + sample).takeLast(cap),
            accepted = true,
        )
    }

    private fun TransferStageTiming.follows(previous: TransferStageTiming): Boolean {
        if (previous.stage.isTerminal) return false
        if (stage.order <= previous.stage.order || elapsedUs < previous.elapsedUs) return false
        if (deltaUs != elapsedUs - previous.elapsedUs) return false
        if (previous.transferId != null && transferId != previous.transferId) return false
        return true
    }
}

data class TransferInventoryEntry(
    val entryId: Int,
    val parentEntryId: Int?,
    val name: String,
    val directory: Boolean,
    val size: Long,
)

/** Native projection of canonical Manifest-v2 session phases. */
enum class Status(
    val wire: String,
) {
    Preparing("preparing"),
    WaitingForPeer("waiting_for_peer"),
    Pairing("pairing"),
    Connecting("connecting"),
    AwaitingDecision("awaiting_decision"),
    Transferring("transferring"),
    Verifying("verifying"),
    Saving("saving"),
    WaitingForReceiverSave("waiting_for_receiver_save"),
    FinalizingDelivery("finalizing_delivery"),
    Paused("paused"),
    Delivered("delivered"),
    Failed("failed"),
    Canceled("canceled"),
    ;

    companion object {
        /** Canonical phase wire string to native UI state. */
        fun fromWire(wire: String): Status? = entries.firstOrNull { it.wire == wire }
    }
}

enum class RecoveryAction(
    val wire: String,
) {
    Retry("retry"),
    Resume("resume"),
    ChooseFolder("choose_folder"),
    OpenSettings("open_settings"),
    RePair("re_pair"),
    None("none"),
    ;

    companion object {
        fun fromWire(wire: String): RecoveryAction = entries.firstOrNull { it.wire == wire } ?: None
    }
}

/** One transfer's observable state, shown as a card. */
data class Transfer(
    val id: Long,
    val direction: Direction,
    /** Process-private rendezvous handle used by the transport. Never use it as
     * a user-visible room identity. */
    val room: String,
    /** Stable presentation identity for grouping transfers in Activity. */
    val activityGroupId: String? = null,
    /** User-visible room/device label captured independently from [room]. */
    val activityGroupLabel: String? = null,
    val fileName: String? = null,
    /** Stable canonical Manifest-v2 job identity. */
    val jobId: String? = null,
    val rootCount: Int = 0,
    val fileCount: Int = 0,
    val directoryCount: Int = 0,
    val exceptionalOffer: Boolean = false,
    /** Bounded authenticated offer projection; the native ledger retains the
     * complete inventory and exposes additional pages on demand. */
    val inventoryPreview: List<TransferInventoryEntry> = emptyList(),
    val inventoryHasMore: Boolean = false,
    /** Local attempt number (resume bumps it). */
    val attempt: Int = 1,
    val pathAddr: String? = null,
    val bytes: Long = 0,
    val total: Long = 0,
    /** Instantaneous throughput (bytes/s) of the last interval. */
    val speedBps: Double = 0.0,
    /** True average throughput (total bytes / elapsed), matching the CLI's avg_bps. */
    val avgBps: Double = 0.0,
    val status: Status = Status.Connecting,
    val error: String? = null,
    /** Where a received file ended up (a `content://` in Downloads), for opening. */
    val savedUri: String? = null,
    val savedUris: List<String> = emptyList(),
    /** Human-readable destination captured from the exact save operation.
     * Null identifies legacy/incomplete records whose UI must use a fallback. */
    val savedDestinationLabel: String? = null,
    /** The name the received file was actually saved under — may differ from
     *  [fileName] (a non-unique source/display name) after a collision bump, e.g. "photo (1).jpg". */
    val savedName: String? = null,
    /** Stable machine cause; never reconstructed from [error]. */
    val failureCause: String? = null,
    val retryable: Boolean = false,
    val recoveryAction: RecoveryAction = RecoveryAction.None,
    /** For an initiated session, the invite payload to show as a QR while waiting
     *  for a peer to pair (null when we joined someone else's code). */
    val qrPayload: String? = null,
    /** Recent throughput samples (bytes/s), for the detail drawer's speed chart. */
    val speedHistory: List<Double> = emptyList(),
    /** Bounded, typed stage timings used by transfer experiments and diagnostics. */
    val stageTimings: List<TransferStageTiming> = emptyList(),
    /** Timestamped log lines scoped to this transfer, for the detail drawer. */
    val log: List<String> = emptyList(),
)

internal object TransferActivityGroup {
    private const val ONE_TIME_PREFIX = "one-time:"
    private const val REMEMBERED_PREFIX = "remembered:"

    fun oneTime(draftId: String): String = scoped(ONE_TIME_PREFIX, draftId, "room draft id")

    fun remembered(relationshipId: String): String = scoped(REMEMBERED_PREFIX, relationshipId, "remembered relationship id")

    private fun scoped(
        prefix: String,
        value: String,
        name: String,
    ): String {
        val normalized = value.trim()
        require(normalized.isNotEmpty()) { "$name is required" }
        return prefix + normalized
    }
}

val Status.isTerminal: Boolean
    get() = TransferPresentationPolicy.isTerminal(this)

/** Human-readable byte count shared by every transfer surface. */
fun humanBytes(n: Long): String =
    when {
        n < 1024 -> "$n B"
        n < 1024 * 1024 -> "%.0f KB".format(n / 1024.0)
        n < 1024L * 1024 * 1024 -> "%.1f MB".format(n / 1048576.0)
        else -> "%.2f GB".format(n / 1073741824.0)
    }

/** Trailing-window average of the ~200 ms-published rate (~2.4 s): the headline
 *  speed/ETA smoothing policy, kept beside the model (not in a composable). */
fun smoothedBps(t: Transfer): Double {
    val window = t.speedHistory.takeLast(12)
    return if (window.isEmpty()) t.avgBps else window.average()
}
