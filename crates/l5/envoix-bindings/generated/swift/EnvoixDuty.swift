// @generated from schema/duty.schema by envoix-bindings. Do not edit;
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

public enum EnvoixDuty {

public static let dutySchemaId = "envoix/binding/duty/4"
public static let dutyMaxFrameBytes = 131072
private static let u63Max: Int64 = 9_223_372_036_854_775_807

public enum DutyErrorKind {
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
public struct DutyContractError: Error, Equatable {
    public let kind: DutyErrorKind
    public let context: String
}

public enum OutcomeCodeView: String, Equatable {
    case completed = "completed"
    case cancelled = "cancelled"
    case paused = "paused"
    case peerLost = "peer_lost"
    case timeout = "timeout"
    case unauthenticated = "unauthenticated"
    case versionMismatch = "version_mismatch"
    case storageFault = "storage_fault"
    case storageFull = "storage_full"
    case publishFailed = "publish_failed"
    case sourceUnreadable = "source_unreadable"
    case networkUnreachable = "network_unreachable"
    case `internal` = "internal"
}

public enum NoticeView: String, Equatable {
    case transferComplete = "transfer_complete"
    case transferFailed = "transfer_failed"
    case actionNeeded = "action_needed"
}

public enum LockDirectiveView: String, Equatable {
    case hold = "hold"
    case release = "release"
}

public struct DutyProvenanceView: Equatable {
    public let card: String
    public let generation: Int64
    public let request: String

    public init(card: String, generation: Int64, request: String) {
        self.card = card
        self.generation = generation
        self.request = request
    }
}

public struct PublicationWorkView: Equatable {
    public let staged: String
    public let displayName: String
    public let totalBytes: Int64
}

public struct ForegroundWorkView: Equatable {
    public let activeTransfers: Int64
}

public struct NotificationWorkView: Equatable {
    public let notice: NoticeView
}

public struct LockWorkView: Equatable {
    public let directive: LockDirectiveView
}

public enum WorkView: Equatable {
    case sourceHandle
    case grant
    case staging
    case publication(PublicationWorkView)
    case courier
    case foreground(ForegroundWorkView)
    case notification(NotificationWorkView)
    case lock(LockWorkView)
    case openShare
}

public struct DutyOrderView: Equatable {
    public let provenance: DutyProvenanceView
    public let work: WorkView
}

public enum SourceRetentionView: String, Equatable {
    case process = "process"
    case persisted = "persisted"
}

public enum SourceSeekabilityView: String, Equatable {
    case seekable = "seekable"
    case sequentialOnly = "sequential_only"
}

public struct AcquiredItemView: Equatable {
    public let item: Int64
    public let retention: SourceRetentionView
    public let seekability: SourceSeekabilityView

    public init(item: Int64, retention: SourceRetentionView, seekability: SourceSeekabilityView) {
        self.item = item
        self.retention = retention
        self.seekability = seekability
    }
}

public struct SourceAcquiredView: Equatable {
    public let items: [AcquiredItemView]

    public init(items: [AcquiredItemView]) {
        self.items = items
    }
}

public enum SourceFailureView: String, Equatable {
    case unreadable = "unreadable"
    case permissionLost = "permission_lost"
    case storageFault = "storage_fault"
    case `internal` = "internal"
}

public struct SourceFailedView: Equatable {
    public let reason: SourceFailureView

    public init(reason: SourceFailureView) {
        self.reason = reason
    }
}

public enum SourceReportView: Equatable {
    case acquired(SourceAcquiredView)
    case failed(SourceFailedView)
}

public enum DutyAnswerView: Equatable {
    case outcome(OutcomeCodeView)
    case source(SourceReportView)
}

public struct DutyReportView: Equatable {
    public let provenance: DutyProvenanceView
    public let answer: DutyAnswerView

