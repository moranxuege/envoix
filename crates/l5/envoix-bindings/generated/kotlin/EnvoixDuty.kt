// @generated from schema/duty.schema by envoix-bindings. Do not edit;
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

package com.envoix.bindings.duty

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener

const val DUTY_SCHEMA_ID: String = "envoix/binding/duty/1"
const val DUTY_MAX_FRAME_BYTES: Int = 4096

enum class DutyErrorKind {
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
class DutyContractException(val kind: DutyErrorKind, val context: String) :
    Exception("read contract: $kind at $context")

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

enum class NoticeView {
    TRANSFER_COMPLETE,
    TRANSFER_FAILED,
    ACTION_NEEDED,
}

enum class LockDirectiveView {
    HOLD,
    RELEASE,
}

data class DutyProvenanceView(
    val card: String,
    val generation: Long,
    val request: String,
)

data class PublicationWorkView(
    val staged: String,
    val displayName: String,
    val totalBytes: Long,
)

data class ForegroundWorkView(
    val activeTransfers: Long,
)

data class NotificationWorkView(
    val notice: NoticeView,
)

data class LockWorkView(
    val directive: LockDirectiveView,
)

sealed interface WorkView {
    object SourceHandle : WorkView
    object Grant : WorkView
    object Staging : WorkView
    data class Publication(val value: PublicationWorkView) : WorkView
    object Courier : WorkView
    data class Foreground(val value: ForegroundWorkView) : WorkView
    data class Notification(val value: NotificationWorkView) : WorkView
    data class Lock(val value: LockWorkView) : WorkView
    object OpenShare : WorkView
}

data class DutyOrderView(
    val provenance: DutyProvenanceView,
    val work: WorkView,
)

data class DutyReportView(
    val provenance: DutyProvenanceView,
    val outcome: OutcomeCodeView,
)

sealed interface DutyBody {
    data class Order(val value: DutyOrderView) : DutyBody
    data class Report(val value: DutyReportView) : DutyBody
}

data class DutyFrame(
    val body: DutyBody,
)

object EnvoixDutyCodec {
    /**
     * Decodes and validates one frame. Every failure is a typed
     * [DutyContractException]; no input, however hostile, misparses.
     */
    fun decode(text: String): DutyFrame {
        if (text.toByteArray(Charsets.UTF_8).size > DUTY_MAX_FRAME_BYTES) {
            throw DutyContractException(DutyErrorKind.FRAME_TOO_LARGE, "DutyFrame")
        }
        val tokener = JSONTokener(text)
        val value = try {
            tokener.nextValue()
        } catch (exception: JSONException) {
            throw DutyContractException(DutyErrorKind.MALFORMED_JSON, "DutyFrame")
        }
        while (tokener.more()) {
            val trailing = tokener.next()
            if (trailing != ' ' && trailing != '\t' && trailing != '\r' && trailing != '\n') {
                throw DutyContractException(DutyErrorKind.MALFORMED_JSON, "DutyFrame")
            }
        }
        val map = obj(value, "DutyFrame")
        val schema = map.opt("schema")
        if (schema !is String) {
            throw DutyContractException(DutyErrorKind.SHAPE, "DutyFrame.schema")
        }
        if (schema != DUTY_SCHEMA_ID) {
            throw DutyContractException(DutyErrorKind.UNKNOWN_SCHEMA, "DutyFrame")
        }
        return decodeDutyFrame(value, "DutyFrame")
    }

    /**
     * Encodes the one frame a frontend may originate, stamping the schema
     * envelope and the `report` body around it and enforcing every bound
     * [decode] checks. Every failure is a typed [DutyContractException]; an
     * over-bound frame never leaves the process.
     */
    fun encode(body: DutyReportView): String {
        val map = JSONObject()
        map.put("schema", DUTY_SCHEMA_ID)
        map.put(
            "body",
            JSONObject().put("kind", "report").put("value", encodeDutyReportView(body)),
        )
        val text = map.toString()
        if (text.toByteArray(Charsets.UTF_8).size > DUTY_MAX_FRAME_BYTES) {
            throw DutyContractException(DutyErrorKind.FRAME_TOO_LARGE, "DutyFrame")
        }
        return text
    }

    private fun obj(value: Any?, context: String): JSONObject =
        value as? JSONObject ?: throw DutyContractException(DutyErrorKind.SHAPE, context)

    private fun knownKeys(map: JSONObject, allowed: Set<String>, context: String) {
        for (key in map.keys()) {
            if (key !in allowed) {
                throw DutyContractException(DutyErrorKind.UNKNOWN_FIELD, context)
            }
        }
    }

    private fun field(map: JSONObject, key: String, context: String): Any? {
        if (!map.has(key)) {
            throw DutyContractException(DutyErrorKind.SHAPE, context)
        }
        val value = map.get(key)
        return if (value == JSONObject.NULL) null else value
    }

