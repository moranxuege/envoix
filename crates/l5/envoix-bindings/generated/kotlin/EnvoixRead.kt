// @generated from schema/read.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveats: `org.json` duplicate-key handling is runtime-dependent
// (Android keeps the last key; the reference json.org jar throws, so JVM unit
// tests may see MALFORMED_JSON where a device sees last-wins). JSON `-0`
// decodes as integer 0 here while the Rust reference codec rejects it (benign:
// every field with a positive minimum still fails its range check).

package com.envoix.bindings

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener

const val READ_SCHEMA_ID: String = "envoix/binding/read/5"
const val READ_MAX_FRAME_BYTES: Int = 1048576

enum class ReadErrorKind {
    FRAME_TOO_LARGE,
    MALFORMED_JSON,
    UNKNOWN_SCHEMA,
    SHAPE,
    UNKNOWN_FIELD,
    UNKNOWN_VARIANT,
    RANGE,
    BOUND,
}

/** Typed codec failure carrying only static schema context. */
class ReadContractException(val kind: ReadErrorKind, val context: String) :
    Exception("read contract: $kind at $context")

/** Bounded contract text that redacts ordinary string interpolation. */
data class ReadSecretString(private val value: String) {
    fun expose(): String = value

    override fun toString(): String = "ReadSecretString([redacted])"
}

enum class DirectionView {
    SEND,
    RECEIVE,
}

enum class PhaseView {
    PREPARING,
    PAIRING,
    AUTHENTICATING,
    TRANSFERRING,
    CONFIRMING,
    PUBLISHING,
    RESTORING,
}

enum class OutcomeCodeView {
    COMPLETED,
    CANCELLED,
    PAUSED,
    PEER_LOST,
    TIMEOUT,
    UNAUTHENTICATED,
    VERSION_MISMATCH,
    STORAGE_FAULT,
    PUBLISH_FAILED,
    SOURCE_UNREADABLE,
    NETWORK_UNREACHABLE,
    INTERNAL,
}

enum class RetryabilityView {
    RETRYABLE,
    TERMINAL,
    NEEDS_USER,
}

enum class RecoveryView {
    RE_PICK_SOURCE,
    RETRY_LATER,
    RECONNECT_PEER,
}

enum class PauseOriginView {
    LOCAL,
    PEER,
    LOST,
}

enum class WorkerKindView {
    ATTEMPT,
    STAGING,
}

enum class RetirementIntentView {
    PAUSE,
    CANCEL,
    FINALIZE,
}

enum class DutyKindView {
    SOURCE_HANDLE,
    GRANT,
    STAGING,
    PUBLICATION,
    COURIER,
    FOREGROUND,
    NOTIFICATION,
    LOCK,
    OPEN_SHARE,
}

enum class CapabilityActionView {
    POST_RECEIPT,
    SELECT_SOURCE,
}

enum class CommandKindView {
    PAUSE,
    CANCEL,
    RESUME,
    REMOVE,
    RE_PICK_SOURCE,
}

enum class RedactedIdKindView {
    RECORD,
    TRANSFER,
    ARTIFACT,
    REQUEST,
}

enum class LosslessKindView {
    TERMINAL,
    CAPABILITY_DUTY,
}

enum class SubscribeRejectionView {
    UNKNOWN_CARD,
    RUNTIME_STOPPED,
    EPOCH_EXHAUSTED,
}

data class OutcomeView(
    val code: OutcomeCodeView,
    val phase: PhaseView,
    val retry: RetryabilityView,
    val recovery: RecoveryView?,
    val display: String,
)

data class PausedView(
    val origin: PauseOriginView,
)

sealed interface ProductStateView {
    object Preparing : ProductStateView
    object Waiting : ProductStateView
    object Connecting : ProductStateView
    object Verifying : ProductStateView
    object Transferring : ProductStateView
    object Confirming : ProductStateView
    data class Paused(val value: PausedView) : ProductStateView
    object Unconfirmed : ProductStateView
    object Completed : ProductStateView
    object Failed : ProductStateView
    object Cancelled : ProductStateView
}

data class RunningView(
    val worker: WorkerKindView,
)

data class RetiringView(
    val worker: WorkerKindView,
    val intent: RetirementIntentView,
)

sealed interface QuiescenceView {
    data class Running(val value: RunningView) : QuiescenceView
    data class Retiring(val value: RetiringView) : QuiescenceView
    object Quiescent : QuiescenceView
}

data class IdentityView(
    val card: String,
    val transfer: String,
    val artifact: String,
)

data class InviteView(
    val code: ReadSecretString,
    val codeFingerprint: String,
    val link: ReadSecretString?,
)

data class CardView(
    val identity: IdentityView,
    val direction: DirectionView,
    val offeredName: String,
    val total: Long,
    val state: ProductStateView,
    val quiescence: QuiescenceView,
    val generation: Long,
    val phase: PhaseView,
    val bytes: Long,
    val bytesResumed: Long,
    val outcome: OutcomeView?,
    val allowedActions: List<CommandKindView>,
    val invite: InviteView?,
)

data class DutyProvenanceView(
    val card: String,
    val generation: Long,
    val request: String,
)

