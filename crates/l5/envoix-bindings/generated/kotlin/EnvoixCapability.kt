// @generated from schema/capability.schema by envoix-bindings. Do not edit;
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

package com.envoix.bindings.capability

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener

const val CAPABILITY_SCHEMA_ID: String = "envoix/binding/capability/3"
const val CAPABILITY_MAX_FRAME_BYTES: Int = 65536

enum class CapabilityErrorKind {
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
class CapabilityContractException(val kind: CapabilityErrorKind, val context: String) :
    Exception("read contract: $kind at $context")

/** Bounded contract text that redacts ordinary string interpolation. */
data class CapabilitySecretString(private val value: String) {
    fun expose(): String = value

    override fun toString(): String = "CapabilitySecretString([redacted])"
}

data class ScannedTextView(
    val text: CapabilitySecretString,
)

enum class DeclinedView {
    CANCELLED,
    REFUSED,
    UNSUPPORTED,
}

data class DeclinedReasonView(
    val reason: DeclinedView,
)

sealed interface ScanInviteStepView {
    object Requested : ScanInviteStepView
    data class Provided(val value: ScannedTextView) : ScanInviteStepView
    data class Declined(val value: DeclinedReasonView) : ScanInviteStepView
}

data class ScanInviteExchangeView(
    val step: ScanInviteStepView,
)

data class SourceAcquisitionKeyView(
    val card: String,
    val generation: Long,
    val request: String,
)

data class PickedItemView(
    val displayName: String,
    val reportedSize: Long?,
)

data class PickedSourceView(
    val items: List<PickedItemView>,
)

enum class PickSourceFailureView {
    PICKER_UNAVAILABLE,
    METADATA_UNAVAILABLE,
    INTERNAL,
}

data class PickSourceFailureReasonView(
    val reason: PickSourceFailureView,
)

sealed interface PickSourceStepView {
    object Requested : PickSourceStepView
    data class Provided(val value: PickedSourceView) : PickSourceStepView
    data class Declined(val value: DeclinedReasonView) : PickSourceStepView
    data class Failed(val value: PickSourceFailureReasonView) : PickSourceStepView
}

data class PickSourceExchangeView(
    val acquisition: SourceAcquisitionKeyView,
    val step: PickSourceStepView,
)

sealed interface CapabilityExchangeView {
    data class ScanInvite(val value: ScanInviteExchangeView) : CapabilityExchangeView
    data class PickSource(val value: PickSourceExchangeView) : CapabilityExchangeView
}

sealed interface CapabilityBody {
    data class Exchange(val value: CapabilityExchangeView) : CapabilityBody
}

data class CapabilityFrame(
    val body: CapabilityBody,
)

object EnvoixCapabilityCodec {
    /**
     * Decodes and validates one frame. Every failure is a typed
     * [CapabilityContractException]; no input, however hostile, misparses.
     */
    fun decode(text: String): CapabilityFrame {
        if (text.toByteArray(Charsets.UTF_8).size > CAPABILITY_MAX_FRAME_BYTES) {
            throw CapabilityContractException(CapabilityErrorKind.FRAME_TOO_LARGE, "CapabilityFrame")
        }
        val tokener = JSONTokener(text)
        val value = try {
            tokener.nextValue()
        } catch (exception: JSONException) {
            throw CapabilityContractException(CapabilityErrorKind.MALFORMED_JSON, "CapabilityFrame")
        }
        while (tokener.more()) {
            val trailing = tokener.next()
            if (trailing != ' ' && trailing != '\t' && trailing != '\r' && trailing != '\n') {
                throw CapabilityContractException(CapabilityErrorKind.MALFORMED_JSON, "CapabilityFrame")
            }
        }
        val map = obj(value, "CapabilityFrame")
        val schema = map.opt("schema")
        if (schema !is String) {
            throw CapabilityContractException(CapabilityErrorKind.SHAPE, "CapabilityFrame.schema")
        }
        if (schema != CAPABILITY_SCHEMA_ID) {
            throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_SCHEMA, "CapabilityFrame")
        }
        return decodeCapabilityFrame(value, "CapabilityFrame")
    }

