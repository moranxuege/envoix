// @generated from schema/capability.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveats: JSON `-0` may decode as integer 0 here while the
// Rust reference codec rejects it (benign: every field with a positive
// minimum still fails its range check). Unpaired-surrogate escapes need no
// explicit scan in this artifact: a Swift `String` cannot hold them, so
// `JSONSerialization` never produces one.
// Encoding sorts object keys (a Swift dictionary has no order of its own).
// `.sortedKeys` sorts with NSString.compare under the system locale, not by
// byte value, so it agrees with the Rust reference codec's sorted map only
// for key sets whose ASCII and collation orders cannot differ — which the
// schema parser enforces for every encode-direction contract. For this
// contract's keys the emitted frame is therefore byte-identical to the
// reference bytes for the same value.

import Foundation

public enum EnvoixCapability {

public static let capabilitySchemaId = "envoix/binding/capability/2"
public static let capabilityMaxFrameBytes = 65536
private static let u63Max: Int64 = 9_223_372_036_854_775_807

public enum CapabilityErrorKind {
    case frameTooLarge
    case malformedJson
    case unknownSchema
    case shape
    case unknownField
    case unknownVariant
    case range
    case bound
}

/// Typed codec failure carrying only static schema context.
public struct CapabilityContractError: Error, Equatable {
    public let kind: CapabilityErrorKind
    public let context: String
}

/// Bounded contract text that redacts ordinary string interpolation.
public struct CapabilitySecretString: Equatable, CustomStringConvertible {
    private let value: String

    /// `public` because a separate-module consumer must be able to
    /// SEAL a value: an originable body carries secret text, so an
    /// internal initializer makes the public encoder uncallable from
    /// the app that imports these bindings.
    public init(_ value: String) { self.value = value }

    public func expose() -> String { value }

    public var description: String { "CapabilitySecretString([redacted])" }
}

public struct ScannedTextView: Equatable {
    public let text: CapabilitySecretString

    public init(text: CapabilitySecretString) {
        self.text = text
    }
}

public enum DeclinedView: String, Equatable {
    case cancelled = "cancelled"
    case refused = "refused"
    case unsupported = "unsupported"
}

public struct DeclinedReasonView: Equatable {
    public let reason: DeclinedView

    public init(reason: DeclinedView) {
        self.reason = reason
    }
}

public enum ScanInviteStepView: Equatable {
    case requested
    case provided(ScannedTextView)
    case declined(DeclinedReasonView)
}

public struct ScanInviteExchangeView: Equatable {
    public let step: ScanInviteStepView

    public init(step: ScanInviteStepView) {
        self.step = step
    }
}

public struct SourceAcquisitionKeyView: Equatable {
    public let card: String
    public let generation: Int64
    public let request: String

    public init(card: String, generation: Int64, request: String) {
        self.card = card
        self.generation = generation
        self.request = request
    }
}

public struct PickedSourceView: Equatable {
    public let displayName: String
    public let reportedSize: Int64?

    public init(displayName: String, reportedSize: Int64?) {
        self.displayName = displayName
        self.reportedSize = reportedSize
    }
}

public enum PickSourceFailureView: String, Equatable {
    case pickerUnavailable = "picker_unavailable"
    case metadataUnavailable = "metadata_unavailable"
    case `internal` = "internal"
}

public struct PickSourceFailureReasonView: Equatable {
    public let reason: PickSourceFailureView

    public init(reason: PickSourceFailureView) {
        self.reason = reason
    }
}

public enum PickSourceStepView: Equatable {
    case requested
    case provided(PickedSourceView)
    case declined(DeclinedReasonView)
    case failed(PickSourceFailureReasonView)
}

public struct PickSourceExchangeView: Equatable {
    public let acquisition: SourceAcquisitionKeyView
    public let step: PickSourceStepView

    public init(acquisition: SourceAcquisitionKeyView, step: PickSourceStepView) {
        self.acquisition = acquisition
        self.step = step
    }
}

public enum CapabilityExchangeView: Equatable {
    case scanInvite(ScanInviteExchangeView)
    case pickSource(PickSourceExchangeView)
}

public enum CapabilityBody: Equatable {
    case exchange(CapabilityExchangeView)
}

public struct CapabilityFrame: Equatable {
    public let body: CapabilityBody
}

public enum EnvoixCapabilityCodec {
    /// Decodes and validates one frame. Every failure is a typed
    /// `CapabilityContractError`; no input, however hostile, misparses.
    public static func decode(_ data: Data) throws -> CapabilityFrame {
        if data.count > capabilityMaxFrameBytes {
            throw CapabilityContractError(kind: .frameTooLarge, context: "CapabilityFrame")
        }
        let parsed: Any
        do {
            parsed = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw CapabilityContractError(kind: .malformedJson, context: "CapabilityFrame")
        }
        let map = try object(parsed, "CapabilityFrame")
        guard let schema = map["schema"] as? String else {
            throw CapabilityContractError(kind: .shape, context: "CapabilityFrame.schema")
        }
        guard schema == capabilitySchemaId else {
            throw CapabilityContractError(kind: .unknownSchema, context: "CapabilityFrame")
        }
        return try decodeCapabilityFrame(parsed, "CapabilityFrame")
    }