    public init(provenance: DutyProvenanceView, answer: DutyAnswerView) {
        self.provenance = provenance
        self.answer = answer
    }
}

public enum DutyBody: Equatable {
    case order(DutyOrderView)
    case report(DutyReportView)
}

public struct DutyFrame: Equatable {
    public let body: DutyBody
}

public enum EnvoixDutyCodec {
    /// Decodes and validates one frame. Every failure is a typed
    /// `DutyContractError`; no input, however hostile, misparses.
    public static func decode(_ data: Data) throws -> DutyFrame {
        if data.count > dutyMaxFrameBytes {
            throw DutyContractError(kind: .frameTooLarge, context: "DutyFrame")
        }
        let parsed: Any
        do {
            parsed = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw DutyContractError(kind: .malformedJson, context: "DutyFrame")
        }
        let map = try object(parsed, "DutyFrame")
        guard let schema = map["schema"] as? String else {
            throw DutyContractError(kind: .shape, context: "DutyFrame.schema")
        }
        guard schema == dutySchemaId else {
            throw DutyContractError(kind: .unknownSchema, context: "DutyFrame")
        }
        return try decodeDutyFrame(parsed, "DutyFrame")
    }

    /// Encodes the one frame a frontend may originate, stamping the schema
    /// envelope and the `report` body around it and enforcing every bound
    /// `decode` checks. Every failure is a typed `DutyContractError`; an
    /// over-bound frame never leaves the process.
    public static func encode(_ body: DutyReportView) throws -> Data {
        let encoded = try encodeDutyReportView(body)
        let object: [String: Any] = [
            "schema": dutySchemaId,
            "body": ["kind": "report", "value": encoded],
        ]
        guard let data = try? JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys]
        ) else {
            throw DutyContractError(kind: .malformedJson, context: "DutyFrame")
        }
        if data.count > dutyMaxFrameBytes {
            throw DutyContractError(kind: .frameTooLarge, context: "DutyFrame")
        }
        return data
    }

    private static func object(_ value: Any?, _ context: String) throws -> [String: Any] {
        guard let map = value as? [String: Any] else {
            throw DutyContractError(kind: .shape, context: context)
        }
        return map
    }

    private static func knownKeys(_ map: [String: Any], _ allowed: Set<String>, _ context: String) throws {
        for key in map.keys where !allowed.contains(key) {
            throw DutyContractError(kind: .unknownField, context: context)
        }
    }

    private static func field(_ map: [String: Any], _ key: String, _ context: String) throws -> Any? {
        guard let value = map[key] else {
            throw DutyContractError(kind: .shape, context: context)
        }
        return value is NSNull ? nil : value
    }