    private fun integer(value: Any?, max: Long, context: String): Long {
        val number = when (value) {
            is Int -> value.toLong()
            is Long -> value
            else -> throw DutyContractException(DutyErrorKind.SHAPE, context)
        }
        if (number < 0 || number > max) {
            throw DutyContractException(DutyErrorKind.RANGE, context)
        }
        return number
    }

    private fun hexChars(text: String): Boolean =
        text.all { it in '0'..'9' || it in 'a'..'f' }

    private fun hexFixed(value: Any?, chars: Int, context: String): String {
        if (value !is String) {
            throw DutyContractException(DutyErrorKind.SHAPE, context)
        }
        if (value.length != chars || !hexChars(value)) {
            throw DutyContractException(DutyErrorKind.BOUND, context)
        }
        return value
    }

    private fun utf8Bounded(value: Any?, maxBytes: Int, context: String): String {
        if (value !is String) {
            throw DutyContractException(DutyErrorKind.SHAPE, context)
        }
        // Unpaired surrogates parse here but not in the Rust reference codec;
        // reject them so every language accepts the same strings.
        var index = 0
        while (index < value.length) {
            val unit = value[index]
            if (unit.isHighSurrogate()) {
                if (index + 1 == value.length || !value[index + 1].isLowSurrogate()) {
                    throw DutyContractException(DutyErrorKind.SHAPE, context)
                }
                index += 2
            } else if (unit.isLowSurrogate()) {
                throw DutyContractException(DutyErrorKind.SHAPE, context)
            } else {
                index += 1
            }
        }
        if (value.toByteArray(Charsets.UTF_8).size > maxBytes) {
            throw DutyContractException(DutyErrorKind.BOUND, context)
        }
        return value
    }

    private fun payload(map: JSONObject, context: String): Any {
        val value = field(map, "value", context)
            ?: throw DutyContractException(DutyErrorKind.SHAPE, context)
        return value
    }

    private fun unitPayload(map: JSONObject, context: String) {
        if (map.has("value") && map.get("value") != JSONObject.NULL) {
            throw DutyContractException(DutyErrorKind.SHAPE, context)
        }
    }

    private fun encodeInteger(value: Long, max: Long, context: String): Long =
        integer(value, max, context)

    private fun encodeHexFixed(value: String, chars: Int, context: String): String =
        hexFixed(value, chars, context)

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
        is String -> throw DutyContractException(DutyErrorKind.UNKNOWN_VARIANT, context)
        else -> throw DutyContractException(DutyErrorKind.SHAPE, context)
    }

    private fun encodeOutcomeCodeView(value: OutcomeCodeView): String = when (value) {
        OutcomeCodeView.COMPLETED -> "completed"
        OutcomeCodeView.CANCELLED -> "cancelled"
        OutcomeCodeView.PAUSED -> "paused"
        OutcomeCodeView.PEER_LOST -> "peer_lost"
        OutcomeCodeView.TIMEOUT -> "timeout"
        OutcomeCodeView.UNAUTHENTICATED -> "unauthenticated"
        OutcomeCodeView.VERSION_MISMATCH -> "version_mismatch"
        OutcomeCodeView.STORAGE_FAULT -> "storage_fault"
        OutcomeCodeView.PUBLISH_FAILED -> "publish_failed"
        OutcomeCodeView.SOURCE_UNREADABLE -> "source_unreadable"
        OutcomeCodeView.NETWORK_UNREACHABLE -> "network_unreachable"
        OutcomeCodeView.INTERNAL -> "internal"
    }

    private fun decodeNoticeView(value: Any?, context: String): NoticeView = when (value) {
        "transfer_complete" -> NoticeView.TRANSFER_COMPLETE
        "transfer_failed" -> NoticeView.TRANSFER_FAILED
        "action_needed" -> NoticeView.ACTION_NEEDED
        is String -> throw DutyContractException(DutyErrorKind.UNKNOWN_VARIANT, context)
        else -> throw DutyContractException(DutyErrorKind.SHAPE, context)
    }

