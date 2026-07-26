// @generated from schema/command.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveats: `org.json` duplicate-key handling is runtime-dependent
// (Android keeps the last key; the reference json.org jar throws, so JVM unit
// tests may see MALFORMED_JSON where a device sees last-wins). JSON `-0`
// decodes as integer 0 here while the Rust reference codec rejects it (benign:
// every field with a positive minimum still fails its range check).
// Encoded frames are semantically identical to the Rust reference codec's but
// not byte-identical: `org.json` decides key order and escaping, both of which
// are runtime-dependent. The wire contract is the decoded value, and every
// decoder here is order-insensitive. The frame cap is defined over the
// canonical (serde_json) serialization, and `org.json` escapes U+0080..U+009F
// and U+2000..U+20FF as `\uXXXX` — up to 3x the canonical bytes — so this
// encoder can refuse a frame the contract permits. It is never the other way
// round: the cap is measured on the bytes this artifact actually emits.

package com.envoix.bindings

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener

const val COMMAND_SCHEMA_ID: String = "envoix/binding/command/3"
const val COMMAND_MAX_FRAME_BYTES: Int = 1048576

// Contract rules frozen by schema/command.schema.
const val NEWEST_ATTACHMENT_COMMANDS: Boolean = true
const val RETRY_HORIZON_COMPLETIONS: Int = 256
const val SUPERSESSION_INERT_PRE_ACCEPTANCE_ONLY: Boolean = true

enum class CommandErrorKind {
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
class CommandContractException(val kind: CommandErrorKind, val context: String) :
    Exception("read contract: $kind at $context")

enum class CommandView {
    PAUSE,
    CANCEL,
    RESUME,
    REMOVE,
    RE_PICK_SOURCE,
}

enum class PauseCauseView {
    LOCAL,
    PEER,
    LOST,
}

data class PausedStateView(
    val origin: PauseCauseView,
)

sealed interface DispositionView {
    object Preparing : DispositionView
    object Waiting : DispositionView
    object Connecting : DispositionView
    object Verifying : DispositionView
    object Transferring : DispositionView
    object Confirming : DispositionView
    data class Paused(val value: PausedStateView) : DispositionView
    object Unconfirmed : DispositionView
    object Completed : DispositionView
    object Failed : DispositionView
    object Cancelled : DispositionView
}

data class SubmitView(
    val card: String,
    val epoch: Long,
    val commandId: String,
    val command: CommandView,
)

data class SendSourceView(
    val displayName: String,
    val total: Long,
)

data class JoinInviteView(
    val invite: String,
)

sealed interface CreateIntentView {
    data class Send(val value: SendSourceView) : CreateIntentView
    data class Join(val value: JoinInviteView) : CreateIntentView
}

data class CreateView(
    val intent: CreateIntentView,
    val requestId: String,
)

sealed interface FrontendIntentView {
    data class Command(val value: SubmitView) : FrontendIntentView
    data class Create(val value: CreateView) : FrontendIntentView
}

enum class RejectionView {
    UNKNOWN_CARD,
    STALE_EPOCH,
    SUPERSEDED,
    AT_CAPACITY,
    RUNTIME_STOPPED,
    INTERRUPTED,
    INTERNAL,
}

sealed interface AcceptanceView {
    object Accepted : AcceptanceView
    data class Duplicate(val value: DispositionView) : AcceptanceView
    data class Conflict(val value: CommandView) : AcceptanceView
    data class Rejected(val value: RejectionView) : AcceptanceView
}

data class CommandAcceptanceView(
    val commandId: String,
    val acceptance: AcceptanceView,
)

sealed interface CompletionView {
    data class Committed(val value: DispositionView) : CompletionView
    data class CommitFailed(val value: DispositionView) : CompletionView
    object Interrupted : CompletionView
    object Internal : CompletionView
}

data class CommandCompletionView(
    val commandId: String,
    val completion: CompletionView,
)

enum class CreateRefusalView {
    INVITE_NOT_RECOGNIZED,
    INVITE_BARE_ROOM_CODE,
    INVITE_MALFORMED,
    INVITE_TOO_LONG,
    INVITE_UNSUPPORTED,
    INVITE_ROLE_UNSUPPORTED,
    STORAGE_FAULT,
    INTERNAL,
}

data class CardCreatedView(
    val card: String,
)

sealed interface CreateOutcomeView {
    data class Created(val value: CardCreatedView) : CreateOutcomeView
    data class Refused(val value: CreateRefusalView) : CreateOutcomeView
}

data class CreateResultView(
    val outcome: CreateOutcomeView,
    val requestId: String,
)

sealed interface CommandBody {
    data class Intent(val value: FrontendIntentView) : CommandBody
    data class Acceptance(val value: CommandAcceptanceView) : CommandBody
    data class Completion(val value: CommandCompletionView) : CommandBody
    data class CreateResult(val value: CreateResultView) : CommandBody
}

data class CommandFrame(
    val body: CommandBody,
)

object EnvoixCommandCodec {
    /**
     * Decodes and validates one frame. Every failure is a typed
     * [CommandContractException]; no input, however hostile, misparses.
     */
    fun decode(text: String): CommandFrame {
        if (text.toByteArray(Charsets.UTF_8).size > COMMAND_MAX_FRAME_BYTES) {
            throw CommandContractException(CommandErrorKind.FRAME_TOO_LARGE, "CommandFrame")
        }
        val tokener = JSONTokener(text)
        val value = try {
            tokener.nextValue()
        } catch (exception: JSONException) {
            throw CommandContractException(CommandErrorKind.MALFORMED_JSON, "CommandFrame")
        }
        while (tokener.more()) {
            val trailing = tokener.next()
            if (trailing != ' ' && trailing != '\t' && trailing != '\r' && trailing != '\n') {
                throw CommandContractException(CommandErrorKind.MALFORMED_JSON, "CommandFrame")
            }
        }
        val map = obj(value, "CommandFrame")
        val schema = map.opt("schema")
        if (schema !is String) {
            throw CommandContractException(CommandErrorKind.SHAPE, "CommandFrame.schema")
        }
        if (schema != COMMAND_SCHEMA_ID) {
            throw CommandContractException(CommandErrorKind.UNKNOWN_SCHEMA, "CommandFrame")
        }
        return decodeCommandFrame(value, "CommandFrame")
    }