    /// Encodes the one frame a frontend may originate, stamping the schema
    /// envelope and the `exchange` body around it and enforcing every bound
    /// `decode` checks. Every failure is a typed `CapabilityContractError`; an
    /// over-bound frame never leaves the process.
    public static func encode(_ body: CapabilityExchangeView) throws -> Data {
        let encoded = try encodeCapabilityExchangeView(body)
        let object: [String: Any] = [
            "schema": capabilitySchemaId,
            "body": ["kind": "exchange", "value": encoded],
        ]
        guard let data = try? JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys]
        ) else {
            throw CapabilityContractError(kind: .malformedJson, context: "CapabilityFrame")
        }
        if data.count > capabilityMaxFrameBytes {
            throw CapabilityContractError(kind: .frameTooLarge, context: "CapabilityFrame")
        }
        return data
    }

    private static func object(_ value: Any?, _ context: String) throws -> [String: Any] {
        guard let map = value as? [String: Any] else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        return map
    }

    private static func knownKeys(_ map: [String: Any], _ allowed: Set<String>, _ context: String) throws {
        for key in map.keys where !allowed.contains(key) {
            throw CapabilityContractError(kind: .unknownField, context: context)
        }
    }

    private static func field(_ map: [String: Any], _ key: String, _ context: String) throws -> Any? {
        guard let value = map[key] else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        return value is NSNull ? nil : value
    }

    private static func integer(_ value: Any?, _ max: Int64, _ context: String) throws -> Int64 {
        guard let number = value as? NSNumber else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        let objCType = String(cString: number.objCType)
        if objCType == "c" || objCType == "B" || objCType == "d" || objCType == "f" {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        let wide = number.int64Value
        guard wide >= 0, wide <= max else {
            throw CapabilityContractError(kind: .range, context: context)
        }
        return wide
    }

    private static func hexChars(_ text: String) -> Bool {
        for scalar in text.unicodeScalars {
            let digit = (scalar.value >= 0x30 && scalar.value <= 0x39)
                || (scalar.value >= 0x61 && scalar.value <= 0x66)
            if !digit {
                return false
            }
        }
        return true
    }

    private static func hexFixed(_ value: Any?, _ chars: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        guard text.utf8.count == chars, hexChars(text) else {
            throw CapabilityContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func utf8Bounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        guard text.utf8.count <= maxBytes else {
            throw CapabilityContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func payload(_ map: [String: Any], _ context: String) throws -> Any {
        guard let value = map["value"], !(value is NSNull) else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        return value
    }

    private static func unitPayload(_ map: [String: Any], _ context: String) throws {
        if let value = map["value"], !(value is NSNull) {
            throw CapabilityContractError(kind: .shape, context: context)
        }
    }

    private static func encodeInteger(_ value: Int64, _ max: Int64, _ context: String) throws -> Int64 {
        return try integer(NSNumber(value: value), max, context)
    }

    private static func encodeHexFixed(_ value: String, _ chars: Int, _ context: String) throws -> String {
        return try hexFixed(value, chars, context)
    }

    private static func encodeUtf8Bounded(_ value: String, _ maxBytes: Int, _ context: String) throws -> String {
        return try utf8Bounded(value, maxBytes, context)
    }

    private static func decodeScannedTextView(_ value: Any?, _ context: String) throws -> ScannedTextView {
        let map = try object(value, context)
        try knownKeys(map, ["text"], context)
        let text = CapabilitySecretString(try utf8Bounded(try field(map, "text", "ScannedTextView.text"), 16384, "ScannedTextView.text"))
        return ScannedTextView(
            text: text
        )
    }

    private static func encodeScannedTextView(_ value: ScannedTextView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["text"] = try encodeUtf8Bounded(value.text.expose(), 16384, "ScannedTextView.text")
        return map
    }

    private static func decodeDeclinedView(_ value: Any?, _ context: String) throws -> DeclinedView {
        guard let text = value as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        guard let decoded = DeclinedView(rawValue: text) else {
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeDeclinedView(_ value: DeclinedView) -> String {
        return value.rawValue
    }

    private static func decodeDeclinedReasonView(_ value: Any?, _ context: String) throws -> DeclinedReasonView {
        let map = try object(value, context)
        try knownKeys(map, ["reason"], context)
        let reason = try decodeDeclinedView(try field(map, "reason", "DeclinedReasonView.reason"), "DeclinedReasonView.reason")
        return DeclinedReasonView(
            reason: reason
        )
    }

    private static func encodeDeclinedReasonView(_ value: DeclinedReasonView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["reason"] = encodeDeclinedView(value.reason)
        return map
    }

    private static func decodeScanInviteStepView(_ value: Any?, _ context: String) throws -> ScanInviteStepView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        switch kind {
        case "requested":
            try unitPayload(map, "ScanInviteStepView.requested")
            return .requested
        case "provided":
            return .provided(try decodeScannedTextView(payload(map, "ScanInviteStepView.provided"), "ScanInviteStepView.provided"))
        case "declined":
            return .declined(try decodeDeclinedReasonView(payload(map, "ScanInviteStepView.declined"), "ScanInviteStepView.declined"))
        default:
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodeScanInviteStepView(_ value: ScanInviteStepView) throws -> [String: Any] {
        switch value {
        case .requested:
            return ["kind": "requested"]
        case .provided(let payload):
            let encoded = try encodeScannedTextView(payload)
            return ["kind": "provided", "value": encoded]
        case .declined(let payload):
            let encoded = try encodeDeclinedReasonView(payload)
            return ["kind": "declined", "value": encoded]
        }
    }

    private static func decodeScanInviteExchangeView(_ value: Any?, _ context: String) throws -> ScanInviteExchangeView {
        let map = try object(value, context)
        try knownKeys(map, ["step"], context)
        let step = try decodeScanInviteStepView(try field(map, "step", "ScanInviteExchangeView.step"), "ScanInviteExchangeView.step")
        return ScanInviteExchangeView(
            step: step
        )
    }

    private static func encodeScanInviteExchangeView(_ value: ScanInviteExchangeView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["step"] = try encodeScanInviteStepView(value.step)
        return map
    }

    private static func decodeSourceAcquisitionKeyView(_ value: Any?, _ context: String) throws -> SourceAcquisitionKeyView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "generation", "request"], context)
        let card = try hexFixed(try field(map, "card", "SourceAcquisitionKeyView.card"), 16, "SourceAcquisitionKeyView.card")
        let generation = try integer(try field(map, "generation", "SourceAcquisitionKeyView.generation"), 4294967295, "SourceAcquisitionKeyView.generation")
        let request = try hexFixed(try field(map, "request", "SourceAcquisitionKeyView.request"), 32, "SourceAcquisitionKeyView.request")
        return SourceAcquisitionKeyView(
            card: card,
            generation: generation,
            request: request
        )
    }

    private static func encodeSourceAcquisitionKeyView(_ value: SourceAcquisitionKeyView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["card"] = try encodeHexFixed(value.card, 16, "SourceAcquisitionKeyView.card")
        map["generation"] = try encodeInteger(value.generation, 4294967295, "SourceAcquisitionKeyView.generation")
        map["request"] = try encodeHexFixed(value.request, 32, "SourceAcquisitionKeyView.request")
        return map
    }

    private static func decodePickedSourceView(_ value: Any?, _ context: String) throws -> PickedSourceView {
        let map = try object(value, context)
        try knownKeys(map, ["display_name", "reported_size"], context)
        let displayName = try utf8Bounded(try field(map, "display_name", "PickedSourceView.display_name"), 1020, "PickedSourceView.display_name")
        let reportedSize: Int64?
        if let present = try field(map, "reported_size", "PickedSourceView.reported_size") {
            reportedSize = try integer(present, u63Max, "PickedSourceView.reported_size")
        } else {
            reportedSize = nil
        }
        return PickedSourceView(
            displayName: displayName,
            reportedSize: reportedSize
        )
    }

    private static func encodePickedSourceView(_ value: PickedSourceView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["display_name"] = try encodeUtf8Bounded(value.displayName, 1020, "PickedSourceView.display_name")
        if let present = value.reportedSize {
            map["reported_size"] = try encodeInteger(present, u63Max, "PickedSourceView.reported_size")
        } else {
            map["reported_size"] = NSNull()
        }
        return map
    }

    private static func decodePickSourceFailureView(_ value: Any?, _ context: String) throws -> PickSourceFailureView {
        guard let text = value as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        guard let decoded = PickSourceFailureView(rawValue: text) else {
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodePickSourceFailureView(_ value: PickSourceFailureView) -> String {
        return value.rawValue
    }

    private static func decodePickSourceFailureReasonView(_ value: Any?, _ context: String) throws -> PickSourceFailureReasonView {
        let map = try object(value, context)
        try knownKeys(map, ["reason"], context)
        let reason = try decodePickSourceFailureView(try field(map, "reason", "PickSourceFailureReasonView.reason"), "PickSourceFailureReasonView.reason")
        return PickSourceFailureReasonView(
            reason: reason
        )
    }

    private static func encodePickSourceFailureReasonView(_ value: PickSourceFailureReasonView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["reason"] = encodePickSourceFailureView(value.reason)
        return map
    }

    private static func decodePickSourceStepView(_ value: Any?, _ context: String) throws -> PickSourceStepView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        switch kind {
        case "requested":
            try unitPayload(map, "PickSourceStepView.requested")
            return .requested
        case "provided":
            return .provided(try decodePickedSourceView(payload(map, "PickSourceStepView.provided"), "PickSourceStepView.provided"))
        case "declined":
            return .declined(try decodeDeclinedReasonView(payload(map, "PickSourceStepView.declined"), "PickSourceStepView.declined"))
        case "failed":
            return .failed(try decodePickSourceFailureReasonView(payload(map, "PickSourceStepView.failed"), "PickSourceStepView.failed"))
        default:
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodePickSourceStepView(_ value: PickSourceStepView) throws -> [String: Any] {
        switch value {
        case .requested:
            return ["kind": "requested"]
        case .provided(let payload):
            let encoded = try encodePickedSourceView(payload)
            return ["kind": "provided", "value": encoded]
        case .declined(let payload):
            let encoded = try encodeDeclinedReasonView(payload)
            return ["kind": "declined", "value": encoded]
        case .failed(let payload):
            let encoded = try encodePickSourceFailureReasonView(payload)
            return ["kind": "failed", "value": encoded]
        }
    }

    private static func decodePickSourceExchangeView(_ value: Any?, _ context: String) throws -> PickSourceExchangeView {
        let map = try object(value, context)
        try knownKeys(map, ["acquisition", "step"], context)
        let acquisition = try decodeSourceAcquisitionKeyView(try field(map, "acquisition", "PickSourceExchangeView.acquisition"), "PickSourceExchangeView.acquisition")
        let step = try decodePickSourceStepView(try field(map, "step", "PickSourceExchangeView.step"), "PickSourceExchangeView.step")
        return PickSourceExchangeView(
            acquisition: acquisition,
            step: step
        )
    }

    private static func encodePickSourceExchangeView(_ value: PickSourceExchangeView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["acquisition"] = try encodeSourceAcquisitionKeyView(value.acquisition)
        map["step"] = try encodePickSourceStepView(value.step)
        return map
    }

    private static func decodeCapabilityExchangeView(_ value: Any?, _ context: String) throws -> CapabilityExchangeView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        switch kind {
        case "scan_invite":
            return .scanInvite(try decodeScanInviteExchangeView(payload(map, "CapabilityExchangeView.scan_invite"), "CapabilityExchangeView.scan_invite"))
        case "pick_source":
            return .pickSource(try decodePickSourceExchangeView(payload(map, "CapabilityExchangeView.pick_source"), "CapabilityExchangeView.pick_source"))
        default:
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodeCapabilityExchangeView(_ value: CapabilityExchangeView) throws -> [String: Any] {
        switch value {
        case .scanInvite(let payload):
            let encoded = try encodeScanInviteExchangeView(payload)
            return ["kind": "scan_invite", "value": encoded]
        case .pickSource(let payload):
            let encoded = try encodePickSourceExchangeView(payload)
            return ["kind": "pick_source", "value": encoded]
        }
    }

    private static func decodeCapabilityBody(_ value: Any?, _ context: String) throws -> CapabilityBody {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        switch kind {
        case "exchange":
            return .exchange(try decodeCapabilityExchangeView(payload(map, "CapabilityBody.exchange"), "CapabilityBody.exchange"))
        default:
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCapabilityFrame(_ value: Any?, _ context: String) throws -> CapabilityFrame {
        let map = try object(value, context)
        try knownKeys(map, ["schema", "body"], context)
        let body = try decodeCapabilityBody(try field(map, "body", "CapabilityFrame.body"), "CapabilityFrame.body")
        return CapabilityFrame(
            body: body
        )
    }
}
}