data class DutyView(
    val provenance: DutyProvenanceView,
    val kind: DutyKindView,
)

data class DutyFrameView(
    val duty: DutyView,
    val action: CapabilityActionView,
)

sealed interface CardUpdateKindView {
    data class Snapshot(val value: CardView) : CardUpdateKindView
    data class Progress(val value: CardView) : CardUpdateKindView
    data class State(val value: CardView) : CardUpdateKindView
    data class Terminal(val value: CardView) : CardUpdateKindView
    data class CapabilityDuty(val value: DutyFrameView) : CardUpdateKindView
}

data class CardUpdateView(
    val epoch: Long,
    val card: String,
    val kind: CardUpdateKindView,
)

data class LagView(
    val epoch: Long,
    val card: String,
    val missed: LosslessKindView,
)

data class ClosedView(
    val epoch: Long,
    val card: String,
)

data class SubscribeRejectedView(
    val card: String,
    val reason: SubscribeRejectionView,
)

data class SessionKeyView(
    val card: String,
    val generation: Long,
)

data class EvidenceProgressView(
    val transferred: Long,
    val total: Long,
)

data class RedactedIdView(
    val kind: RedactedIdKindView,
)

sealed interface EvidenceValueView {
    data class Phase(val value: PhaseView) : EvidenceValueView
    data class Progress(val value: EvidenceProgressView) : EvidenceValueView
    data class Outcome(val value: OutcomeView) : EvidenceValueView
    data class Identifier(val value: RedactedIdView) : EvidenceValueView
}

data class DegradedView(
    val droppedEvents: Long,
)

sealed interface DiagnosticsStatusView {
    object Complete : DiagnosticsStatusView
    data class Degraded(val value: DegradedView) : DiagnosticsStatusView
}

data class TimelineEntryView(
    val sequence: Long,
    val value: EvidenceValueView,
)

data class EvidenceTimelineView(
    val session: SessionKeyView,
    val status: DiagnosticsStatusView,
    val entries: List<TimelineEntryView>,
)

data class ProtocolManifestView(
    val setId: String,
    val dataAlpn: String,
    val dataMagic: String,
    val dataWireVersion: Long,
)

data class AbiSchemaManifestView(
    val readBindingSchemaId: String,
    val commandBindingSchemaId: String,
    val evidenceRustAbiId: String,
    val evidenceTimelineSchemaId: String,
    val mailboxReceiptSchemaId: String,
    val operationEnvelopeSchemaId: String,
)

data class TrustRootSha256View(
    val fingerprint: String,
)

sealed interface TrustRootView {
    object Unprovisioned : TrustRootView
    data class Sha256(val value: TrustRootSha256View) : TrustRootView
}

data class BuildManifestView(
    val packageVersion: String,
    val protocol: ProtocolManifestView,
    val abiSchema: AbiSchemaManifestView,
    val trustRoot: TrustRootView,
)

sealed interface ReadBody {
    data class CardUpdate(val value: CardUpdateView) : ReadBody
    data class Lag(val value: LagView) : ReadBody
    data class Closed(val value: ClosedView) : ReadBody
    data class SubscribeRejected(val value: SubscribeRejectedView) : ReadBody
    data class Evidence(val value: EvidenceTimelineView) : ReadBody
    data class BuildManifest(val value: BuildManifestView) : ReadBody
}

data class ReadFrame(
    val body: ReadBody,
)

enum class GateDecision {
    DELIVER,
    DROP_STALE,
    CONTRACT_BREACH,
}

/**
 * Client-side admission for the per-epoch card stream: one gate per
 * attachment. Frames from another epoch are stale; every epoch starts
 * with a snapshot; a lag or close ends the epoch permanently.
 */
class EpochGate(private val epoch: Long) {
    private var sawSnapshot = false
    private var dead = false

    fun admit(frame: ReadFrame): GateDecision = when (val body = frame.body) {
        is ReadBody.CardUpdate -> {
            val update = body.value
            if (update.epoch != epoch || dead) {
                GateDecision.DROP_STALE
            } else if (update.kind is CardUpdateKindView.Snapshot) {
                if (sawSnapshot) {
                    GateDecision.CONTRACT_BREACH
                } else {
                    sawSnapshot = true
                    GateDecision.DELIVER
                }
            } else if (sawSnapshot) {
                GateDecision.DELIVER
            } else {
                GateDecision.CONTRACT_BREACH
            }
        }
        is ReadBody.Lag -> terminate(body.value.epoch)
        is ReadBody.Closed -> terminate(body.value.epoch)
        else -> GateDecision.DELIVER
    }

    private fun terminate(frameEpoch: Long): GateDecision =
        if (frameEpoch == epoch && !dead) {
            dead = true
            GateDecision.DELIVER
        } else {
            GateDecision.DROP_STALE
        }
}