    private fun decodeLockDirectiveView(value: Any?, context: String): LockDirectiveView = when (value) {
        "hold" -> LockDirectiveView.HOLD
        "release" -> LockDirectiveView.RELEASE
        is String -> throw DutyContractException(DutyErrorKind.UNKNOWN_VARIANT, context)
        else -> throw DutyContractException(DutyErrorKind.SHAPE, context)
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

    private fun encodeDutyProvenanceView(value: DutyProvenanceView): JSONObject {
        val map = JSONObject()
        map.put("card", encodeHexFixed(value.card, 16, "DutyProvenanceView.card"))
        map.put("generation", encodeInteger(value.generation, 4294967295, "DutyProvenanceView.generation"))
        map.put("request", encodeHexFixed(value.request, 32, "DutyProvenanceView.request"))
        return map
    }

    private fun decodePublicationWorkView(value: Any?, context: String): PublicationWorkView {
        val map = obj(value, context)
        knownKeys(map, setOf("staged", "display_name", "total_bytes"), context)
        return PublicationWorkView(
            staged = utf8Bounded(field(map, "staged", "PublicationWorkView.staged"), 512, "PublicationWorkView.staged"),
            displayName = utf8Bounded(field(map, "display_name", "PublicationWorkView.display_name"), 255, "PublicationWorkView.display_name"),
            totalBytes = integer(field(map, "total_bytes", "PublicationWorkView.total_bytes"), Long.MAX_VALUE, "PublicationWorkView.total_bytes"),
        )
    }

    private fun decodeForegroundWorkView(value: Any?, context: String): ForegroundWorkView {
        val map = obj(value, context)
        knownKeys(map, setOf("active_transfers"), context)
        return ForegroundWorkView(
            activeTransfers = integer(field(map, "active_transfers", "ForegroundWorkView.active_transfers"), 4294967295, "ForegroundWorkView.active_transfers"),
        )
    }

    private fun decodeNotificationWorkView(value: Any?, context: String): NotificationWorkView {
        val map = obj(value, context)
        knownKeys(map, setOf("notice"), context)
        return NotificationWorkView(
            notice = decodeNoticeView(field(map, "notice", "NotificationWorkView.notice"), "NotificationWorkView.notice"),
        )
    }

    private fun decodeLockWorkView(value: Any?, context: String): LockWorkView {
        val map = obj(value, context)
        knownKeys(map, setOf("directive"), context)
        return LockWorkView(
            directive = decodeLockDirectiveView(field(map, "directive", "LockWorkView.directive"), "LockWorkView.directive"),
        )
    }

    private fun decodeWorkView(value: Any?, context: String): WorkView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw DutyContractException(DutyErrorKind.SHAPE, context)
        return when (kind) {
            "source_handle" -> {
                unitPayload(map, "WorkView.source_handle")
                WorkView.SourceHandle
            }
            "grant" -> {
                unitPayload(map, "WorkView.grant")
                WorkView.Grant
            }
            "staging" -> {
                unitPayload(map, "WorkView.staging")
                WorkView.Staging
            }
            "publication" -> WorkView.Publication(
                decodePublicationWorkView(payload(map, "WorkView.publication"), "WorkView.publication"),
            )
            "courier" -> {
                unitPayload(map, "WorkView.courier")
                WorkView.Courier
            }
            "foreground" -> WorkView.Foreground(
                decodeForegroundWorkView(payload(map, "WorkView.foreground"), "WorkView.foreground"),
            )
            "notification" -> WorkView.Notification(
                decodeNotificationWorkView(payload(map, "WorkView.notification"), "WorkView.notification"),
            )
            "lock" -> WorkView.Lock(
                decodeLockWorkView(payload(map, "WorkView.lock"), "WorkView.lock"),
            )
            "open_share" -> {
                unitPayload(map, "WorkView.open_share")
                WorkView.OpenShare
            }
            else -> throw DutyContractException(DutyErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeDutyOrderView(value: Any?, context: String): DutyOrderView {
        val map = obj(value, context)
        knownKeys(map, setOf("provenance", "work"), context)
        return DutyOrderView(
            provenance = decodeDutyProvenanceView(field(map, "provenance", "DutyOrderView.provenance"), "DutyOrderView.provenance"),
            work = decodeWorkView(field(map, "work", "DutyOrderView.work"), "DutyOrderView.work"),
        )
    }

    private fun decodeDutyReportView(value: Any?, context: String): DutyReportView {
        val map = obj(value, context)
        knownKeys(map, setOf("provenance", "outcome"), context)
        return DutyReportView(
            provenance = decodeDutyProvenanceView(field(map, "provenance", "DutyReportView.provenance"), "DutyReportView.provenance"),
            outcome = decodeOutcomeCodeView(field(map, "outcome", "DutyReportView.outcome"), "DutyReportView.outcome"),
        )
    }

    private fun encodeDutyReportView(value: DutyReportView): JSONObject {
        val map = JSONObject()
        map.put("provenance", encodeDutyProvenanceView(value.provenance))
        map.put("outcome", encodeOutcomeCodeView(value.outcome))
        return map
    }

    private fun decodeDutyBody(value: Any?, context: String): DutyBody {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw DutyContractException(DutyErrorKind.SHAPE, context)
        return when (kind) {
            "order" -> DutyBody.Order(
                decodeDutyOrderView(payload(map, "DutyBody.order"), "DutyBody.order"),
            )
            "report" -> DutyBody.Report(
                decodeDutyReportView(payload(map, "DutyBody.report"), "DutyBody.report"),
            )
            else -> throw DutyContractException(DutyErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeDutyFrame(value: Any?, context: String): DutyFrame {
        val map = obj(value, context)
        knownKeys(map, setOf("schema", "body"), context)
        return DutyFrame(
            body = decodeDutyBody(field(map, "body", "DutyFrame.body"), "DutyFrame.body"),
        )
    }
}