    /**
     * Encodes the one frame a frontend may originate, stamping the schema
     * envelope and the `intent` body around it and enforcing every bound
     * [decode] checks. Every failure is a typed [CommandContractException]; an
     * over-bound frame never leaves the process.
     */
    fun encode(body: FrontendIntentView): String {
        val map = JSONObject()
        map.put("schema", COMMAND_SCHEMA_ID)
        map.put(
            "body",
            JSONObject().put("kind", "intent").put("value", encodeFrontendIntentView(body)),
        )
        val text = map.toString()
        if (text.toByteArray(Charsets.UTF_8).size > COMMAND_MAX_FRAME_BYTES) {
            throw CommandContractException(CommandErrorKind.FRAME_TOO_LARGE, "CommandFrame")
        }
        return text
    }

    private fun obj(value: Any?, context: String): JSONObject =
        value as? JSONObject ?: throw CommandContractException(CommandErrorKind.SHAPE, context)

    private fun knownKeys(map: JSONObject, allowed: Set<String>, context: String) {
        for (key in map.keys()) {
            if (key !in allowed) {
                throw CommandContractException(CommandErrorKind.UNKNOWN_FIELD, context)
            }
        }
    }

    private fun field(map: JSONObject, key: String, context: String): Any? {
        if (!map.has(key)) {
            throw CommandContractException(CommandErrorKind.SHAPE, context)
        }
        val value = map.get(key)
        return if (value == JSONObject.NULL) null else value
    }

