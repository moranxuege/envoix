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

public static let capabilitySchemaId = "envoix/binding/capability/1"
public static let capabilityMaxFrameBytes = 65536
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

    init(_ value: String) { self.value = value }

    public func expose() -> String { value }

    public var description: String { "CapabilitySecretString([redacted])" }
}

public enum CapabilityRequestView: String, Equatable {
    case scanInvite = "scan_invite"
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

public enum CapabilityStepView: Equatable {
    case requested
    case provided(ScannedTextView)
    case declined(DeclinedReasonView)
}

public struct CapabilityExchangeView: Equatable {
    public let capability: CapabilityRequestView
    public let step: CapabilityStepView

    public init(capability: CapabilityRequestView, step: CapabilityStepView) {
        self.capability = capability
        self.step = step
    }
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

    private static func encodeUtf8Bounded(_ value: String, _ maxBytes: Int, _ context: String) throws -> String {
        return try utf8Bounded(value, maxBytes, context)
    }

    private static func decodeCapabilityRequestView(_ value: Any?, _ context: String) throws -> CapabilityRequestView {
        guard let text = value as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        guard let decoded = CapabilityRequestView(rawValue: text) else {
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeCapabilityRequestView(_ value: CapabilityRequestView) -> String {
        return value.rawValue
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

    private static func decodeCapabilityStepView(_ value: Any?, _ context: String) throws -> CapabilityStepView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CapabilityContractError(kind: .shape, context: context)
        }
        switch kind {
        case "requested":
            try unitPayload(map, "CapabilityStepView.requested")
            return .requested
        case "provided":
            return .provided(try decodeScannedTextView(payload(map, "CapabilityStepView.provided"), "CapabilityStepView.provided"))
        case "declined":
            return .declined(try decodeDeclinedReasonView(payload(map, "CapabilityStepView.declined"), "CapabilityStepView.declined"))
        default:
            throw CapabilityContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodeCapabilityStepView(_ value: CapabilityStepView) throws -> [String: Any] {
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

    private static func decodeCapabilityExchangeView(_ value: Any?, _ context: String) throws -> CapabilityExchangeView {
        let map = try object(value, context)
        try knownKeys(map, ["capability", "step"], context)
        let capability = try decodeCapabilityRequestView(try field(map, "capability", "CapabilityExchangeView.capability"), "CapabilityExchangeView.capability")
        let step = try decodeCapabilityStepView(try field(map, "step", "CapabilityExchangeView.step"), "CapabilityExchangeView.step")
        return CapabilityExchangeView(
            capability: capability,
            step: step
        )
    }

    private static func encodeCapabilityExchangeView(_ value: CapabilityExchangeView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["capability"] = encodeCapabilityRequestView(value.capability)
        map["step"] = try encodeCapabilityStepView(value.step)
        return map
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