    private static func integer(_ value: Any?, _ max: Int64, _ context: String) throws -> Int64 {
        guard let number = value as? NSNumber else {
            throw DutyContractError(kind: .shape, context: context)
        }
        let objCType = String(cString: number.objCType)
        if objCType == "c" || objCType == "B" || objCType == "d" || objCType == "f" {
            throw DutyContractError(kind: .shape, context: context)
        }
        let wide = number.int64Value
        guard wide >= 0, wide <= max else {
            throw DutyContractError(kind: .range, context: context)
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
            throw DutyContractError(kind: .shape, context: context)
        }
        guard text.utf8.count == chars, hexChars(text) else {
            throw DutyContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func utf8Bounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard text.utf8.count <= maxBytes else {
            throw DutyContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func decodeList<T>(
        _ value: Any?,
        _ maxLen: Int,
        _ context: String,
        _ decodeElement: (Any?, String) throws -> T
    ) throws -> [T] {
        guard let items = value as? [Any] else {
            throw DutyContractError(kind: .shape, context: context)
        }
        if items.count > maxLen {
            throw DutyContractError(kind: .bound, context: context)
        }
        return try items.map { try decodeElement($0 is NSNull ? nil : $0, context) }
    }

    private static func payload(_ map: [String: Any], _ context: String) throws -> Any {
        guard let value = map["value"], !(value is NSNull) else {
            throw DutyContractError(kind: .shape, context: context)
        }
        return value
    }

    private static func unitPayload(_ map: [String: Any], _ context: String) throws {
        if let value = map["value"], !(value is NSNull) {
            throw DutyContractError(kind: .shape, context: context)
        }
    }

    private static func encodeInteger(_ value: Int64, _ max: Int64, _ context: String) throws -> Int64 {
        return try integer(NSNumber(value: value), max, context)
    }

    private static func encodeHexFixed(_ value: String, _ chars: Int, _ context: String) throws -> String {
        return try hexFixed(value, chars, context)
    }

    private static func encodeList<T>(
        _ value: [T],
        _ maxLen: Int,
        _ context: String,
        _ encodeElement: (T) throws -> Any
    ) throws -> [Any] {
        if value.count > maxLen {
            throw DutyContractError(kind: .bound, context: context)
        }
        return try value.map(encodeElement)
    }

    private static func decodeOutcomeCodeView(_ value: Any?, _ context: String) throws -> OutcomeCodeView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = OutcomeCodeView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeOutcomeCodeView(_ value: OutcomeCodeView) -> String {
        return value.rawValue
    }

    private static func decodeNoticeView(_ value: Any?, _ context: String) throws -> NoticeView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = NoticeView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeLockDirectiveView(_ value: Any?, _ context: String) throws -> LockDirectiveView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = LockDirectiveView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeDutyProvenanceView(_ value: Any?, _ context: String) throws -> DutyProvenanceView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "generation", "request"], context)
        let card = try hexFixed(try field(map, "card", "DutyProvenanceView.card"), 16, "DutyProvenanceView.card")
        let generation = try integer(try field(map, "generation", "DutyProvenanceView.generation"), 4294967295, "DutyProvenanceView.generation")
        let request = try hexFixed(try field(map, "request", "DutyProvenanceView.request"), 32, "DutyProvenanceView.request")
        return DutyProvenanceView(
            card: card,
            generation: generation,
            request: request
        )
    }

    private static func encodeDutyProvenanceView(_ value: DutyProvenanceView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["card"] = try encodeHexFixed(value.card, 16, "DutyProvenanceView.card")
        map["generation"] = try encodeInteger(value.generation, 4294967295, "DutyProvenanceView.generation")
        map["request"] = try encodeHexFixed(value.request, 32, "DutyProvenanceView.request")
        return map
    }

    private static func decodePublicationWorkView(_ value: Any?, _ context: String) throws -> PublicationWorkView {
        let map = try object(value, context)
        try knownKeys(map, ["staged", "display_name", "total_bytes"], context)
        let staged = try utf8Bounded(try field(map, "staged", "PublicationWorkView.staged"), 512, "PublicationWorkView.staged")
        let displayName = try utf8Bounded(try field(map, "display_name", "PublicationWorkView.display_name"), 255, "PublicationWorkView.display_name")
        let totalBytes = try integer(try field(map, "total_bytes", "PublicationWorkView.total_bytes"), u63Max, "PublicationWorkView.total_bytes")
        return PublicationWorkView(
            staged: staged,
            displayName: displayName,
            totalBytes: totalBytes
        )
    }

    private static func decodeForegroundWorkView(_ value: Any?, _ context: String) throws -> ForegroundWorkView {
        let map = try object(value, context)
        try knownKeys(map, ["active_transfers"], context)
        let activeTransfers = try integer(try field(map, "active_transfers", "ForegroundWorkView.active_transfers"), 4294967295, "ForegroundWorkView.active_transfers")
        return ForegroundWorkView(
            activeTransfers: activeTransfers
        )
    }

    private static func decodeNotificationWorkView(_ value: Any?, _ context: String) throws -> NotificationWorkView {
        let map = try object(value, context)
        try knownKeys(map, ["notice"], context)
        let notice = try decodeNoticeView(try field(map, "notice", "NotificationWorkView.notice"), "NotificationWorkView.notice")
        return NotificationWorkView(
            notice: notice
        )
    }

    private static func decodeLockWorkView(_ value: Any?, _ context: String) throws -> LockWorkView {
        let map = try object(value, context)
        try knownKeys(map, ["directive"], context)
        let directive = try decodeLockDirectiveView(try field(map, "directive", "LockWorkView.directive"), "LockWorkView.directive")
        return LockWorkView(
            directive: directive
        )
    }