    private fun integer(value: Any?, max: Long, context: String): Long {
        val number = when (value) {
            is Int -> value.toLong()
            is Long -> value
            else -> throw CommandContractException(CommandErrorKind.SHAPE, context)
        }
        if (number < 0 || number > max) {
            throw CommandContractException(CommandErrorKind.RANGE, context)
        }
        return number
    }

    private fun hexChars(text: String): Boolean =
        text.all { it in '0'..'9' || it in 'a'..'f' }

    private fun hexFixed(value: Any?, chars: Int, context: String): String {
        if (value !is String) {
            throw CommandContractException(CommandErrorKind.SHAPE, context)
        }
        if (value.length != chars || !hexChars(value)) {
            throw CommandContractException(CommandErrorKind.BOUND, context)
        }
        return value
    }

    private fun utf8Bounded(value: Any?, maxBytes: Int, context: String): String {
        if (value !is String) {
            throw CommandContractException(CommandErrorKind.SHAPE, context)
        }
        // Unpaired surrogates parse here but not in the Rust reference codec;
        // reject them so every language accepts the same strings.
        var index = 0
        while (index < value.length) {
            val unit = value[index]
            if (unit.isHighSurrogate()) {
                if (index + 1 == value.length || !value[index + 1].isLowSurrogate()) {
                    throw CommandContractException(CommandErrorKind.SHAPE, context)
                }
                index += 2
            } else if (unit.isLowSurrogate()) {
                throw CommandContractException(CommandErrorKind.SHAPE, context)
            } else {
                index += 1
            }
        }
        if (value.toByteArray(Charsets.UTF_8).size > maxBytes) {
            throw CommandContractException(CommandErrorKind.BOUND, context)
        }
        return value
    }

    private fun payload(map: JSONObject, context: String): Any {
        val value = field(map, "value", context)
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return value
    }

    private fun unitPayload(map: JSONObject, context: String) {
        if (map.has("value") && map.get("value") != JSONObject.NULL) {
            throw CommandContractException(CommandErrorKind.SHAPE, context)
        }
    }

    private fun encodeInteger(value: Long, max: Long, context: String): Long =
        integer(value, max, context)

    private fun encodeHexFixed(value: String, chars: Int, context: String): String =
        hexFixed(value, chars, context)

    private fun encodeUtf8Bounded(value: String, maxBytes: Int, context: String): String =
        utf8Bounded(value, maxBytes, context)

    private fun decodeCommandView(value: Any?, context: String): CommandView = when (value) {
        "pause" -> CommandView.PAUSE
        "cancel" -> CommandView.CANCEL
        "resume" -> CommandView.RESUME
        "remove" -> CommandView.REMOVE
        "re_pick_source" -> CommandView.RE_PICK_SOURCE
        is String -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CommandContractException(CommandErrorKind.SHAPE, context)
    }

    private fun encodeCommandView(value: CommandView): String = when (value) {
        CommandView.PAUSE -> "pause"
        CommandView.CANCEL -> "cancel"
        CommandView.RESUME -> "resume"
        CommandView.REMOVE -> "remove"
        CommandView.RE_PICK_SOURCE -> "re_pick_source"
    }

    private fun decodePauseCauseView(value: Any?, context: String): PauseCauseView = when (value) {
        "local" -> PauseCauseView.LOCAL
        "peer" -> PauseCauseView.PEER
        "lost" -> PauseCauseView.LOST
        is String -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CommandContractException(CommandErrorKind.SHAPE, context)
    }

    private fun decodePausedStateView(value: Any?, context: String): PausedStateView {
        val map = obj(value, context)
        knownKeys(map, setOf("origin"), context)
        return PausedStateView(
            origin = decodePauseCauseView(field(map, "origin", "PausedStateView.origin"), "PausedStateView.origin"),
        )
    }