object EnvoixReadCodec {
    /**
     * Decodes and validates one frame. Every failure is a typed
     * [ReadContractException]; no input, however hostile, misparses.
     */
    fun decode(text: String): ReadFrame {
        if (text.toByteArray(Charsets.UTF_8).size > READ_MAX_FRAME_BYTES) {
            throw ReadContractException(ReadErrorKind.FRAME_TOO_LARGE, "ReadFrame")
        }
        val tokener = JSONTokener(text)
        val value = try {
            tokener.nextValue()
        } catch (exception: JSONException) {
            throw ReadContractException(ReadErrorKind.MALFORMED_JSON, "ReadFrame")
        }
        while (tokener.more()) {
            val trailing = tokener.next()
            if (trailing != ' ' && trailing != '\t' && trailing != '\r' && trailing != '\n') {
                throw ReadContractException(ReadErrorKind.MALFORMED_JSON, "ReadFrame")
            }
        }
        val map = obj(value, "ReadFrame")
        val schema = map.opt("schema")
        if (schema !is String) {
            throw ReadContractException(ReadErrorKind.SHAPE, "ReadFrame.schema")
        }
        if (schema != READ_SCHEMA_ID) {
            throw ReadContractException(ReadErrorKind.UNKNOWN_SCHEMA, "ReadFrame")
        }
        return decodeReadFrame(value, "ReadFrame")
    }

    private fun obj(value: Any?, context: String): JSONObject =
        value as? JSONObject ?: throw ReadContractException(ReadErrorKind.SHAPE, context)

    private fun knownKeys(map: JSONObject, allowed: Set<String>, context: String) {
        for (key in map.keys()) {
            if (key !in allowed) {
                throw ReadContractException(ReadErrorKind.UNKNOWN_FIELD, context)
            }
        }
    }

    private fun field(map: JSONObject, key: String, context: String): Any? {
        if (!map.has(key)) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        val value = map.get(key)
        return if (value == JSONObject.NULL) null else value
    }