    /**
     * Encodes the one frame a frontend may originate, stamping the schema
     * envelope and the `exchange` body around it and enforcing every bound
     * [decode] checks. Every failure is a typed [CapabilityContractException]; an
     * over-bound frame never leaves the process.
     */
    fun encode(body: CapabilityExchangeView): String {
        val map = JSONObject()
        map.put("schema", CAPABILITY_SCHEMA_ID)
        map.put(
            "body",
            JSONObject().put("kind", "exchange").put("value", encodeCapabilityExchangeView(body)),
        )
        val text = map.toString()
        if (text.toByteArray(Charsets.UTF_8).size > CAPABILITY_MAX_FRAME_BYTES) {
            throw CapabilityContractException(CapabilityErrorKind.FRAME_TOO_LARGE, "CapabilityFrame")
        }
        return text
    }

    private fun obj(value: Any?, context: String): JSONObject =
        value as? JSONObject ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)

    private fun knownKeys(map: JSONObject, allowed: Set<String>, context: String) {
        for (key in map.keys()) {
            if (key !in allowed) {
                throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_FIELD, context)
            }
        }
    }

    private fun field(map: JSONObject, key: String, context: String): Any? {
        if (!map.has(key)) {
            throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        }
        val value = map.get(key)
        return if (value == JSONObject.NULL) null else value
    }

    private fun integer(value: Any?, max: Long, context: String): Long {
        val number = when (value) {
            is Int -> value.toLong()
            is Long -> value
            else -> throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        }
        if (number < 0 || number > max) {
            throw CapabilityContractException(CapabilityErrorKind.RANGE, context)
        }
        return number
    }

    private fun hexChars(text: String): Boolean =
        text.all { it in '0'..'9' || it in 'a'..'f' }

    private fun hexFixed(value: Any?, chars: Int, context: String): String {
        if (value !is String) {
            throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        }
        if (value.length != chars || !hexChars(value)) {
            throw CapabilityContractException(CapabilityErrorKind.BOUND, context)
        }
        return value
    }

    private fun utf8Bounded(value: Any?, maxBytes: Int, context: String): String {
        if (value !is String) {
            throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        }
        // Unpaired surrogates parse here but not in the Rust reference codec;
        // reject them so every language accepts the same strings.
        var index = 0
        while (index < value.length) {
            val unit = value[index]
            if (unit.isHighSurrogate()) {
                if (index + 1 == value.length || !value[index + 1].isLowSurrogate()) {
                    throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
                }
                index += 2
            } else if (unit.isLowSurrogate()) {
                throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
            } else {
                index += 1
            }
        }
        if (value.toByteArray(Charsets.UTF_8).size > maxBytes) {
            throw CapabilityContractException(CapabilityErrorKind.BOUND, context)
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
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        if (items.length() > maxLen) {
            throw CapabilityContractException(CapabilityErrorKind.BOUND, context)
        }
        return (0 until items.length()).map { index ->
            val item = items.get(index)
            decodeElement(if (item == JSONObject.NULL) null else item, context)
        }
    }

    private fun payload(map: JSONObject, context: String): Any {
        val value = field(map, "value", context)
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        return value
    }

    private fun unitPayload(map: JSONObject, context: String) {
        if (map.has("value") && map.get("value") != JSONObject.NULL) {
            throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        }
    }

    private fun encodeInteger(value: Long, max: Long, context: String): Long =
        integer(value, max, context)

    private fun encodeHexFixed(value: String, chars: Int, context: String): String =
        hexFixed(value, chars, context)

    private fun encodeUtf8Bounded(value: String, maxBytes: Int, context: String): String =
        utf8Bounded(value, maxBytes, context)

    private fun <T> encodeList(
        value: List<T>,
        maxLen: Int,
        context: String,
        encodeElement: (T) -> Any,
    ): JSONArray {
        if (value.size > maxLen) {
            throw CapabilityContractException(CapabilityErrorKind.BOUND, context)
        }
        val items = JSONArray()
        for (item in value) {
            items.put(encodeElement(item))
        }
        return items
    }

    private fun decodeScannedTextView(value: Any?, context: String): ScannedTextView {
        val map = obj(value, context)
        knownKeys(map, setOf("text"), context)
        return ScannedTextView(
            text = CapabilitySecretString(utf8Bounded(field(map, "text", "ScannedTextView.text"), 16384, "ScannedTextView.text")),
        )
    }

    private fun encodeScannedTextView(value: ScannedTextView): JSONObject {
        val map = JSONObject()
        map.put("text", encodeUtf8Bounded(value.text.expose(), 16384, "ScannedTextView.text"))
        return map
    }

    private fun decodeDeclinedView(value: Any?, context: String): DeclinedView = when (value) {
        "cancelled" -> DeclinedView.CANCELLED
        "refused" -> DeclinedView.REFUSED
        "unsupported" -> DeclinedView.UNSUPPORTED
        is String -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
    }

    private fun encodeDeclinedView(value: DeclinedView): String = when (value) {
        DeclinedView.CANCELLED -> "cancelled"
        DeclinedView.REFUSED -> "refused"
        DeclinedView.UNSUPPORTED -> "unsupported"
    }

    private fun decodeDeclinedReasonView(value: Any?, context: String): DeclinedReasonView {
        val map = obj(value, context)
        knownKeys(map, setOf("reason"), context)
        return DeclinedReasonView(
            reason = decodeDeclinedView(field(map, "reason", "DeclinedReasonView.reason"), "DeclinedReasonView.reason"),
        )
    }

    private fun encodeDeclinedReasonView(value: DeclinedReasonView): JSONObject {
        val map = JSONObject()
        map.put("reason", encodeDeclinedView(value.reason))
        return map
    }

    private fun decodeScanInviteStepView(value: Any?, context: String): ScanInviteStepView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        return when (kind) {
            "requested" -> {
                unitPayload(map, "ScanInviteStepView.requested")
                ScanInviteStepView.Requested
            }
            "provided" -> ScanInviteStepView.Provided(
                decodeScannedTextView(payload(map, "ScanInviteStepView.provided"), "ScanInviteStepView.provided"),
            )
            "declined" -> ScanInviteStepView.Declined(
                decodeDeclinedReasonView(payload(map, "ScanInviteStepView.declined"), "ScanInviteStepView.declined"),
            )
            else -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun encodeScanInviteStepView(value: ScanInviteStepView): JSONObject = when (value) {
        is ScanInviteStepView.Requested -> JSONObject().put("kind", "requested")
        is ScanInviteStepView.Provided ->
            JSONObject().put("kind", "provided").put("value", encodeScannedTextView(value.value))
        is ScanInviteStepView.Declined ->
            JSONObject().put("kind", "declined").put("value", encodeDeclinedReasonView(value.value))
    }

    private fun decodeScanInviteExchangeView(value: Any?, context: String): ScanInviteExchangeView {
        val map = obj(value, context)
        knownKeys(map, setOf("step"), context)
        return ScanInviteExchangeView(
            step = decodeScanInviteStepView(field(map, "step", "ScanInviteExchangeView.step"), "ScanInviteExchangeView.step"),
        )
    }

    private fun encodeScanInviteExchangeView(value: ScanInviteExchangeView): JSONObject {
        val map = JSONObject()
        map.put("step", encodeScanInviteStepView(value.step))
        return map
    }

    private fun decodeSourceAcquisitionKeyView(value: Any?, context: String): SourceAcquisitionKeyView {
        val map = obj(value, context)
        knownKeys(map, setOf("card", "generation", "request"), context)
        return SourceAcquisitionKeyView(
            card = hexFixed(field(map, "card", "SourceAcquisitionKeyView.card"), 16, "SourceAcquisitionKeyView.card"),
            generation = integer(field(map, "generation", "SourceAcquisitionKeyView.generation"), 4294967295, "SourceAcquisitionKeyView.generation"),
            request = hexFixed(field(map, "request", "SourceAcquisitionKeyView.request"), 32, "SourceAcquisitionKeyView.request"),
        )
    }

    private fun encodeSourceAcquisitionKeyView(value: SourceAcquisitionKeyView): JSONObject {
        val map = JSONObject()
        map.put("card", encodeHexFixed(value.card, 16, "SourceAcquisitionKeyView.card"))
        map.put("generation", encodeInteger(value.generation, 4294967295, "SourceAcquisitionKeyView.generation"))
        map.put("request", encodeHexFixed(value.request, 32, "SourceAcquisitionKeyView.request"))
        return map
    }

    private fun decodePickedItemView(value: Any?, context: String): PickedItemView {
        val map = obj(value, context)
        knownKeys(map, setOf("display_name", "reported_size"), context)
        return PickedItemView(
            displayName = utf8Bounded(field(map, "display_name", "PickedItemView.display_name"), 1020, "PickedItemView.display_name"),
            reportedSize = field(map, "reported_size", "PickedItemView.reported_size")?.let { integer(it, Long.MAX_VALUE, "PickedItemView.reported_size") },
        )
    }

    private fun encodePickedItemView(value: PickedItemView): JSONObject {
        val map = JSONObject()
        map.put("display_name", encodeUtf8Bounded(value.displayName, 1020, "PickedItemView.display_name"))
        map.put("reported_size", value.reportedSize?.let { encodeInteger(it, Long.MAX_VALUE, "PickedItemView.reported_size") } ?: JSONObject.NULL)
        return map
    }

    private fun decodePickedSourceView(value: Any?, context: String): PickedSourceView {
        val map = obj(value, context)
        knownKeys(map, setOf("items"), context)
        return PickedSourceView(
            items = decodeList(field(map, "items", "PickedSourceView.items"), 1024, "PickedSourceView.items", ::decodePickedItemView),
        )
    }

    private fun encodePickedSourceView(value: PickedSourceView): JSONObject {
        val map = JSONObject()
        map.put("items", encodeList(value.items, 1024, "PickedSourceView.items", ::encodePickedItemView))
        return map
    }

    private fun decodePickSourceFailureView(value: Any?, context: String): PickSourceFailureView = when (value) {
        "picker_unavailable" -> PickSourceFailureView.PICKER_UNAVAILABLE
        "metadata_unavailable" -> PickSourceFailureView.METADATA_UNAVAILABLE
        "internal" -> PickSourceFailureView.INTERNAL
        is String -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        else -> throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
    }

    private fun encodePickSourceFailureView(value: PickSourceFailureView): String = when (value) {
        PickSourceFailureView.PICKER_UNAVAILABLE -> "picker_unavailable"
        PickSourceFailureView.METADATA_UNAVAILABLE -> "metadata_unavailable"
        PickSourceFailureView.INTERNAL -> "internal"
    }

    private fun decodePickSourceFailureReasonView(value: Any?, context: String): PickSourceFailureReasonView {
        val map = obj(value, context)
        knownKeys(map, setOf("reason"), context)
        return PickSourceFailureReasonView(
            reason = decodePickSourceFailureView(field(map, "reason", "PickSourceFailureReasonView.reason"), "PickSourceFailureReasonView.reason"),
        )
    }

    private fun encodePickSourceFailureReasonView(value: PickSourceFailureReasonView): JSONObject {
        val map = JSONObject()
        map.put("reason", encodePickSourceFailureView(value.reason))
        return map
    }

    private fun decodePickSourceStepView(value: Any?, context: String): PickSourceStepView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        return when (kind) {
            "requested" -> {
                unitPayload(map, "PickSourceStepView.requested")
                PickSourceStepView.Requested
            }
            "provided" -> PickSourceStepView.Provided(
                decodePickedSourceView(payload(map, "PickSourceStepView.provided"), "PickSourceStepView.provided"),
            )
            "declined" -> PickSourceStepView.Declined(
                decodeDeclinedReasonView(payload(map, "PickSourceStepView.declined"), "PickSourceStepView.declined"),
            )
            "failed" -> PickSourceStepView.Failed(
                decodePickSourceFailureReasonView(payload(map, "PickSourceStepView.failed"), "PickSourceStepView.failed"),
            )
            else -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun encodePickSourceStepView(value: PickSourceStepView): JSONObject = when (value) {
        is PickSourceStepView.Requested -> JSONObject().put("kind", "requested")
        is PickSourceStepView.Provided ->
            JSONObject().put("kind", "provided").put("value", encodePickedSourceView(value.value))
        is PickSourceStepView.Declined ->
            JSONObject().put("kind", "declined").put("value", encodeDeclinedReasonView(value.value))
        is PickSourceStepView.Failed ->
            JSONObject().put("kind", "failed").put("value", encodePickSourceFailureReasonView(value.value))
    }

    private fun decodePickSourceExchangeView(value: Any?, context: String): PickSourceExchangeView {
        val map = obj(value, context)
        knownKeys(map, setOf("acquisition", "step"), context)
        return PickSourceExchangeView(
            acquisition = decodeSourceAcquisitionKeyView(field(map, "acquisition", "PickSourceExchangeView.acquisition"), "PickSourceExchangeView.acquisition"),
            step = decodePickSourceStepView(field(map, "step", "PickSourceExchangeView.step"), "PickSourceExchangeView.step"),
        )
    }

    private fun encodePickSourceExchangeView(value: PickSourceExchangeView): JSONObject {
        val map = JSONObject()
        map.put("acquisition", encodeSourceAcquisitionKeyView(value.acquisition))
        map.put("step", encodePickSourceStepView(value.step))
        return map
    }

    private fun decodeCapabilityExchangeView(value: Any?, context: String): CapabilityExchangeView {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        return when (kind) {
            "scan_invite" -> CapabilityExchangeView.ScanInvite(
                decodeScanInviteExchangeView(payload(map, "CapabilityExchangeView.scan_invite"), "CapabilityExchangeView.scan_invite"),
            )
            "pick_source" -> CapabilityExchangeView.PickSource(
                decodePickSourceExchangeView(payload(map, "CapabilityExchangeView.pick_source"), "CapabilityExchangeView.pick_source"),
            )
            else -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun encodeCapabilityExchangeView(value: CapabilityExchangeView): JSONObject = when (value) {
        is CapabilityExchangeView.ScanInvite ->
            JSONObject().put("kind", "scan_invite").put("value", encodeScanInviteExchangeView(value.value))
        is CapabilityExchangeView.PickSource ->
            JSONObject().put("kind", "pick_source").put("value", encodePickSourceExchangeView(value.value))
    }

    private fun decodeCapabilityBody(value: Any?, context: String): CapabilityBody {
        val map = obj(value, context)
        knownKeys(map, setOf("kind", "value"), context)
        val kind = field(map, "kind", context) as? String
            ?: throw CapabilityContractException(CapabilityErrorKind.SHAPE, context)
        return when (kind) {
            "exchange" -> CapabilityBody.Exchange(
                decodeCapabilityExchangeView(payload(map, "CapabilityBody.exchange"), "CapabilityBody.exchange"),
            )
            else -> throw CapabilityContractException(CapabilityErrorKind.UNKNOWN_VARIANT, context)
        }
    }

    private fun decodeCapabilityFrame(value: Any?, context: String): CapabilityFrame {
        val map = obj(value, context)
        knownKeys(map, setOf("schema", "body"), context)
        return CapabilityFrame(
            body = decodeCapabilityBody(field(map, "body", "CapabilityFrame.body"), "CapabilityFrame.body"),
        )
    }
}