    private fun decodeDispositionView(value: Any?, context: String): DispositionView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "preparing" -> {
                unitPayload(map, "DispositionView.preparing")
                DispositionView.Preparing
            }
            "waiting" -> {
                unitPayload(map, "DispositionView.waiting")
                DispositionView.Waiting
            }
            "connecting" -> {
                unitPayload(map, "DispositionView.connecting")
                DispositionView.Connecting
            }
            "verifying" -> {
                unitPayload(map, "DispositionView.verifying")
                DispositionView.Verifying
            }
            "transferring" -> {
                unitPayload(map, "DispositionView.transferring")
                DispositionView.Transferring
            }
            "confirming" -> {
                unitPayload(map, "DispositionView.confirming")
                DispositionView.Confirming
            }
            "paused" -> DispositionView.Paused(
                decodePausedStateView(payload(map, "DispositionView.paused"), "DispositionView.paused"),
            )
            "unconfirmed" -> {
                unitPayload(map, "DispositionView.unconfirmed")
                DispositionView.Unconfirmed
            }
            "completed" -> {
                unitPayload(map, "DispositionView.completed")
                DispositionView.Completed
            }
            "failed" -> {
                unitPayload(map, "DispositionView.failed")
                DispositionView.Failed
            }
            "cancelled" -> {
                unitPayload(map, "DispositionView.cancelled")
                DispositionView.Cancelled
            }
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeSubmitView(value: Any?, context: String): SubmitView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "epoch", "command_id", "command"), context)
        return SubmitView(
            card = hexFixed(field(map, "card", "SubmitView.card"), 16, "SubmitView.card"),
            epoch = integer(field(map, "epoch", "SubmitView.epoch"), Long.MAX_VALUE, "SubmitView.epoch"),
            commandId = hexFixed(field(map, "command_id", "SubmitView.command_id"), 32, "SubmitView.command_id"),
            command = decodeCommandView(field(map, "command", "SubmitView.command"), "SubmitView.command"),
        )
    }

    private fun encodeSubmitView(value: SubmitView): JSONObject {
        val map = JSONObject()
        map.put("card", encodeHexFixed(value.card, 16, "SubmitView.card"))
        map.put("epoch", encodeInteger(value.epoch, Long.MAX_VALUE, "SubmitView.epoch"))
        map.put("command_id", encodeHexFixed(value.commandId, 32, "SubmitView.command_id"))
        map.put("command", encodeCommandView(value.command))
        return map
    }

    private fun decodeSendSourceView(value: Any?, context: String): SendSourceView {
        val map = obj(value, context)
        knownKeys(map, setOf("display_name", "total"), context)
        return SendSourceView(
            displayName = utf8Bounded(field(map, "display_name", "SendSourceView.display_name"), 255, "SendSourceView.display_name"),
            total = integer(field(map, "total", "SendSourceView.total"), Long.MAX_VALUE, "SendSourceView.total"),
        )
    }

    private fun encodeSendSourceView(value: SendSourceView): JSONObject {
        val map = JSONObject()
        map.put("display_name", encodeUtf8Bounded(value.displayName, 255, "SendSourceView.display_name"))
        map.put("total", encodeInteger(value.total, Long.MAX_VALUE, "SendSourceView.total"))
        return map
    }

    private fun decodeJoinInviteView(value: Any?, context: String): JoinInviteView {
        val map = obj(value, context)
        knownKeys(map, setOf("invite"), context)
        return JoinInviteView(
            invite = utf8Bounded(field(map, "invite", "JoinInviteView.invite"), 16384, "JoinInviteView.invite"),
        )
    }

    private fun encodeJoinInviteView(value: JoinInviteView): JSONObject {
        val map = JSONObject()
        map.put("invite", encodeUtf8Bounded(value.invite, 16384, "JoinInviteView.invite"))
        return map
    }

    private fun decodeCreateIntentView(value: Any?, context: String): CreateIntentView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "send" -> CreateIntentView.Send(
                decodeSendSourceView(payload(map, "CreateIntentView.send"), "CreateIntentView.send"),
            )
            "join" -> CreateIntentView.Join(
                decodeJoinInviteView(payload(map, "CreateIntentView.join"), "CreateIntentView.join"),
            )
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun encodeCreateIntentView(value: CreateIntentView): JSONObject = when (value) {
        is CreateIntentView.Send ->
            JSONObject().put("kind", "send").put("value", encodeSendSourceView(value.value))
        is CreateIntentView.Join ->
            JSONObject().put("kind", "join").put("value", encodeJoinInviteView(value.value))
    }

    private fun decodeCreateView(value: Any?, context: String): CreateView {
        val map = obj(value, context)
        knownKeys(map, setOf("intent", "request_id"), context)
        return CreateView(
            intent = decodeCreateIntentView(field(map, "intent", "CreateView.intent"), "CreateView.intent"),
            requestId = hexFixed(field(map, "request_id", "CreateView.request_id"), 32, "CreateView.request_id"),
        )
    }

    private fun encodeCreateView(value: CreateView): JSONObject {
        val map = JSONObject()
        map.put("intent", encodeCreateIntentView(value.intent))
        map.put("request_id", encodeHexFixed(value.requestId, 32, "CreateView.request_id"))
        return map
    }

    private fun decodeFrontendIntentView(value: Any?, context: String): FrontendIntentView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "command" -> FrontendIntentView.Command(
                decodeSubmitView(payload(map, "FrontendIntentView.command"), "FrontendIntentView.command"),
            )
            "create" -> FrontendIntentView.Create(
                decodeCreateView(payload(map, "FrontendIntentView.create"), "FrontendIntentView.create"),
            )
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun encodeFrontendIntentView(value: FrontendIntentView): JSONObject = when (value) {
        is FrontendIntentView.Command ->
            JSONObject().put("kind", "command").put("value", encodeSubmitView(value.value))
        is FrontendIntentView.Create ->
            JSONObject().put("kind", "create").put("value", encodeCreateView(value.value))
    }

    private fun decodeRejectionView(value: Any?, context: String): RejectionView = when (value) {
        "unknown_card" -> RejectionView.UNKNOWN_CARD
        "stale_epoch" -> RejectionView.STALE_EPOCH
        "superseded" -> RejectionView.SUPERSEDED
        "at_capacity" -> RejectionView.AT_CAPACITY
        "runtime_stopped" -> RejectionView.RUNTIME_STOPPED
        "interrupted" -> RejectionView.INTERRUPTED
        "internal" -> RejectionView.INTERNAL
        is String -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CommandContractException(CommandErrorKind.SHAPE, context)
    }

    private fun decodeAcceptanceView(value: Any?, context: String): AcceptanceView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "accepted" -> {
                unitPayload(map, "AcceptanceView.accepted")
                AcceptanceView.Accepted
            }
            "duplicate" -> AcceptanceView.Duplicate(
                decodeDispositionView(payload(map, "AcceptanceView.duplicate"), "AcceptanceView.duplicate"),
            )
            "conflict" -> AcceptanceView.Conflict(
                decodeCommandView(payload(map, "AcceptanceView.conflict"), "AcceptanceView.conflict"),
            )
            "rejected" -> AcceptanceView.Rejected(
                decodeRejectionView(payload(map, "AcceptanceView.rejected"), "AcceptanceView.rejected"),
            )
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCommandAcceptanceView(value: Any?, context: String): CommandAcceptanceView {
        val map = obj(value, context)
        knownKeys(map, setOf("command_id", "acceptance"), context)
        return CommandAcceptanceView(
            commandId = hexFixed(field(map, "command_id", "CommandAcceptanceView.command_id"), 32, "CommandAcceptanceView.command_id"),
            acceptance = decodeAcceptanceView(field(map, "acceptance", "CommandAcceptanceView.acceptance"), "CommandAcceptanceView.acceptance"),
        )
    }

    private fun decodeCompletionView(value: Any?, context: String): CompletionView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "committed" -> CompletionView.Committed(
                decodeDispositionView(payload(map, "CompletionView.committed"), "CompletionView.committed"),
            )
            "commit_failed" -> CompletionView.CommitFailed(
                decodeDispositionView(payload(map, "CompletionView.commit_failed"), "CompletionView.commit_failed"),
            )
            "interrupted" -> {
                unitPayload(map, "CompletionView.interrupted")
                CompletionView.Interrupted
            }
            "internal" -> {
                unitPayload(map, "CompletionView.internal")
                CompletionView.Internal
            }
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCommandCompletionView(value: Any?, context: String): CommandCompletionView {
        val map = obj(value, context)
        knownKeys(map, setOf("command_id", "completion"), context)
        return CommandCompletionView(
            commandId = hexFixed(field(map, "command_id", "CommandCompletionView.command_id"), 32, "CommandCompletionView.command_id"),
            completion = decodeCompletionView(field(map, "completion", "CommandCompletionView.completion"), "CommandCompletionView.completion"),
        )
    }

    private fun decodeCreateRefusalView(value: Any?, context: String): CreateRefusalView = when (value) {
        "invite_not_recognized" -> CreateRefusalView.INVITE_NOT_RECOGNIZED
        "invite_bare_room_code" -> CreateRefusalView.INVITE_BARE_ROOM_CODE
        "invite_malformed" -> CreateRefusalView.INVITE_MALFORMED
        "invite_too_long" -> CreateRefusalView.INVITE_TOO_LONG
        "invite_unsupported" -> CreateRefusalView.INVITE_UNSUPPORTED
        "invite_role_unsupported" -> CreateRefusalView.INVITE_ROLE_UNSUPPORTED
        "storage_fault" -> CreateRefusalView.STORAGE_FAULT
        "internal" -> CreateRefusalView.INTERNAL
        is String -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CommandContractException(CommandErrorKind.SHAPE, context)
    }

    private fun decodeCardCreatedView(value: Any?, context: String): CardCreatedView {
        val map = obj(value, context)
        knownKeys(map, setOf("card"), context)
        return CardCreatedView(
            card = hexFixed(field(map, "card", "CardCreatedView.card"), 16, "CardCreatedView.card"),
        )
    }

    private fun decodeCreateOutcomeView(value: Any?, context: String): CreateOutcomeView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "created" -> CreateOutcomeView.Created(
                decodeCardCreatedView(payload(map, "CreateOutcomeView.created"), "CreateOutcomeView.created"),
            )
            "refused" -> CreateOutcomeView.Refused(
                decodeCreateRefusalView(payload(map, "CreateOutcomeView.refused"), "CreateOutcomeView.refused"),
            )
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCreateResultView(value: Any?, context: String): CreateResultView {
        val map = obj(value, context)
        knownKeys(map, setOf("outcome", "request_id"), context)
        return CreateResultView(
            outcome = decodeCreateOutcomeView(field(map, "outcome", "CreateResultView.outcome"), "CreateResultView.outcome"),
            requestId = hexFixed(field(map, "request_id", "CreateResultView.request_id"), 32, "CreateResultView.request_id"),
        )
    }

    private fun decodeCommandBody(value: Any?, context: String): CommandBody {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CommandContractException(CommandErrorKind.SHAPE, context)
        return when (kind) {
            "intent" -> CommandBody.Intent(
                decodeFrontendIntentView(payload(map, "CommandBody.intent"), "CommandBody.intent"),
            )
            "acceptance" -> CommandBody.Acceptance(
                decodeCommandAcceptanceView(payload(map, "CommandBody.acceptance"), "CommandBody.acceptance"),
            )
            "completion" -> CommandBody.Completion(
                decodeCommandCompletionView(payload(map, "CommandBody.completion"), "CommandBody.completion"),
            )
            "create_result" -> CommandBody.CreateResult(
                decodeCreateResultView(payload(map, "CommandBody.create_result"), "CommandBody.create_result"),
            )
            else -> throw CommandContractException(CommandErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCommandFrame(value: Any?, context: String): CommandFrame {
        val map = obj(value, context)
        knownKeys(map, setOf("schema", "body"), context)
        return CommandFrame(
            body = decodeCommandBody(field(map, "body", "CommandFrame.body"), "CommandFrame.body"),
        )
    }
}