    private fun integer(value: Any?, max: Long, context: String): Long {
        val number = when (value) {
            is Int -> value.toLong()
            is Long -> value
            else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        if (number < 0 || number > max) {
            throw ReadContractException(ReadErrorKind.RANGE, context)
        }
        return number
    }

    private fun hexChars(text: String): Boolean =
        text.all { it in '0'..'9' || it in 'a'..'f' }

    private fun hexFixed(value: Any?, chars: Int, context: String): String {
        if (value !is String) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        if (value.length != chars || !hexChars(value)) {
            throw ReadContractException(ReadErrorKind.BOUND, context)
        }
        return value
    }

    private fun hexVariable(value: Any?, maxChars: Int, context: String): String {
        if (value !is String) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        val valid = value.isNotEmpty() &&
            value.length % 2 == 0 &&
            value.length <= maxChars &&
            hexChars(value)
        if (!valid) {
            throw ReadContractException(ReadErrorKind.BOUND, context)
        }
        return value
    }

    private fun utf8Bounded(value: Any?, maxBytes: Int, context: String): String {
        if (value !is String) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        // Unpaired surrogates parse here but not in the Rust reference codec;
        // reject them so every language accepts the same strings.
        var index = 0
        while (index < value.length) {
            val unit = value[index]
            if (unit.isHighSurrogate()) {
                if (index + 1 == value.length || !value[index + 1].isLowSurrogate()) {
                    throw ReadContractException(ReadErrorKind.SHAPE, context)
                }
                index += 2
            } else if (unit.isLowSurrogate()) {
                throw ReadContractException(ReadErrorKind.SHAPE, context)
            } else {
                index += 1
            }
        }
        if (value.toByteArray(Charsets.UTF_8).size > maxBytes) {
            throw ReadContractException(ReadErrorKind.BOUND, context)
        }
        return value
    }

    private fun asciiBounded(value: Any?, maxBytes: Int, context: String): String {
        if (value !is String) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
        val valid = value.length <= maxBytes && value.all { it in ' '..'~' }
        if (!valid) {
            throw ReadContractException(ReadErrorKind.BOUND, context)
        }
        return value
    }

    private fun <T> decodeList(
        value: Any?,
        maxLen: Int,
        context: String,
        decodeElement: (Any?, String) -> T,
    ): List<T> {
        val items = value as? JSONArray
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        if (items.length() > maxLen) {
            throw ReadContractException(ReadErrorKind.BOUND, context)
        }
        return (0 until items.length()).map { index ->
            val item = items.get(index)
            decodeElement(if (item == JSONObject.NULL) null else item, context)
        }
    }

    private fun payload(map: JSONObject, context: String): Any {
        val value = field(map, "value", context)
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return value
    }

    private fun unitPayload(map: JSONObject, context: String) {
        if (map.has("value") && map.get("value") != JSONObject.NULL) {
            throw ReadContractException(ReadErrorKind.SHAPE, context)
        }
    }

    private fun decodeDirectionView(value: Any?, context: String): DirectionView = when (value) {
        "send" -> DirectionView.SEND
        "receive" -> DirectionView.RECEIVE
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodePhaseView(value: Any?, context: String): PhaseView = when (value) {
        "preparing" -> PhaseView.PREPARING
        "pairing" -> PhaseView.PAIRING
        "authenticating" -> PhaseView.AUTHENTICATING
        "transferring" -> PhaseView.TRANSFERRING
        "confirming" -> PhaseView.CONFIRMING
        "publishing" -> PhaseView.PUBLISHING
        "restoring" -> PhaseView.RESTORING
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeOutcomeCodeView(value: Any?, context: String): OutcomeCodeView = when (value) {
        "completed" -> OutcomeCodeView.COMPLETED
        "cancelled" -> OutcomeCodeView.CANCELLED
        "paused" -> OutcomeCodeView.PAUSED
        "peer_lost" -> OutcomeCodeView.PEER_LOST
        "timeout" -> OutcomeCodeView.TIMEOUT
        "unauthenticated" -> OutcomeCodeView.UNAUTHENTICATED
        "version_mismatch" -> OutcomeCodeView.VERSION_MISMATCH
        "storage_fault" -> OutcomeCodeView.STORAGE_FAULT
        "publish_failed" -> OutcomeCodeView.PUBLISH_FAILED
        "source_unreadable" -> OutcomeCodeView.SOURCE_UNREADABLE
        "network_unreachable" -> OutcomeCodeView.NETWORK_UNREACHABLE
        "internal" -> OutcomeCodeView.INTERNAL
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeRetryabilityView(value: Any?, context: String): RetryabilityView = when (value) {
        "retryable" -> RetryabilityView.RETRYABLE
        "terminal" -> RetryabilityView.TERMINAL
        "needs_user" -> RetryabilityView.NEEDS_USER
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeRecoveryView(value: Any?, context: String): RecoveryView = when (value) {
        "re_pick_source" -> RecoveryView.RE_PICK_SOURCE
        "retry_later" -> RecoveryView.RETRY_LATER
        "reconnect_peer" -> RecoveryView.RECONNECT_PEER
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodePauseOriginView(value: Any?, context: String): PauseOriginView = when (value) {
        "local" -> PauseOriginView.LOCAL
        "peer" -> PauseOriginView.PEER
        "lost" -> PauseOriginView.LOST
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeWorkerKindView(value: Any?, context: String): WorkerKindView = when (value) {
        "attempt" -> WorkerKindView.ATTEMPT
        "staging" -> WorkerKindView.STAGING
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeRetirementIntentView(value: Any?, context: String): RetirementIntentView = when (value) {
        "pause" -> RetirementIntentView.PAUSE
        "cancel" -> RetirementIntentView.CANCEL
        "finalize" -> RetirementIntentView.FINALIZE
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeDutyKindView(value: Any?, context: String): DutyKindView = when (value) {
        "source_handle" -> DutyKindView.SOURCE_HANDLE
        "grant" -> DutyKindView.GRANT
        "staging" -> DutyKindView.STAGING
        "publication" -> DutyKindView.PUBLICATION
        "courier" -> DutyKindView.COURIER
        "foreground" -> DutyKindView.FOREGROUND
        "notification" -> DutyKindView.NOTIFICATION
        "lock" -> DutyKindView.LOCK
        "open_share" -> DutyKindView.OPEN_SHARE
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeCapabilityActionView(value: Any?, context: String): CapabilityActionView = when (value) {
        "post_receipt" -> CapabilityActionView.POST_RECEIPT
        "select_source" -> CapabilityActionView.SELECT_SOURCE
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeCommandKindView(value: Any?, context: String): CommandKindView = when (value) {
        "pause" -> CommandKindView.PAUSE
        "cancel" -> CommandKindView.CANCEL
        "resume" -> CommandKindView.RESUME
        "remove" -> CommandKindView.REMOVE
        "re_pick_source" -> CommandKindView.RE_PICK_SOURCE
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeRedactedIdKindView(value: Any?, context: String): RedactedIdKindView = when (value) {
        "record" -> RedactedIdKindView.RECORD
        "transfer" -> RedactedIdKindView.TRANSFER
        "artifact" -> RedactedIdKindView.ARTIFACT
        "request" -> RedactedIdKindView.REQUEST
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeLosslessKindView(value: Any?, context: String): LosslessKindView = when (value) {
        "terminal" -> LosslessKindView.TERMINAL
        "capability_duty" -> LosslessKindView.CAPABILITY_DUTY
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeSubscribeRejectionView(value: Any?, context: String): SubscribeRejectionView = when (value) {
        "unknown_card" -> SubscribeRejectionView.UNKNOWN_CARD
        "runtime_stopped" -> SubscribeRejectionView.RUNTIME_STOPPED
        "epoch_exhausted" -> SubscribeRejectionView.EPOCH_EXHAUSTED
        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        else -> throw ReadContractException(ReadErrorKind.SHAPE, context)
    }

    private fun decodeOutcomeView(value: Any?, context: String): OutcomeView {
        val map = obj(value, context)
        knownKeys(map, setOf("code", "phase", "retry", "recovery", "display"), context)
        return OutcomeView(
            code = decodeOutcomeCodeView(field(map, "code", "OutcomeView.code"), "OutcomeView.code"),
            phase = decodePhaseView(field(map, "phase", "OutcomeView.phase"), "OutcomeView.phase"),
            retry = decodeRetryabilityView(field(map, "retry", "OutcomeView.retry"), "OutcomeView.retry"),
            recovery = field(map, "recovery", "OutcomeView.recovery")?.let { decodeRecoveryView(it, "OutcomeView.recovery") },
            display = utf8Bounded(field(map, "display", "OutcomeView.display"), 160, "OutcomeView.display"),
        )
    }

    private fun decodePausedView(value: Any?, context: String): PausedView {
        val map = obj(value, context)
        knownKeys(map, setOf("origin"), context)
        return PausedView(
            origin = decodePauseOriginView(field(map, "origin", "PausedView.origin"), "PausedView.origin"),
        )
    }

    private fun decodeProductStateView(value: Any?, context: String): ProductStateView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "preparing" -> {
                unitPayload(map, "ProductStateView.preparing")
                ProductStateView.Preparing
            }
            "waiting" -> {
                unitPayload(map, "ProductStateView.waiting")
                ProductStateView.Waiting
            }
            "connecting" -> {
                unitPayload(map, "ProductStateView.connecting")
                ProductStateView.Connecting
            }
            "verifying" -> {
                unitPayload(map, "ProductStateView.verifying")
                ProductStateView.Verifying
            }
            "transferring" -> {
                unitPayload(map, "ProductStateView.transferring")
                ProductStateView.Transferring
            }
            "confirming" -> {
                unitPayload(map, "ProductStateView.confirming")
                ProductStateView.Confirming
            }
            "paused" -> ProductStateView.Paused(
                decodePausedView(payload(map, "ProductStateView.paused"), "ProductStateView.paused"),
            )
            "unconfirmed" -> {
                unitPayload(map, "ProductStateView.unconfirmed")
                ProductStateView.Unconfirmed
            }
            "completed" -> {
                unitPayload(map, "ProductStateView.completed")
                ProductStateView.Completed
            }
            "failed" -> {
                unitPayload(map, "ProductStateView.failed")
                ProductStateView.Failed
            }
            "cancelled" -> {
                unitPayload(map, "ProductStateView.cancelled")
                ProductStateView.Cancelled
            }
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeRunningView(value: Any?, context: String): RunningView {
        val map = obj(value, context)
        knownKeys(map, setOf("worker"), context)
        return RunningView(
            worker = decodeWorkerKindView(field(map, "worker", "RunningView.worker"), "RunningView.worker"),
        )
    }

    private fun decodeRetiringView(value: Any?, context: String): RetiringView {
        val map = obj(value, context)
        knownKeys(map, setOf("worker", "intent"), context)
        return RetiringView(
            worker = decodeWorkerKindView(field(map, "worker", "RetiringView.worker"), "RetiringView.worker"),
            intent = decodeRetirementIntentView(field(map, "intent", "RetiringView.intent"), "RetiringView.intent"),
        )
    }

    private fun decodeQuiescenceView(value: Any?, context: String): QuiescenceView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "running" -> QuiescenceView.Running(
                decodeRunningView(payload(map, "QuiescenceView.running"), "QuiescenceView.running"),
            )
            "retiring" -> QuiescenceView.Retiring(
                decodeRetiringView(payload(map, "QuiescenceView.retiring"), "QuiescenceView.retiring"),
            )
            "quiescent" -> {
                unitPayload(map, "QuiescenceView.quiescent")
                QuiescenceView.Quiescent
            }
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeIdentityView(value: Any?, context: String): IdentityView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "transfer", "artifact"), context)
        return IdentityView(
            card = hexFixed(field(map, "card", "IdentityView.card"), 16, "IdentityView.card"),
            transfer = hexFixed(field(map, "transfer", "IdentityView.transfer"), 32, "IdentityView.transfer"),
            artifact = hexFixed(field(map, "artifact", "IdentityView.artifact"), 32, "IdentityView.artifact"),
        )
    }

    private fun decodeInviteView(value: Any?, context: String): InviteView {
        val map = obj(value, context)
        knownKeys(map, setOf("code", "code_fingerprint", "link"), context)
        return InviteView(
            code = ReadSecretString(utf8Bounded(field(map, "code", "InviteView.code"), 64, "InviteView.code")),
            codeFingerprint = hexFixed(field(map, "code_fingerprint", "InviteView.code_fingerprint"), 16, "InviteView.code_fingerprint"),
            link = field(map, "link", "InviteView.link")?.let { ReadSecretString(utf8Bounded(it, 5481, "InviteView.link")) },
        )
    }

    private fun decodeCardView(value: Any?, context: String): CardView {
        val map = obj(value, context)
        knownKeys(map, setOf("identity", "direction", "offered_name", "total", "state", "quiescence", "generation", "phase", "bytes", "bytes_resumed", "outcome", "allowed_actions", "invite"), context)
        return CardView(
            identity = decodeIdentityView(field(map, "identity", "CardView.identity"), "CardView.identity"),
            direction = decodeDirectionView(field(map, "direction", "CardView.direction"), "CardView.direction"),
            offeredName = utf8Bounded(field(map, "offered_name", "CardView.offered_name"), 255, "CardView.offered_name"),
            total = integer(field(map, "total", "CardView.total"), Long.MAX_VALUE, "CardView.total"),
            state = decodeProductStateView(field(map, "state", "CardView.state"), "CardView.state"),
            quiescence = decodeQuiescenceView(field(map, "quiescence", "CardView.quiescence"), "CardView.quiescence"),
            generation = integer(field(map, "generation", "CardView.generation"), 4294967295, "CardView.generation"),
            phase = decodePhaseView(field(map, "phase", "CardView.phase"), "CardView.phase"),
            bytes = integer(field(map, "bytes", "CardView.bytes"), Long.MAX_VALUE, "CardView.bytes"),
            bytesResumed = integer(field(map, "bytes_resumed", "CardView.bytes_resumed"), Long.MAX_VALUE, "CardView.bytes_resumed"),
            outcome = field(map, "outcome", "CardView.outcome")?.let { decodeOutcomeView(it, "CardView.outcome") },
            allowedActions = decodeList(field(map, "allowed_actions", "CardView.allowed_actions"), 5, "CardView.allowed_actions", ::decodeCommandKindView),
            invite = field(map, "invite", "CardView.invite")?.let { decodeInviteView(it, "CardView.invite") },
        )
    }

    private fun decodeDutyProvenanceView(value: Any?, context: String): DutyProvenanceView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "generation", "request"), context)
        return DutyProvenanceView(
            card = hexFixed(field(map, "card", "DutyProvenanceView.card"), 16, "DutyProvenanceView.card"),
            generation = integer(field(map, "generation", "DutyProvenanceView.generation"), 4294967295, "DutyProvenanceView.generation"),
            request = hexFixed(field(map, "request", "DutyProvenanceView.request"), 32, "DutyProvenanceView.request"),
        )
    }

    private fun decodeDutyView(value: Any?, context: String): DutyView {
        val map = obj(value, context)
        knownKeys(map, setOf("provenance", "kind"), context)
        return DutyView(
            provenance = decodeDutyProvenanceView(field(map, "provenance", "DutyView.provenance"), "DutyView.provenance"),
            kind = decodeDutyKindView(field(map, "kind", "DutyView.kind"), "DutyView.kind"),
        )
    }

    private fun decodeDutyFrameView(value: Any?, context: String): DutyFrameView {
        val map = obj(value, context)
        knownKeys(map, setOf("duty", "action"), context)
        return DutyFrameView(
            duty = decodeDutyView(field(map, "duty", "DutyFrameView.duty"), "DutyFrameView.duty"),
            action = decodeCapabilityActionView(field(map, "action", "DutyFrameView.action"), "DutyFrameView.action"),
        )
    }

    private fun decodeCardUpdateKindView(value: Any?, context: String): CardUpdateKindView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "snapshot" -> CardUpdateKindView.Snapshot(
                decodeCardView(payload(map, "CardUpdateKindView.snapshot"), "CardUpdateKindView.snapshot"),
            )
            "progress" -> CardUpdateKindView.Progress(
                decodeCardView(payload(map, "CardUpdateKindView.progress"), "CardUpdateKindView.progress"),
            )
            "state" -> CardUpdateKindView.State(
                decodeCardView(payload(map, "CardUpdateKindView.state"), "CardUpdateKindView.state"),
            )
            "terminal" -> CardUpdateKindView.Terminal(
                decodeCardView(payload(map, "CardUpdateKindView.terminal"), "CardUpdateKindView.terminal"),
            )
            "capability_duty" -> CardUpdateKindView.CapabilityDuty(
                decodeDutyFrameView(payload(map, "CardUpdateKindView.capability_duty"), "CardUpdateKindView.capability_duty"),
            )
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCardUpdateView(value: Any?, context: String): CardUpdateView {
        val map = obj(value, context)
        knownKeys(map, setOf("epoch", "card", "kind"), context)
        return CardUpdateView(
            epoch = integer(field(map, "epoch", "CardUpdateView.epoch"), Long.MAX_VALUE, "CardUpdateView.epoch"),
            card = hexFixed(field(map, "card", "CardUpdateView.card"), 16, "CardUpdateView.card"),
            kind = decodeCardUpdateKindView(field(map, "kind", "CardUpdateView.kind"), "CardUpdateView.kind"),
        )
    }

    private fun decodeLagView(value: Any?, context: String): LagView {
        val map = obj(value, context)
        knownKeys(map, setOf("epoch", "card", "missed"), context)
        return LagView(
            epoch = integer(field(map, "epoch", "LagView.epoch"), Long.MAX_VALUE, "LagView.epoch"),
            card = hexFixed(field(map, "card", "LagView.card"), 16, "LagView.card"),
            missed = decodeLosslessKindView(field(map, "missed", "LagView.missed"), "LagView.missed"),
        )
    }

    private fun decodeClosedView(value: Any?, context: String): ClosedView {
        val map = obj(value, context)
        knownKeys(map, setOf("epoch", "card"), context)
        return ClosedView(
            epoch = integer(field(map, "epoch", "ClosedView.epoch"), Long.MAX_VALUE, "ClosedView.epoch"),
            card = hexFixed(field(map, "card", "ClosedView.card"), 16, "ClosedView.card"),
        )
    }

    private fun decodeSubscribeRejectedView(value: Any?, context: String): SubscribeRejectedView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "reason"), context)
        return SubscribeRejectedView(
            card = hexFixed(field(map, "card", "SubscribeRejectedView.card"), 16, "SubscribeRejectedView.card"),
            reason = decodeSubscribeRejectionView(field(map, "reason", "SubscribeRejectedView.reason"), "SubscribeRejectedView.reason"),
        )
    }

    private fun decodeSessionKeyView(value: Any?, context: String): SessionKeyView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "generation"), context)
        return SessionKeyView(
            card = hexFixed(field(map, "card", "SessionKeyView.card"), 16, "SessionKeyView.card"),
            generation = integer(field(map, "generation", "SessionKeyView.generation"), 4294967295, "SessionKeyView.generation"),
        )
    }

    private fun decodeEvidenceProgressView(value: Any?, context: String): EvidenceProgressView {
        val map = obj(value, context)
        knownKeys(map, setOf("transferred", "total"), context)
        return EvidenceProgressView(
            transferred = integer(field(map, "transferred", "EvidenceProgressView.transferred"), Long.MAX_VALUE, "EvidenceProgressView.transferred"),
            total = integer(field(map, "total", "EvidenceProgressView.total"), Long.MAX_VALUE, "EvidenceProgressView.total"),
        )
    }

    private fun decodeRedactedIdView(value: Any?, context: String): RedactedIdView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind"), context)
        return RedactedIdView(
            kind = decodeRedactedIdKindView(field(map, "kind", "RedactedIdView.kind"), "RedactedIdView.kind"),
        )
    }

    private fun decodeEvidenceValueView(value: Any?, context: String): EvidenceValueView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "phase" -> EvidenceValueView.Phase(
                decodePhaseView(payload(map, "EvidenceValueView.phase"), "EvidenceValueView.phase"),
            )
            "progress" -> EvidenceValueView.Progress(
                decodeEvidenceProgressView(payload(map, "EvidenceValueView.progress"), "EvidenceValueView.progress"),
            )
            "outcome" -> EvidenceValueView.Outcome(
                decodeOutcomeView(payload(map, "EvidenceValueView.outcome"), "EvidenceValueView.outcome"),
            )
            "identifier" -> EvidenceValueView.Identifier(
                decodeRedactedIdView(payload(map, "EvidenceValueView.identifier"), "EvidenceValueView.identifier"),
            )
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeDegradedView(value: Any?, context: String): DegradedView {
        val map = obj(value, context)
        knownKeys(map, setOf("dropped_events"), context)
        return DegradedView(
            droppedEvents = integer(field(map, "dropped_events", "DegradedView.dropped_events"), Long.MAX_VALUE, "DegradedView.dropped_events"),
        )
    }

    private fun decodeDiagnosticsStatusView(value: Any?, context: String): DiagnosticsStatusView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "complete" -> {
                unitPayload(map, "DiagnosticsStatusView.complete")
                DiagnosticsStatusView.Complete
            }
            "degraded" -> DiagnosticsStatusView.Degraded(
                decodeDegradedView(payload(map, "DiagnosticsStatusView.degraded"), "DiagnosticsStatusView.degraded"),
            )
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeTimelineEntryView(value: Any?, context: String): TimelineEntryView {
        val map = obj(value, context)
        knownKeys(map, setOf("sequence", "value"), context)
        return TimelineEntryView(
            sequence = integer(field(map, "sequence", "TimelineEntryView.sequence"), Long.MAX_VALUE, "TimelineEntryView.sequence"),
            value = decodeEvidenceValueView(field(map, "value", "TimelineEntryView.value"), "TimelineEntryView.value"),
        )
    }

    private fun decodeEvidenceTimelineView(value: Any?, context: String): EvidenceTimelineView {
        val map = obj(value, context)
        knownKeys(map, setOf("session", "status", "entries"), context)
        return EvidenceTimelineView(
            session = decodeSessionKeyView(field(map, "session", "EvidenceTimelineView.session"), "EvidenceTimelineView.session"),
            status = decodeDiagnosticsStatusView(field(map, "status", "EvidenceTimelineView.status"), "EvidenceTimelineView.status"),
            entries = decodeList(field(map, "entries", "EvidenceTimelineView.entries"), 1024, "EvidenceTimelineView.entries", ::decodeTimelineEntryView),
        )
    }

    private fun decodeProtocolManifestView(value: Any?, context: String): ProtocolManifestView {
        val map = obj(value, context)
        knownKeys(map, setOf("set_id", "data_alpn", "data_magic", "data_wire_version"), context)
        return ProtocolManifestView(
            setId = asciiBounded(field(map, "set_id", "ProtocolManifestView.set_id"), 64, "ProtocolManifestView.set_id"),
            dataAlpn = hexVariable(field(map, "data_alpn", "ProtocolManifestView.data_alpn"), 64, "ProtocolManifestView.data_alpn"),
            dataMagic = hexVariable(field(map, "data_magic", "ProtocolManifestView.data_magic"), 32, "ProtocolManifestView.data_magic"),
            dataWireVersion = integer(field(map, "data_wire_version", "ProtocolManifestView.data_wire_version"), 65535, "ProtocolManifestView.data_wire_version"),
        )
    }

    private fun decodeAbiSchemaManifestView(value: Any?, context: String): AbiSchemaManifestView {
        val map = obj(value, context)
        knownKeys(map, setOf("read_binding_schema_id", "command_binding_schema_id", "evidence_rust_abi_id", "evidence_timeline_schema_id", "mailbox_receipt_schema_id", "operation_envelope_schema_id"), context)
        return AbiSchemaManifestView(
            readBindingSchemaId = asciiBounded(field(map, "read_binding_schema_id", "AbiSchemaManifestView.read_binding_schema_id"), 64, "AbiSchemaManifestView.read_binding_schema_id"),
            commandBindingSchemaId = asciiBounded(field(map, "command_binding_schema_id", "AbiSchemaManifestView.command_binding_schema_id"), 64, "AbiSchemaManifestView.command_binding_schema_id"),
            evidenceRustAbiId = asciiBounded(field(map, "evidence_rust_abi_id", "AbiSchemaManifestView.evidence_rust_abi_id"), 64, "AbiSchemaManifestView.evidence_rust_abi_id"),
            evidenceTimelineSchemaId = asciiBounded(field(map, "evidence_timeline_schema_id", "AbiSchemaManifestView.evidence_timeline_schema_id"), 64, "AbiSchemaManifestView.evidence_timeline_schema_id"),
            mailboxReceiptSchemaId = asciiBounded(field(map, "mailbox_receipt_schema_id", "AbiSchemaManifestView.mailbox_receipt_schema_id"), 64, "AbiSchemaManifestView.mailbox_receipt_schema_id"),
            operationEnvelopeSchemaId = asciiBounded(field(map, "operation_envelope_schema_id", "AbiSchemaManifestView.operation_envelope_schema_id"), 64, "AbiSchemaManifestView.operation_envelope_schema_id"),
        )
    }

    private fun decodeTrustRootSha256View(value: Any?, context: String): TrustRootSha256View {
        val map = obj(value, context)
        knownKeys(map, setOf("fingerprint"), context)
        return TrustRootSha256View(
            fingerprint = hexFixed(field(map, "fingerprint", "TrustRootSha256View.fingerprint"), 64, "TrustRootSha256View.fingerprint"),
        )
    }

    private fun decodeTrustRootView(value: Any?, context: String): TrustRootView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "unprovisioned" -> {
                unitPayload(map, "TrustRootView.unprovisioned")
                TrustRootView.Unprovisioned
            }
            "sha256" -> TrustRootView.Sha256(
                decodeTrustRootSha256View(payload(map, "TrustRootView.sha256"), "TrustRootView.sha256"),
            )
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeBuildManifestView(value: Any?, context: String): BuildManifestView {
        val map = obj(value, context)
        knownKeys(map, setOf("package_version", "protocol", "abi_schema", "trust_root"), context)
        return BuildManifestView(
            packageVersion = asciiBounded(field(map, "package_version", "BuildManifestView.package_version"), 32, "BuildManifestView.package_version"),
            protocol = decodeProtocolManifestView(field(map, "protocol", "BuildManifestView.protocol"), "BuildManifestView.protocol"),
            abiSchema = decodeAbiSchemaManifestView(field(map, "abi_schema", "BuildManifestView.abi_schema"), "BuildManifestView.abi_schema"),
            trustRoot = decodeTrustRootView(field(map, "trust_root", "BuildManifestView.trust_root"), "BuildManifestView.trust_root"),
        )
    }

    private fun decodeReadBody(value: Any?, context: String): ReadBody {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw ReadContractException(ReadErrorKind.SHAPE, context)
        return when (kind) {
            "card_update" -> ReadBody.CardUpdate(
                decodeCardUpdateView(payload(map, "ReadBody.card_update"), "ReadBody.card_update"),
            )
            "lag" -> ReadBody.Lag(
                decodeLagView(payload(map, "ReadBody.lag"), "ReadBody.lag"),
            )
            "closed" -> ReadBody.Closed(
                decodeClosedView(payload(map, "ReadBody.closed"), "ReadBody.closed"),
            )
            "subscribe_rejected" -> ReadBody.SubscribeRejected(
                decodeSubscribeRejectedView(payload(map, "ReadBody.subscribe_rejected"), "ReadBody.subscribe_rejected"),
            )
            "evidence" -> ReadBody.Evidence(
                decodeEvidenceTimelineView(payload(map, "ReadBody.evidence"), "ReadBody.evidence"),
            )
            "build_manifest" -> ReadBody.BuildManifest(
                decodeBuildManifestView(payload(map, "ReadBody.build_manifest"), "ReadBody.build_manifest"),
            )
            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeReadFrame(value: Any?, context: String): ReadFrame {
        val map = obj(value, context)
        knownKeys(map, setOf("schema", "body"), context)
        return ReadFrame(
            body = decodeReadBody(field(map, "body", "ReadFrame.body"), "ReadFrame.body"),
        )
    }
}
