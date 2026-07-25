// @generated from schema/command.schema by envoix-bindings. Do not edit;
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

public let commandSchemaId = "envoix/binding/command/1"
public let commandMaxFrameBytes = 1048576
// Contract rules frozen by schema/command.schema.
public let newestAttachmentCommands = true
public let retryHorizonCompletions = 256
public let supersessionInertPreAcceptanceOnly = true
private let u63Max: Int64 = 9_223_372_036_854_775_807

public enum CommandErrorKind {
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
public struct CommandContractError: Error, Equatable {
    public let kind: CommandErrorKind
    public let context: String
}

public enum CommandView: String, Equatable {
    case pause = "pause"
    case cancel = "cancel"
    case resume = "resume"
    case remove = "remove"
    case rePickSource = "re_pick_source"
}

public enum PauseCauseView: String, Equatable {
    case local = "local"
    case peer = "peer"
    case lost = "lost"
}

public struct PausedStateView: Equatable {
    public let origin: PauseCauseView
}

public enum DispositionView: Equatable {
    case preparing
    case waiting
    case connecting
    case verifying
    case transferring
    case confirming
    case paused(PausedStateView)
    case unconfirmed
    case completed
    case failed
    case cancelled
}

public struct SubmitView: Equatable {
    public let card: String
    public let epoch: Int64
    public let commandId: String
    public let command: CommandView

    public init(card: String, epoch: Int64, commandId: String, command: CommandView) {
        self.card = card
        self.epoch = epoch
        self.commandId = commandId
        self.command = command
    }
}

public enum RejectionView: String, Equatable {
    case unknownCard = "unknown_card"
    case staleEpoch = "stale_epoch"
    case superseded = "superseded"
    case atCapacity = "at_capacity"
    case runtimeStopped = "runtime_stopped"
    case interrupted = "interrupted"
    case conflict = "conflict"
    case `internal` = "internal"
}

public enum AcceptanceView: Equatable {
    case accepted
    case duplicate(DispositionView)
    case rejected(RejectionView)
}

public struct CommandAcceptanceView: Equatable {
    public let commandId: String
    public let acceptance: AcceptanceView
}

public enum CompletionView: Equatable {
    case committed(DispositionView)
    case commitFailed(DispositionView)
    case interrupted
    case `internal`
}

public struct CommandCompletionView: Equatable {
    public let commandId: String
    public let completion: CompletionView
}

public enum CommandBody: Equatable {
    case submit(SubmitView)
    case acceptance(CommandAcceptanceView)
    case completion(CommandCompletionView)
}

public struct CommandFrame: Equatable {
    public let body: CommandBody
}

public enum EnvoixCommandCodec {
    /// Decodes and validates one frame. Every failure is a typed
    /// `CommandContractError`; no input, however hostile, misparses.
    public static func decode(_ data: Data) throws -> CommandFrame {
        if data.count > commandMaxFrameBytes {
            throw CommandContractError(kind: .frameTooLarge, context: "CommandFrame")
        }
        let parsed: Any
        do {
            parsed = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw CommandContractError(kind: .malformedJson, context: "CommandFrame")
        }
        let map = try object(parsed, "CommandFrame")
        guard let schema = map["schema"] as? String else {
            throw CommandContractError(kind: .shape, context: "CommandFrame.schema")
        }
        guard schema == commandSchemaId else {
            throw CommandContractError(kind: .unknownSchema, context: "CommandFrame")
        }
        return try decodeCommandFrame(parsed, "CommandFrame")
    }