    private static func decodeWorkView(_ value: Any?, _ context: String) throws -> WorkView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        switch kind {
        case "source_handle":
            try unitPayload(map, "WorkView.source_handle")
            return .sourceHandle
        case "grant":
            try unitPayload(map, "WorkView.grant")
            return .grant
        case "staging":
            try unitPayload(map, "WorkView.staging")
            return .staging
        case "publication":
            return .publication(try decodePublicationWorkView(payload(map, "WorkView.publication"), "WorkView.publication"))
        case "courier":
            try unitPayload(map, "WorkView.courier")
            return .courier
        case "foreground":
            return .foreground(try decodeForegroundWorkView(payload(map, "WorkView.foreground"), "WorkView.foreground"))
        case "notification":
            return .notification(try decodeNotificationWorkView(payload(map, "WorkView.notification"), "WorkView.notification"))
        case "lock":
            return .lock(try decodeLockWorkView(payload(map, "WorkView.lock"), "WorkView.lock"))
        case "open_share":
            try unitPayload(map, "WorkView.open_share")
            return .openShare
        default:
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeDutyOrderView(_ value: Any?, _ context: String) throws -> DutyOrderView {
        let map = try object(value, context)
        try knownKeys(map, ["provenance", "work"], context)
        let provenance = try decodeDutyProvenanceView(try field(map, "provenance", "DutyOrderView.provenance"), "DutyOrderView.provenance")
        let work = try decodeWorkView(try field(map, "work", "DutyOrderView.work"), "DutyOrderView.work")
        return DutyOrderView(
            provenance: provenance,
            work: work
        )
    }

    private static func decodeSourceRetentionView(_ value: Any?, _ context: String) throws -> SourceRetentionView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = SourceRetentionView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeSourceRetentionView(_ value: SourceRetentionView) -> String {
        return value.rawValue
    }

    private static func decodeSourceSeekabilityView(_ value: Any?, _ context: String) throws -> SourceSeekabilityView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = SourceSeekabilityView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeSourceSeekabilityView(_ value: SourceSeekabilityView) -> String {
        return value.rawValue
    }

    private static func decodeAcquiredItemView(_ value: Any?, _ context: String) throws -> AcquiredItemView {
        let map = try object(value, context)
        try knownKeys(map, ["item", "retention", "seekability"], context)
        let item = try integer(try field(map, "item", "AcquiredItemView.item"), 4294967295, "AcquiredItemView.item")
        let retention = try decodeSourceRetentionView(try field(map, "retention", "AcquiredItemView.retention"), "AcquiredItemView.retention")
        let seekability = try decodeSourceSeekabilityView(try field(map, "seekability", "AcquiredItemView.seekability"), "AcquiredItemView.seekability")
        return AcquiredItemView(
            item: item,
            retention: retention,
            seekability: seekability
        )
    }

    private static func encodeAcquiredItemView(_ value: AcquiredItemView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["item"] = try encodeInteger(value.item, 4294967295, "AcquiredItemView.item")
        map["retention"] = encodeSourceRetentionView(value.retention)
        map["seekability"] = encodeSourceSeekabilityView(value.seekability)
        return map
    }

    private static func decodeSourceAcquiredView(_ value: Any?, _ context: String) throws -> SourceAcquiredView {
        let map = try object(value, context)
        try knownKeys(map, ["items"], context)
        let items = try decodeList(try field(map, "items", "SourceAcquiredView.items"), 1024, "SourceAcquiredView.items", decodeAcquiredItemView)
        return SourceAcquiredView(
            items: items
        )
    }

    private static func encodeSourceAcquiredView(_ value: SourceAcquiredView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["items"] = try encodeList(value.items, 1024, "SourceAcquiredView.items", encodeAcquiredItemView)
        return map
    }

    private static func decodeSourceFailureView(_ value: Any?, _ context: String) throws -> SourceFailureView {
        guard let text = value as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        guard let decoded = SourceFailureView(rawValue: text) else {
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func encodeSourceFailureView(_ value: SourceFailureView) -> String {
        return value.rawValue
    }

    private static func decodeSourceFailedView(_ value: Any?, _ context: String) throws -> SourceFailedView {
        let map = try object(value, context)
        try knownKeys(map, ["reason"], context)
        let reason = try decodeSourceFailureView(try field(map, "reason", "SourceFailedView.reason"), "SourceFailedView.reason")
        return SourceFailedView(
            reason: reason
        )
    }

    private static func encodeSourceFailedView(_ value: SourceFailedView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["reason"] = encodeSourceFailureView(value.reason)
        return map
    }

    private static func decodeSourceReportView(_ value: Any?, _ context: String) throws -> SourceReportView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        switch kind {
        case "acquired":
            return .acquired(try decodeSourceAcquiredView(payload(map, "SourceReportView.acquired"), "SourceReportView.acquired"))
        case "failed":
            return .failed(try decodeSourceFailedView(payload(map, "SourceReportView.failed"), "SourceReportView.failed"))
        default:
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodeSourceReportView(_ value: SourceReportView) throws -> [String: Any] {
        switch value {
        case .acquired(let payload):
            let encoded = try encodeSourceAcquiredView(payload)
            return ["kind": "acquired", "value": encoded]
        case .failed(let payload):
            let encoded = try encodeSourceFailedView(payload)
            return ["kind": "failed", "value": encoded]
        }
    }

    private static func decodeDutyAnswerView(_ value: Any?, _ context: String) throws -> DutyAnswerView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        switch kind {
        case "outcome":
            return .outcome(try decodeOutcomeCodeView(payload(map, "DutyAnswerView.outcome"), "DutyAnswerView.outcome"))
        case "source":
            return .source(try decodeSourceReportView(payload(map, "DutyAnswerView.source"), "DutyAnswerView.source"))
        default:
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func encodeDutyAnswerView(_ value: DutyAnswerView) throws -> [String: Any] {
        switch value {
        case .outcome(let payload):
            let encoded = encodeOutcomeCodeView(payload)
            return ["kind": "outcome", "value": encoded]
        case .source(let payload):
            let encoded = try encodeSourceReportView(payload)
            return ["kind": "source", "value": encoded]
        }
    }

    private static func decodeDutyReportView(_ value: Any?, _ context: String) throws -> DutyReportView {
        let map = try object(value, context)
        try knownKeys(map, ["provenance", "answer"], context)
        let provenance = try decodeDutyProvenanceView(try field(map, "provenance", "DutyReportView.provenance"), "DutyReportView.provenance")
        let answer = try decodeDutyAnswerView(try field(map, "answer", "DutyReportView.answer"), "DutyReportView.answer")
        return DutyReportView(
            provenance: provenance,
            answer: answer
        )
    }

    private static func encodeDutyReportView(_ value: DutyReportView) throws -> [String: Any] {
        var map: [String: Any] = [:]
        map["provenance"] = try encodeDutyProvenanceView(value.provenance)
        map["answer"] = try encodeDutyAnswerView(value.answer)
        return map
    }

    private static func decodeDutyBody(_ value: Any?, _ context: String) throws -> DutyBody {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw DutyContractError(kind: .shape, context: context)
        }
        switch kind {
        case "order":
            return .order(try decodeDutyOrderView(payload(map, "DutyBody.order"), "DutyBody.order"))
        case "report":
            return .report(try decodeDutyReportView(payload(map, "DutyBody.report"), "DutyBody.report"))
        default:
            throw DutyContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeDutyFrame(_ value: Any?, _ context: String) throws -> DutyFrame {
        let map = try object(value, context)
        try knownKeys(map, ["schema", "body"], context)
        let body = try decodeDutyBody(try field(map, "body", "DutyFrame.body"), "DutyFrame.body")
        return DutyFrame(
            body: body
        )
    }
}
}