    /// Encodes the one frame a frontend may originate, stamping the schema
    /// envelope and the `submit` body around it and enforcing every bound
    /// `decode` checks. Every failure is a typed `CommandContractError`; an
    /// over-bound frame never leaves the process.
    public static func encode(_ body: SubmitView) throws -> Data {
        let encoded = try encodeSubmitView(body)
        let object: [String: Any] = [
            "schema": commandSchemaId,
            "body": ["kind": "submit", "value": encoded],
        ]
        guard let data = try? JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys]
        ) else {
            throw CommandContractError(kind: .malformedJson, context: "CommandFrame")
        }
        if data.count > commandMaxFrameBytes {
            throw CommandContractError(kind: .frameTooLarge, context: "CommandFrame")
        }
        return data
    }

    private static func object(_ value: Any?, _ context: String) throws -> [String: Any] {
        guard let map = value as? [String: Any] else {
            throw CommandContractError(kind: .shape, context: context)
        }
        return map
    }

    private static func knownKeys(_ map: [String: Any], _ allowed: Set<String>, _ context: String) throws {
        for key in map.keys where !allowed.contains(key) {
            throw CommandContractError(kind: .unknownField, context: context)
        }
    }

    private static func field(_ map: [String: Any], _ key: String, _ context: String) throws -> Any? {
        guard let value = map[key] else {
            throw CommandContractError(kind: .shape, context: context)
        }
        return value is NSNull ? nil : value
    }

    private static func integer(_ value: Any?, _ max: Int64, _ context: String) throws -> Int64 {
        guard let number = value as? NSNumber else {
            throw CommandContractError(kind: .shape, context: context)
        }
        let objCType = String(cString: number.objCType)
        if objCType == "c" || objCType == "B" || objCType == "d" || objCType == "f" {
            throw CommandContractError(kind: .shape, context: context)
        }
        let wide = number.int64Value
        guard wide >= 0, wide <= max else {
            throw CommandContractError(kind: .range, context: context)
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
            throw CommandContractError(kind: .shape, context: context)
        }
        guard text.utf8.count == chars, hexChars(text) else {
            throw CommandContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func payload(_ map: [String: Any], _ context: String) throws -> Any {
        guard let value = map["value"], !(value is NSNull) else {
            throw CommandContractError(kind: .shape, context: context)
        }
        return value
    }

    private static func unitPayload(_ map: [String: Any], _ context: String) throws {
        if let value = map["value"], !(value is NSNull) {
            throw CommandContractError(kind: .shape, context: context)
        }
    }

    private static func encodeInteger(_ value: Int64, _ max: Int64, _ context: String) throws -> Int64 {
        return try integer(NSNumber(value: value), max, context)
    }

    private static func encodeHexFixed(_ value: String, _ chars: Int, _ context: String) throws -> String {
        return try hexFixed(value, chars, context)
    }

    private static func decodeCommandView(_ value: Any?, _ context: String) throws -> CommandView {
        guard let text = value as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        guard let decoded = CommandView(rawValue: text) else {
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeCommandView(_ value: CommandView) -> String {
        return value.rawValue
    }

    private static func decodePauseCauseView(_ value: Any?, _ context: String) throws -> PauseCauseView {
        guard let text = value as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        guard let decoded = PauseCauseView(rawValue: text) else {
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodePausedStateView(_ value: Any?, _ context: String) throws -> PausedStateView {
        let map = try object(value, context)
        try knownKeys(map, ["origin"], context)
        let origin = try decodePauseCauseView(try field(map, "origin", "PausedStateView.origin"), "PausedStateView.origin")
        return PausedStateView(
            origin: origin
        )
    }

    private static func decodeDispositionView(_ value: Any?, _ context: String) throws -> DispositionView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        switch kind {
        case "preparing":
            try unitPayload(map, "DispositionView.preparing")
            return .preparing
        case "waiting":
            try unitPayload(map, "DispositionView.waiting")
            return .waiting
        case "connecting":
            try unitPayload(map, "DispositionView.connecting")
            return .connecting
        case "verifying":
            try unitPayload(map, "DispositionView.verifying")
            return .verifying
        case "transferring":
            try unitPayload(map, "DispositionView.transferring")
            return .transferring
        case "confirming":
            try unitPayload(map, "DispositionView.confirming")
            return .confirming
        case "paused":
            return .paused(try decodePausedStateView(payload(map, "DispositionView.paused"), "DispositionView.paused"))
        case "unconfirmed":
            try unitPayload(map, "DispositionView.unconfirmed")
            return .unconfirmed
        case "completed":
            try unitPayload(map, "DispositionView.completed")
            return .completed
        case "failed":
            try unitPayload(map, "DispositionView.failed")
            return .failed
        case "cancelled":
            try unitPayload(map, "DispositionView.cancelled")
            return .cancelled
        default:
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeSubmitView(_ value: Any?, _ context: String) throws -> SubmitView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "epoch", "command_id", "command"], context)
        let card = try hexFixed(try field(map, "card", "SubmitView.card"), 16, "SubmitView.card")
        let epoch = try integer(try field(map, "epoch", "SubmitView.epoch"), u63Max, "SubmitView.epoch")
        let commandId = try hexFixed(try field(map, "command_id", "SubmitView.command_id"), 32, "SubmitView.command_id")
        let command = try decodeCommandView(try field(map, "command", "SubmitView.command"), "SubmitView.command")
        return SubmitView(
            card: card,
            epoch: epoch,
            commandId: commandId,
            command: command
        )
    }

    private static func encodeSubmitView(_ value: SubmitView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["card"] = try encodeHexFixed(value.card, 16, "SubmitView.card")
        map["epoch"] = try encodeInteger(value.epoch, u63Max, "SubmitView.epoch")
        map["command_id"] = try encodeHexFixed(value.commandId, 32, "SubmitView.command_id")
        map["command"] = encodeCommandView(value.command)
        return map
    }

    private static func decodeRejectionView(_ value: Any?, _ context: String) throws -> RejectionView {
        guard let text = value as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        guard let decoded = RejectionView(rawValue: text) else {
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeAcceptanceView(_ value: Any?, _ context: String) throws -> AcceptanceView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        switch kind {
        case "accepted":
            try unitPayload(map, "AcceptanceView.accepted")
            return .accepted
        case "duplicate":
            return .duplicate(try decodeDispositionView(payload(map, "AcceptanceView.duplicate"), "AcceptanceView.duplicate"))
        case "rejected":
            return .rejected(try decodeRejectionView(payload(map, "AcceptanceView.rejected"), "AcceptanceView.rejected"))
        default:
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCommandAcceptanceView(_ value: Any?, _ context: String) throws -> CommandAcceptanceView {
        let map = try object(value, context)
        try knownKeys(map, ["command_id", "acceptance"], context)
        let commandId = try hexFixed(try field(map, "command_id", "CommandAcceptanceView.command_id"), 32, "CommandAcceptanceView.command_id")
        let acceptance = try decodeAcceptanceView(try field(map, "acceptance", "CommandAcceptanceView.acceptance"), "CommandAcceptanceView.acceptance")
        return CommandAcceptanceView(
            commandId: commandId,
            acceptance: acceptance
        )
    }

    private static func decodeCompletionView(_ value: Any?, _ context: String) throws -> CompletionView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        switch kind {
        case "committed":
            return .committed(try decodeDispositionView(payload(map, "CompletionView.committed"), "CompletionView.committed"))
        case "commit_failed":
            return .commitFailed(try decodeDispositionView(payload(map, "CompletionView.commit_failed"), "CompletionView.commit_failed"))
        case "interrupted":
            try unitPayload(map, "CompletionView.interrupted")
            return .interrupted
        case "internal":
            try unitPayload(map, "CompletionView.internal")
            return .`internal`
        default:
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCommandCompletionView(_ value: Any?, _ context: String) throws -> CommandCompletionView {
        let map = try object(value, context)
        try knownKeys(map, ["command_id", "completion"], context)
        let commandId = try hexFixed(try field(map, "command_id", "CommandCompletionView.command_id"), 32, "CommandCompletionView.command_id")
        let completion = try decodeCompletionView(try field(map, "completion", "CommandCompletionView.completion"), "CommandCompletionView.completion")
        return CommandCompletionView(
            commandId: commandId,
            completion: completion
        )
    }

    private static func decodeCommandBody(_ value: Any?, _ context: String) throws -> CommandBody {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw CommandContractError(kind: .shape, context: context)
        }
        switch kind {
        case "submit":
            return .submit(try decodeSubmitView(payload(map, "CommandBody.submit"), "CommandBody.submit"))
        case "acceptance":
            return .acceptance(try decodeCommandAcceptanceView(payload(map, "CommandBody.acceptance"), "CommandBody.acceptance"))
        case "completion":
            return .completion(try decodeCommandCompletionView(payload(map, "CommandBody.completion"), "CommandBody.completion"))
        default:
            throw CommandContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCommandFrame(_ value: Any?, _ context: String) throws -> CommandFrame {
        let map = try object(value, context)
        try knownKeys(map, ["schema", "body"], context)
        let body = try decodeCommandBody(try field(map, "body", "CommandFrame.body"), "CommandFrame.body")
        return CommandFrame(
            body: body
        )
    }
}
