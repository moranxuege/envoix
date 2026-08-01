// @generated from schema/read.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveats: JSON `-0` may decode as integer 0 here while the
// Rust reference codec rejects it (benign: every field with a positive
// minimum still fails its range check). Unpaired-surrogate escapes need no
// explicit scan in this artifact: a Swift `String` cannot hold them, so
// `JSONSerialization` never produces one.

import Foundation

public enum EnvoixRead {

public static let readSchemaId = "envoix/binding/read/11"
public static let readMaxFrameBytes = 1048576
private static let u63Max: Int64 = 9_223_372_036_854_775_807

public enum ReadErrorKind {
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
public struct ReadContractError: Error, Equatable {
    public let kind: ReadErrorKind
    public let context: String
}

/// Bounded contract text that redacts ordinary string interpolation.
public struct ReadSecretString: Equatable, CustomStringConvertible {
    private let value: String

    /// `public` because a separate-module consumer must be able to
    /// SEAL a value: an originable body carries secret text, so an
    /// internal initializer makes the public encoder uncallable from
    /// the app that imports these bindings.
    public init(_ value: String) { self.value = value }

    public func expose() -> String { value }

    public var description: String { "ReadSecretString([redacted])" }
}

public enum DirectionView: String, Equatable {
    case send = "send"
    case receive = "receive"
}

public enum PhaseView: String, Equatable {
    case preparing = "preparing"
    case pairing = "pairing"
    case authenticating = "authenticating"
    case transferring = "transferring"
    case confirming = "confirming"
    case publishing = "publishing"
    case restoring = "restoring"
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

public enum RetryabilityView: String, Equatable {
    case retryable = "retryable"
    case terminal = "terminal"
    case needsUser = "needs_user"
}

public enum RecoveryView: String, Equatable {
    case rePickSource = "re_pick_source"
    case retryLater = "retry_later"
    case reconnectPeer = "reconnect_peer"
}

public enum PauseOriginView: String, Equatable {
    case local = "local"
    case peer = "peer"
    case lost = "lost"
}

public enum WorkerKindView: String, Equatable {
    case attempt = "attempt"
    case staging = "staging"
}

public enum RetirementIntentView: String, Equatable {
    case pause = "pause"
    case cancel = "cancel"
    case finalize = "finalize"
}

public enum DutyKindView: String, Equatable {
    case sourceHandle = "source_handle"
    case grant = "grant"
    case staging = "staging"
    case publication = "publication"
    case courier = "courier"
    case foreground = "foreground"
    case notification = "notification"
    case lock = "lock"
    case openShare = "open_share"
}

public enum CapabilityActionView: String, Equatable {
    case postReceipt = "post_receipt"
    case acquireSource = "acquire_source"
}

public enum CommandKindView: String, Equatable {
    case pause = "pause"
    case cancel = "cancel"
    case resume = "resume"
    case remove = "remove"
    case rePickSource = "re_pick_source"
}

public enum RedactedIdKindView: String, Equatable {
    case record = "record"
    case transfer = "transfer"
    case artifact = "artifact"
    case request = "request"
}

public enum LosslessKindView: String, Equatable {
    case terminal = "terminal"
    case capabilityDuty = "capability_duty"
}

public enum SubscribeRejectionView: String, Equatable {
    case unknownCard = "unknown_card"
    case runtimeStopped = "runtime_stopped"
    case epochExhausted = "epoch_exhausted"
}

public struct OutcomeView: Equatable {
    public let code: OutcomeCodeView
    public let phase: PhaseView
    public let retry: RetryabilityView
    public let recovery: RecoveryView?
    public let display: String
}

public struct PausedView: Equatable {
    public let origin: PauseOriginView
}

public enum ProductStateView: Equatable {
    case preparing
    case waiting
    case connecting
    case verifying
    case transferring
    case confirming
    case paused(PausedView)
    case unconfirmed
    case completed
    case failed
    case cancelled
}

public struct RunningView: Equatable {
    public let worker: WorkerKindView
}

public struct RetiringView: Equatable {
    public let worker: WorkerKindView
    public let intent: RetirementIntentView
}

public enum QuiescenceView: Equatable {
    case running(RunningView)
    case retiring(RetiringView)
    case quiescent
}

public struct IdentityView: Equatable {
    public let card: String
    public let transfer: String
    public let artifact: String
}

public struct QrView: Equatable {
    public let width: Int64
    public let modules: ReadSecretString
}

public struct InviteView: Equatable {
    public let code: ReadSecretString
    public let codeFingerprint: String
    public let link: ReadSecretString?
    public let qr: QrView?
}

public enum RoomParticipationView: String, Equatable {
    case minted = "minted"
    case joined = "joined"
}

public struct SourceAcquisitionKeyView: Equatable {
    public let card: String
    public let generation: Int64
    public let request: String
}

public enum SourcePromptReasonView: String, Equatable {
    case initial = "initial"
    case unreadable = "unreadable"
    case permissionLost = "permission_lost"
    case storageFault = "storage_fault"
    case stagingFailed = "staging_failed"
    case `internal` = "internal"
}

public struct AcceptedSourceOfferView: Equatable {
    public let acquisition: SourceAcquisitionKeyView
    public let displayName: String
    public let reportedSize: Int64?
}

public struct TransferContentView: Equatable {
    public let offeredName: String
    public let total: Int64
}

public struct ContentReplacedView: Equatable {
    public let previous: TransferContentView
    public let count: Int64
}

public struct SourceNotRequiredView: Equatable {
    public let peerContent: TransferContentView?
}

public struct SourceSelectableView: Equatable {
    public let acquisition: SourceAcquisitionKeyView
    public let reason: SourcePromptReasonView
}

public struct SourceRePickRequiredView: Equatable {
    public let reason: SourcePromptReasonView
    public let previousOffer: AcceptedSourceOfferView
}

public enum SourceSelectionGateView: Equatable {
    case selectable(SourceSelectableView)
    case rePickRequired(SourceRePickRequiredView)
}

public struct SourceAwaitingSelectionView: Equatable {
    public let selection: SourceSelectionGateView
}

public struct SourceReadyView: Equatable {
    public let offer: AcceptedSourceOfferView
    public let content: TransferContentView
}

public enum SourceLifecycleView: Equatable {
    case notRequired(SourceNotRequiredView)
    case awaitingSelection(SourceAwaitingSelectionView)
    case acquiring(AcceptedSourceOfferView)
    case staging(AcceptedSourceOfferView)
    case ready(SourceReadyView)
}

public struct PickSourceActionView: Equatable {
    public let acquisition: SourceAcquisitionKeyView
}

public enum CardActionView: Equatable {
    case command(CommandKindView)
    case pickSource(PickSourceActionView)
}

public struct CardView: Equatable {
    public let identity: IdentityView
    public let participation: RoomParticipationView
    public let direction: DirectionView
    public let source: SourceLifecycleView
    public let state: ProductStateView
    public let quiescence: QuiescenceView
    public let generation: Int64
    public let phase: PhaseView
    public let bytes: Int64
    public let bytesResumed: Int64
    public let outcome: OutcomeView?
    public let allowedActions: [CardActionView]
    public let invite: InviteView?
    public let contentReplaced: ContentReplacedView?
}

public struct DutyProvenanceView: Equatable {
    public let card: String
    public let generation: Int64
    public let request: String
}

public struct DutyView: Equatable {
    public let provenance: DutyProvenanceView
    public let kind: DutyKindView
}

public struct DutyFrameView: Equatable {
    public let duty: DutyView
    public let action: CapabilityActionView
}

public enum CardUpdateKindView: Equatable {
    case snapshot(CardView)
    case progress(CardView)
    case state(CardView)
    case terminal(CardView)
    case capabilityDuty(DutyFrameView)
}

public struct CardUpdateView: Equatable {
    public let epoch: Int64
    public let card: String
    public let kind: CardUpdateKindView
}

public struct LagView: Equatable {
    public let epoch: Int64
    public let card: String
    public let missed: LosslessKindView
}

public struct ClosedView: Equatable {
    public let epoch: Int64
    public let card: String
}

public struct SubscribeRejectedView: Equatable {
    public let card: String
    public let reason: SubscribeRejectionView
}

public struct SessionKeyView: Equatable {
    public let card: String
    public let generation: Int64
}

public struct EvidenceProgressView: Equatable {
    public let transferred: Int64
    public let total: Int64
}

public struct RedactedIdView: Equatable {
    public let kind: RedactedIdKindView
}

public enum EvidenceValueView: Equatable {
    case phase(PhaseView)
    case progress(EvidenceProgressView)
    case outcome(OutcomeView)
    case identifier(RedactedIdView)
}

public struct DegradedView: Equatable {
    public let droppedEvents: Int64
}

public enum DiagnosticsStatusView: Equatable {
    case complete
    case degraded(DegradedView)
}

public struct TimelineEntryView: Equatable {
    public let sequence: Int64
    public let value: EvidenceValueView
}

public struct EvidenceTimelineView: Equatable {
    public let session: SessionKeyView
    public let status: DiagnosticsStatusView
    public let entries: [TimelineEntryView]
}

public struct ProtocolManifestView: Equatable {
    public let setId: String
    public let dataAlpn: String
    public let dataMagic: String
    public let dataWireVersion: Int64
}

public struct AbiSchemaManifestView: Equatable {
    public let readBindingSchemaId: String
    public let commandBindingSchemaId: String
    public let capabilityBindingSchemaId: String
    public let evidenceRustAbiId: String
    public let evidenceTimelineSchemaId: String
    public let mailboxReceiptSchemaId: String
    public let operationEnvelopeSchemaId: String
}

public struct DeploymentManifestView: Equatable {
    public let environment: String
    public let rendezvousEndpoint: String
    public let relayUrl: String
}

public struct BuildManifestView: Equatable {
    public let packageVersion: String
    public let `protocol`: ProtocolManifestView
    public let abiSchema: AbiSchemaManifestView
    public let deployment: DeploymentManifestView
}

public enum ReadBody: Equatable {
    case cardUpdate(CardUpdateView)
    case lag(LagView)
    case closed(ClosedView)
    case subscribeRejected(SubscribeRejectedView)
    case evidence(EvidenceTimelineView)
    case buildManifest(BuildManifestView)
}

public struct ReadFrame: Equatable {
    public let body: ReadBody
}

public enum GateDecision {
    case deliver
    case dropStale
    case contractBreach
}

/// Client-side admission for the per-epoch card stream: one gate per
/// attachment. Frames from another epoch are stale; every epoch starts
/// with a snapshot; a lag or close ends the epoch permanently.
public struct EpochGate {
    private let epoch: Int64
    private var sawSnapshot = false
    private var dead = false

    public init(attach epoch: Int64) {
        self.epoch = epoch
    }

    public mutating func admit(_ frame: ReadFrame) -> GateDecision {
        switch frame.body {
        case .cardUpdate(let update):
            if update.epoch != epoch || dead {
                return .dropStale
            }
            if case .snapshot = update.kind {
                if sawSnapshot {
                    return .contractBreach
                }
                sawSnapshot = true
                return .deliver
            }
            return sawSnapshot ? .deliver : .contractBreach
        case .lag(let lag):
            return terminate(lag.epoch)
        case .closed(let closed):
            return terminate(closed.epoch)
        default:
            return .deliver
        }
    }

    private mutating func terminate(_ frameEpoch: Int64) -> GateDecision {
        if frameEpoch == epoch && !dead {
            dead = true
            return .deliver
        }
        return .dropStale
    }
}

public enum EnvoixReadCodec {
    /// Decodes and validates one frame. Every failure is a typed
    /// `ReadContractError`; no input, however hostile, misparses.
    public static func decode(_ data: Data) throws -> ReadFrame {
        if data.count > readMaxFrameBytes {
            throw ReadContractError(kind: .frameTooLarge, context: "ReadFrame")
        }
        let parsed: Any
        do {
            parsed = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw ReadContractError(kind: .malformedJson, context: "ReadFrame")
        }
        let map = try object(parsed, "ReadFrame")
        guard let schema = map["schema"] as? String else {
            throw ReadContractError(kind: .shape, context: "ReadFrame.schema")
        }
        guard schema == readSchemaId else {
            throw ReadContractError(kind: .unknownSchema, context: "ReadFrame")
        }
        return try decodeReadFrame(parsed, "ReadFrame")
    }

    private static func object(_ value: Any?, _ context: String) throws -> [String: Any] {
        guard let map = value as? [String: Any] else {
            throw ReadContractError(kind: .shape, context: context)
        }
        return map
    }

    private static func knownKeys(_ map: [String: Any], _ allowed: Set<String>, _ context: String) throws {
        for key in map.keys where !allowed.contains(key) {
            throw ReadContractError(kind: .unknownField, context: context)
        }
    }

    private static func field(_ map: [String: Any], _ key: String, _ context: String) throws -> Any? {
        guard let value = map[key] else {
            throw ReadContractError(kind: .shape, context: context)
        }
        return value is NSNull ? nil : value
    }

    private static func integer(_ value: Any?, _ max: Int64, _ context: String) throws -> Int64 {
        guard let number = value as? NSNumber else {
            throw ReadContractError(kind: .shape, context: context)
        }
        let objCType = String(cString: number.objCType)
        if objCType == "c" || objCType == "B" || objCType == "d" || objCType == "f" {
            throw ReadContractError(kind: .shape, context: context)
        }
        let wide = number.int64Value
        guard wide >= 0, wide <= max else {
            throw ReadContractError(kind: .range, context: context)
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
            throw ReadContractError(kind: .shape, context: context)
        }
        guard text.utf8.count == chars, hexChars(text) else {
            throw ReadContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func hexVariable(_ value: Any?, _ maxChars: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        let length = text.utf8.count
        let valid = length > 0 && length % 2 == 0 && length <= maxChars && hexChars(text)
        guard valid else {
            throw ReadContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func utf8Bounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard text.utf8.count <= maxBytes else {
            throw ReadContractError(kind: .bound, context: context)
        }
        return text
    }

    private static func asciiBounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        let valid = text.utf8.count <= maxBytes
            && text.unicodeScalars.allSatisfy { $0.value >= 0x20 && $0.value <= 0x7e }
        guard valid else {
            throw ReadContractError(kind: .bound, context: context)
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
            throw ReadContractError(kind: .shape, context: context)
        }
        if items.count > maxLen {
            throw ReadContractError(kind: .bound, context: context)
        }
        return try items.map { try decodeElement($0 is NSNull ? nil : $0, context) }
    }

    private static func payload(_ map: [String: Any], _ context: String) throws -> Any {
        guard let value = map["value"], !(value is NSNull) else {
            throw ReadContractError(kind: .shape, context: context)
        }
        return value
    }

    private static func unitPayload(_ map: [String: Any], _ context: String) throws {
        if let value = map["value"], !(value is NSNull) {
            throw ReadContractError(kind: .shape, context: context)
        }
    }

    private static func decodeDirectionView(_ value: Any?, _ context: String) throws -> DirectionView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = DirectionView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodePhaseView(_ value: Any?, _ context: String) throws -> PhaseView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = PhaseView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeOutcomeCodeView(_ value: Any?, _ context: String) throws -> OutcomeCodeView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = OutcomeCodeView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeRetryabilityView(_ value: Any?, _ context: String) throws -> RetryabilityView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = RetryabilityView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeRecoveryView(_ value: Any?, _ context: String) throws -> RecoveryView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = RecoveryView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodePauseOriginView(_ value: Any?, _ context: String) throws -> PauseOriginView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = PauseOriginView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeWorkerKindView(_ value: Any?, _ context: String) throws -> WorkerKindView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = WorkerKindView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeRetirementIntentView(_ value: Any?, _ context: String) throws -> RetirementIntentView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = RetirementIntentView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeDutyKindView(_ value: Any?, _ context: String) throws -> DutyKindView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = DutyKindView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeCapabilityActionView(_ value: Any?, _ context: String) throws -> CapabilityActionView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = CapabilityActionView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeCommandKindView(_ value: Any?, _ context: String) throws -> CommandKindView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = CommandKindView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeRedactedIdKindView(_ value: Any?, _ context: String) throws -> RedactedIdKindView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = RedactedIdKindView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeLosslessKindView(_ value: Any?, _ context: String) throws -> LosslessKindView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = LosslessKindView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeSubscribeRejectionView(_ value: Any?, _ context: String) throws -> SubscribeRejectionView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = SubscribeRejectionView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeOutcomeView(_ value: Any?, _ context: String) throws -> OutcomeView {
        let map = try object(value, context)
        try knownKeys(map, ["code", "phase", "retry", "recovery", "display"], context)
        let code = try decodeOutcomeCodeView(try field(map, "code", "OutcomeView.code"), "OutcomeView.code")
        let phase = try decodePhaseView(try field(map, "phase", "OutcomeView.phase"), "OutcomeView.phase")
        let retry = try decodeRetryabilityView(try field(map, "retry", "OutcomeView.retry"), "OutcomeView.retry")
        let recovery: RecoveryView?
        if let present = try field(map, "recovery", "OutcomeView.recovery") {
            recovery = try decodeRecoveryView(present, "OutcomeView.recovery")
        } else {
            recovery = nil
        }
        let display = try utf8Bounded(try field(map, "display", "OutcomeView.display"), 160, "OutcomeView.display")
        return OutcomeView(
            code: code,
            phase: phase,
            retry: retry,
            recovery: recovery,
            display: display
        )
    }

    private static func decodePausedView(_ value: Any?, _ context: String) throws -> PausedView {
        let map = try object(value, context)
        try knownKeys(map, ["origin"], context)
        let origin = try decodePauseOriginView(try field(map, "origin", "PausedView.origin"), "PausedView.origin")
        return PausedView(
            origin: origin
        )
    }

    private static func decodeProductStateView(_ value: Any?, _ context: String) throws -> ProductStateView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "preparing":
            try unitPayload(map, "ProductStateView.preparing")
            return .preparing
        case "waiting":
            try unitPayload(map, "ProductStateView.waiting")
            return .waiting
        case "connecting":
            try unitPayload(map, "ProductStateView.connecting")
            return .connecting
        case "verifying":
            try unitPayload(map, "ProductStateView.verifying")
            return .verifying
        case "transferring":
            try unitPayload(map, "ProductStateView.transferring")
            return .transferring
        case "confirming":
            try unitPayload(map, "ProductStateView.confirming")
            return .confirming
        case "paused":
            return .paused(try decodePausedView(payload(map, "ProductStateView.paused"), "ProductStateView.paused"))
        case "unconfirmed":
            try unitPayload(map, "ProductStateView.unconfirmed")
            return .unconfirmed
        case "completed":
            try unitPayload(map, "ProductStateView.completed")
            return .completed
        case "failed":
            try unitPayload(map, "ProductStateView.failed")
            return .failed
        case "cancelled":
            try unitPayload(map, "ProductStateView.cancelled")
            return .cancelled
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeRunningView(_ value: Any?, _ context: String) throws -> RunningView {
        let map = try object(value, context)
        try knownKeys(map, ["worker"], context)
        let worker = try decodeWorkerKindView(try field(map, "worker", "RunningView.worker"), "RunningView.worker")
        return RunningView(
            worker: worker
        )
    }

    private static func decodeRetiringView(_ value: Any?, _ context: String) throws -> RetiringView {
        let map = try object(value, context)
        try knownKeys(map, ["worker", "intent"], context)
        let worker = try decodeWorkerKindView(try field(map, "worker", "RetiringView.worker"), "RetiringView.worker")
        let intent = try decodeRetirementIntentView(try field(map, "intent", "RetiringView.intent"), "RetiringView.intent")
        return RetiringView(
            worker: worker,
            intent: intent
        )
    }

    private static func decodeQuiescenceView(_ value: Any?, _ context: String) throws -> QuiescenceView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "running":
            return .running(try decodeRunningView(payload(map, "QuiescenceView.running"), "QuiescenceView.running"))
        case "retiring":
            return .retiring(try decodeRetiringView(payload(map, "QuiescenceView.retiring"), "QuiescenceView.retiring"))
        case "quiescent":
            try unitPayload(map, "QuiescenceView.quiescent")
            return .quiescent
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeIdentityView(_ value: Any?, _ context: String) throws -> IdentityView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "transfer", "artifact"], context)
        let card = try hexFixed(try field(map, "card", "IdentityView.card"), 16, "IdentityView.card")
        let transfer = try hexFixed(try field(map, "transfer", "IdentityView.transfer"), 32, "IdentityView.transfer")
        let artifact = try hexFixed(try field(map, "artifact", "IdentityView.artifact"), 32, "IdentityView.artifact")
        return IdentityView(
            card: card,
            transfer: transfer,
            artifact: artifact
        )
    }

    private static func decodeQrView(_ value: Any?, _ context: String) throws -> QrView {
        let map = try object(value, context)
        try knownKeys(map, ["width", "modules"], context)
        let width = try integer(try field(map, "width", "QrView.width"), 65535, "QrView.width")
        let modules = ReadSecretString(try hexVariable(try field(map, "modules", "QrView.modules"), 7834, "QrView.modules"))
        return QrView(
            width: width,
            modules: modules
        )
    }

    private static func decodeInviteView(_ value: Any?, _ context: String) throws -> InviteView {
        let map = try object(value, context)
        try knownKeys(map, ["code", "code_fingerprint", "link", "qr"], context)
        let code = ReadSecretString(try utf8Bounded(try field(map, "code", "InviteView.code"), 64, "InviteView.code"))
        let codeFingerprint = try hexFixed(try field(map, "code_fingerprint", "InviteView.code_fingerprint"), 16, "InviteView.code_fingerprint")
        let link: ReadSecretString?
        if let present = try field(map, "link", "InviteView.link") {
            link = ReadSecretString(try utf8Bounded(present, 5481, "InviteView.link"))
        } else {
            link = nil
        }
        let qr: QrView?
        if let present = try field(map, "qr", "InviteView.qr") {
            qr = try decodeQrView(present, "InviteView.qr")
        } else {
            qr = nil
        }
        return InviteView(
            code: code,
            codeFingerprint: codeFingerprint,
            link: link,
            qr: qr
        )
    }

    private static func decodeRoomParticipationView(_ value: Any?, _ context: String) throws -> RoomParticipationView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = RoomParticipationView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
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

    private static func decodeSourcePromptReasonView(_ value: Any?, _ context: String) throws -> SourcePromptReasonView {
        guard let text = value as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        guard let decoded = SourcePromptReasonView(rawValue: text) else {
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
        return decoded
    }

    private static func decodeAcceptedSourceOfferView(_ value: Any?, _ context: String) throws -> AcceptedSourceOfferView {
        let map = try object(value, context)
        try knownKeys(map, ["acquisition", "display_name", "reported_size"], context)
        let acquisition = try decodeSourceAcquisitionKeyView(try field(map, "acquisition", "AcceptedSourceOfferView.acquisition"), "AcceptedSourceOfferView.acquisition")
        let displayName = try utf8Bounded(try field(map, "display_name", "AcceptedSourceOfferView.display_name"), 255, "AcceptedSourceOfferView.display_name")
        let reportedSize: Int64?
        if let present = try field(map, "reported_size", "AcceptedSourceOfferView.reported_size") {
            reportedSize = try integer(present, u63Max, "AcceptedSourceOfferView.reported_size")
        } else {
            reportedSize = nil
        }
        return AcceptedSourceOfferView(
            acquisition: acquisition,
            displayName: displayName,
            reportedSize: reportedSize
        )
    }

    private static func decodeTransferContentView(_ value: Any?, _ context: String) throws -> TransferContentView {
        let map = try object(value, context)
        try knownKeys(map, ["offered_name", "total"], context)
        let offeredName = try utf8Bounded(try field(map, "offered_name", "TransferContentView.offered_name"), 255, "TransferContentView.offered_name")
        let total = try integer(try field(map, "total", "TransferContentView.total"), u63Max, "TransferContentView.total")
        return TransferContentView(
            offeredName: offeredName,
            total: total
        )
    }

    private static func decodeContentReplacedView(_ value: Any?, _ context: String) throws -> ContentReplacedView {
        let map = try object(value, context)
        try knownKeys(map, ["previous", "count"], context)
        let previous = try decodeTransferContentView(try field(map, "previous", "ContentReplacedView.previous"), "ContentReplacedView.previous")
        let count = try integer(try field(map, "count", "ContentReplacedView.count"), 4294967295, "ContentReplacedView.count")
        return ContentReplacedView(
            previous: previous,
            count: count
        )
    }

    private static func decodeSourceNotRequiredView(_ value: Any?, _ context: String) throws -> SourceNotRequiredView {
        let map = try object(value, context)
        try knownKeys(map, ["peer_content"], context)
        let peerContent: TransferContentView?
        if let present = try field(map, "peer_content", "SourceNotRequiredView.peer_content") {
            peerContent = try decodeTransferContentView(present, "SourceNotRequiredView.peer_content")
        } else {
            peerContent = nil
        }
        return SourceNotRequiredView(
            peerContent: peerContent
        )
    }

    private static func decodeSourceSelectableView(_ value: Any?, _ context: String) throws -> SourceSelectableView {
        let map = try object(value, context)
        try knownKeys(map, ["acquisition", "reason"], context)
        let acquisition = try decodeSourceAcquisitionKeyView(try field(map, "acquisition", "SourceSelectableView.acquisition"), "SourceSelectableView.acquisition")
        let reason = try decodeSourcePromptReasonView(try field(map, "reason", "SourceSelectableView.reason"), "SourceSelectableView.reason")
        return SourceSelectableView(
            acquisition: acquisition,
            reason: reason
        )
    }

    private static func decodeSourceRePickRequiredView(_ value: Any?, _ context: String) throws -> SourceRePickRequiredView {
        let map = try object(value, context)
        try knownKeys(map, ["reason", "previous_offer"], context)
        let reason = try decodeSourcePromptReasonView(try field(map, "reason", "SourceRePickRequiredView.reason"), "SourceRePickRequiredView.reason")
        let previousOffer = try decodeAcceptedSourceOfferView(try field(map, "previous_offer", "SourceRePickRequiredView.previous_offer"), "SourceRePickRequiredView.previous_offer")
        return SourceRePickRequiredView(
            reason: reason,
            previousOffer: previousOffer
        )
    }

    private static func decodeSourceSelectionGateView(_ value: Any?, _ context: String) throws -> SourceSelectionGateView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "selectable":
            return .selectable(try decodeSourceSelectableView(payload(map, "SourceSelectionGateView.selectable"), "SourceSelectionGateView.selectable"))
        case "re_pick_required":
            return .rePickRequired(try decodeSourceRePickRequiredView(payload(map, "SourceSelectionGateView.re_pick_required"), "SourceSelectionGateView.re_pick_required"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeSourceAwaitingSelectionView(_ value: Any?, _ context: String) throws -> SourceAwaitingSelectionView {
        let map = try object(value, context)
        try knownKeys(map, ["selection"], context)
        let selection = try decodeSourceSelectionGateView(try field(map, "selection", "SourceAwaitingSelectionView.selection"), "SourceAwaitingSelectionView.selection")
        return SourceAwaitingSelectionView(
            selection: selection
        )
    }

    private static func decodeSourceReadyView(_ value: Any?, _ context: String) throws -> SourceReadyView {
        let map = try object(value, context)
        try knownKeys(map, ["offer", "content"], context)
        let offer = try decodeAcceptedSourceOfferView(try field(map, "offer", "SourceReadyView.offer"), "SourceReadyView.offer")
        let content = try decodeTransferContentView(try field(map, "content", "SourceReadyView.content"), "SourceReadyView.content")
        return SourceReadyView(
            offer: offer,
            content: content
        )
    }

    private static func decodeSourceLifecycleView(_ value: Any?, _ context: String) throws -> SourceLifecycleView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "not_required":
            return .notRequired(try decodeSourceNotRequiredView(payload(map, "SourceLifecycleView.not_required"), "SourceLifecycleView.not_required"))
        case "awaiting_selection":
            return .awaitingSelection(try decodeSourceAwaitingSelectionView(payload(map, "SourceLifecycleView.awaiting_selection"), "SourceLifecycleView.awaiting_selection"))
        case "acquiring":
            return .acquiring(try decodeAcceptedSourceOfferView(payload(map, "SourceLifecycleView.acquiring"), "SourceLifecycleView.acquiring"))
        case "staging":
            return .staging(try decodeAcceptedSourceOfferView(payload(map, "SourceLifecycleView.staging"), "SourceLifecycleView.staging"))
        case "ready":
            return .ready(try decodeSourceReadyView(payload(map, "SourceLifecycleView.ready"), "SourceLifecycleView.ready"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodePickSourceActionView(_ value: Any?, _ context: String) throws -> PickSourceActionView {
        let map = try object(value, context)
        try knownKeys(map, ["acquisition"], context)
        let acquisition = try decodeSourceAcquisitionKeyView(try field(map, "acquisition", "PickSourceActionView.acquisition"), "PickSourceActionView.acquisition")
        return PickSourceActionView(
            acquisition: acquisition
        )
    }

    private static func decodeCardActionView(_ value: Any?, _ context: String) throws -> CardActionView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "command":
            return .command(try decodeCommandKindView(payload(map, "CardActionView.command"), "CardActionView.command"))
        case "pick_source":
            return .pickSource(try decodePickSourceActionView(payload(map, "CardActionView.pick_source"), "CardActionView.pick_source"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCardView(_ value: Any?, _ context: String) throws -> CardView {
        let map = try object(value, context)
        try knownKeys(map, ["identity", "participation", "direction", "source", "state", "quiescence", "generation", "phase", "bytes", "bytes_resumed", "outcome", "allowed_actions", "invite", "content_replaced"], context)
        let identity = try decodeIdentityView(try field(map, "identity", "CardView.identity"), "CardView.identity")
        let participation = try decodeRoomParticipationView(try field(map, "participation", "CardView.participation"), "CardView.participation")
        let direction = try decodeDirectionView(try field(map, "direction", "CardView.direction"), "CardView.direction")
        let source = try decodeSourceLifecycleView(try field(map, "source", "CardView.source"), "CardView.source")
        let state = try decodeProductStateView(try field(map, "state", "CardView.state"), "CardView.state")
        let quiescence = try decodeQuiescenceView(try field(map, "quiescence", "CardView.quiescence"), "CardView.quiescence")
        let generation = try integer(try field(map, "generation", "CardView.generation"), 4294967295, "CardView.generation")
        let phase = try decodePhaseView(try field(map, "phase", "CardView.phase"), "CardView.phase")
        let bytes = try integer(try field(map, "bytes", "CardView.bytes"), u63Max, "CardView.bytes")
        let bytesResumed = try integer(try field(map, "bytes_resumed", "CardView.bytes_resumed"), u63Max, "CardView.bytes_resumed")
        let outcome: OutcomeView?
        if let present = try field(map, "outcome", "CardView.outcome") {
            outcome = try decodeOutcomeView(present, "CardView.outcome")
        } else {
            outcome = nil
        }
        let allowedActions = try decodeList(try field(map, "allowed_actions", "CardView.allowed_actions"), 6, "CardView.allowed_actions", decodeCardActionView)
        let invite: InviteView?
        if let present = try field(map, "invite", "CardView.invite") {
            invite = try decodeInviteView(present, "CardView.invite")
        } else {
            invite = nil
        }
        let contentReplaced: ContentReplacedView?
        if let present = try field(map, "content_replaced", "CardView.content_replaced") {
            contentReplaced = try decodeContentReplacedView(present, "CardView.content_replaced")
        } else {
            contentReplaced = nil
        }
        return CardView(
            identity: identity,
            participation: participation,
            direction: direction,
            source: source,
            state: state,
            quiescence: quiescence,
            generation: generation,
            phase: phase,
            bytes: bytes,
            bytesResumed: bytesResumed,
            outcome: outcome,
            allowedActions: allowedActions,
            invite: invite,
            contentReplaced: contentReplaced
        )
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

    private static func decodeDutyView(_ value: Any?, _ context: String) throws -> DutyView {
        let map = try object(value, context)
        try knownKeys(map, ["provenance", "kind"], context)
        let provenance = try decodeDutyProvenanceView(try field(map, "provenance", "DutyView.provenance"), "DutyView.provenance")
        let kind = try decodeDutyKindView(try field(map, "kind", "DutyView.kind"), "DutyView.kind")
        return DutyView(
            provenance: provenance,
            kind: kind
        )
    }

    private static func decodeDutyFrameView(_ value: Any?, _ context: String) throws -> DutyFrameView {
        let map = try object(value, context)
        try knownKeys(map, ["duty", "action"], context)
        let duty = try decodeDutyView(try field(map, "duty", "DutyFrameView.duty"), "DutyFrameView.duty")
        let action = try decodeCapabilityActionView(try field(map, "action", "DutyFrameView.action"), "DutyFrameView.action")
        return DutyFrameView(
            duty: duty,
            action: action
        )
    }

    private static func decodeCardUpdateKindView(_ value: Any?, _ context: String) throws -> CardUpdateKindView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "snapshot":
            return .snapshot(try decodeCardView(payload(map, "CardUpdateKindView.snapshot"), "CardUpdateKindView.snapshot"))
        case "progress":
            return .progress(try decodeCardView(payload(map, "CardUpdateKindView.progress"), "CardUpdateKindView.progress"))
        case "state":
            return .state(try decodeCardView(payload(map, "CardUpdateKindView.state"), "CardUpdateKindView.state"))
        case "terminal":
            return .terminal(try decodeCardView(payload(map, "CardUpdateKindView.terminal"), "CardUpdateKindView.terminal"))
        case "capability_duty":
            return .capabilityDuty(try decodeDutyFrameView(payload(map, "CardUpdateKindView.capability_duty"), "CardUpdateKindView.capability_duty"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeCardUpdateView(_ value: Any?, _ context: String) throws -> CardUpdateView {
        let map = try object(value, context)
        try knownKeys(map, ["epoch", "card", "kind"], context)
        let epoch = try integer(try field(map, "epoch", "CardUpdateView.epoch"), u63Max, "CardUpdateView.epoch")
        let card = try hexFixed(try field(map, "card", "CardUpdateView.card"), 16, "CardUpdateView.card")
        let kind = try decodeCardUpdateKindView(try field(map, "kind", "CardUpdateView.kind"), "CardUpdateView.kind")
        return CardUpdateView(
            epoch: epoch,
            card: card,
            kind: kind
        )
    }

    private static func decodeLagView(_ value: Any?, _ context: String) throws -> LagView {
        let map = try object(value, context)
        try knownKeys(map, ["epoch", "card", "missed"], context)
        let epoch = try integer(try field(map, "epoch", "LagView.epoch"), u63Max, "LagView.epoch")
        let card = try hexFixed(try field(map, "card", "LagView.card"), 16, "LagView.card")
        let missed = try decodeLosslessKindView(try field(map, "missed", "LagView.missed"), "LagView.missed")
        return LagView(
            epoch: epoch,
            card: card,
            missed: missed
        )
    }

    private static func decodeClosedView(_ value: Any?, _ context: String) throws -> ClosedView {
        let map = try object(value, context)
        try knownKeys(map, ["epoch", "card"], context)
        let epoch = try integer(try field(map, "epoch", "ClosedView.epoch"), u63Max, "ClosedView.epoch")
        let card = try hexFixed(try field(map, "card", "ClosedView.card"), 16, "ClosedView.card")
        return ClosedView(
            epoch: epoch,
            card: card
        )
    }

    private static func decodeSubscribeRejectedView(_ value: Any?, _ context: String) throws -> SubscribeRejectedView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "reason"], context)
        let card = try hexFixed(try field(map, "card", "SubscribeRejectedView.card"), 16, "SubscribeRejectedView.card")
        let reason = try decodeSubscribeRejectionView(try field(map, "reason", "SubscribeRejectedView.reason"), "SubscribeRejectedView.reason")
        return SubscribeRejectedView(
            card: card,
            reason: reason
        )
    }

    private static func decodeSessionKeyView(_ value: Any?, _ context: String) throws -> SessionKeyView {
        let map = try object(value, context)
        try knownKeys(map, ["card", "generation"], context)
        let card = try hexFixed(try field(map, "card", "SessionKeyView.card"), 16, "SessionKeyView.card")
        let generation = try integer(try field(map, "generation", "SessionKeyView.generation"), 4294967295, "SessionKeyView.generation")
        return SessionKeyView(
            card: card,
            generation: generation
        )
    }

    private static func decodeEvidenceProgressView(_ value: Any?, _ context: String) throws -> EvidenceProgressView {
        let map = try object(value, context)
        try knownKeys(map, ["transferred", "total"], context)
        let transferred = try integer(try field(map, "transferred", "EvidenceProgressView.transferred"), u63Max, "EvidenceProgressView.transferred")
        let total = try integer(try field(map, "total", "EvidenceProgressView.total"), u63Max, "EvidenceProgressView.total")
        return EvidenceProgressView(
            transferred: transferred,
            total: total
        )
    }

    private static func decodeRedactedIdView(_ value: Any?, _ context: String) throws -> RedactedIdView {
        let map = try object(value, context)
        try knownKeys(map, ["kind"], context)
        let kind = try decodeRedactedIdKindView(try field(map, "kind", "RedactedIdView.kind"), "RedactedIdView.kind")
        return RedactedIdView(
            kind: kind
        )
    }

    private static func decodeEvidenceValueView(_ value: Any?, _ context: String) throws -> EvidenceValueView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "phase":
            return .phase(try decodePhaseView(payload(map, "EvidenceValueView.phase"), "EvidenceValueView.phase"))
        case "progress":
            return .progress(try decodeEvidenceProgressView(payload(map, "EvidenceValueView.progress"), "EvidenceValueView.progress"))
        case "outcome":
            return .outcome(try decodeOutcomeView(payload(map, "EvidenceValueView.outcome"), "EvidenceValueView.outcome"))
        case "identifier":
            return .identifier(try decodeRedactedIdView(payload(map, "EvidenceValueView.identifier"), "EvidenceValueView.identifier"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeDegradedView(_ value: Any?, _ context: String) throws -> DegradedView {
        let map = try object(value, context)
        try knownKeys(map, ["dropped_events"], context)
        let droppedEvents = try integer(try field(map, "dropped_events", "DegradedView.dropped_events"), u63Max, "DegradedView.dropped_events")
        return DegradedView(
            droppedEvents: droppedEvents
        )
    }

    private static func decodeDiagnosticsStatusView(_ value: Any?, _ context: String) throws -> DiagnosticsStatusView {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "complete":
            try unitPayload(map, "DiagnosticsStatusView.complete")
            return .complete
        case "degraded":
            return .degraded(try decodeDegradedView(payload(map, "DiagnosticsStatusView.degraded"), "DiagnosticsStatusView.degraded"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeTimelineEntryView(_ value: Any?, _ context: String) throws -> TimelineEntryView {
        let map = try object(value, context)
        try knownKeys(map, ["sequence", "value"], context)
        let sequence = try integer(try field(map, "sequence", "TimelineEntryView.sequence"), u63Max, "TimelineEntryView.sequence")
        let value = try decodeEvidenceValueView(try field(map, "value", "TimelineEntryView.value"), "TimelineEntryView.value")
        return TimelineEntryView(
            sequence: sequence,
            value: value
        )
    }

    private static func decodeEvidenceTimelineView(_ value: Any?, _ context: String) throws -> EvidenceTimelineView {
        let map = try object(value, context)
        try knownKeys(map, ["session", "status", "entries"], context)
        let session = try decodeSessionKeyView(try field(map, "session", "EvidenceTimelineView.session"), "EvidenceTimelineView.session")
        let status = try decodeDiagnosticsStatusView(try field(map, "status", "EvidenceTimelineView.status"), "EvidenceTimelineView.status")
        let entries = try decodeList(try field(map, "entries", "EvidenceTimelineView.entries"), 1024, "EvidenceTimelineView.entries", decodeTimelineEntryView)
        return EvidenceTimelineView(
            session: session,
            status: status,
            entries: entries
        )
    }

    private static func decodeProtocolManifestView(_ value: Any?, _ context: String) throws -> ProtocolManifestView {
        let map = try object(value, context)
        try knownKeys(map, ["set_id", "data_alpn", "data_magic", "data_wire_version"], context)
        let setId = try asciiBounded(try field(map, "set_id", "ProtocolManifestView.set_id"), 64, "ProtocolManifestView.set_id")
        let dataAlpn = try hexVariable(try field(map, "data_alpn", "ProtocolManifestView.data_alpn"), 64, "ProtocolManifestView.data_alpn")
        let dataMagic = try hexVariable(try field(map, "data_magic", "ProtocolManifestView.data_magic"), 32, "ProtocolManifestView.data_magic")
        let dataWireVersion = try integer(try field(map, "data_wire_version", "ProtocolManifestView.data_wire_version"), 65535, "ProtocolManifestView.data_wire_version")
        return ProtocolManifestView(
            setId: setId,
            dataAlpn: dataAlpn,
            dataMagic: dataMagic,
            dataWireVersion: dataWireVersion
        )
    }

    private static func decodeAbiSchemaManifestView(_ value: Any?, _ context: String) throws -> AbiSchemaManifestView {
        let map = try object(value, context)
        try knownKeys(map, ["read_binding_schema_id", "command_binding_schema_id", "capability_binding_schema_id", "evidence_rust_abi_id", "evidence_timeline_schema_id", "mailbox_receipt_schema_id", "operation_envelope_schema_id"], context)
        let readBindingSchemaId = try asciiBounded(try field(map, "read_binding_schema_id", "AbiSchemaManifestView.read_binding_schema_id"), 64, "AbiSchemaManifestView.read_binding_schema_id")
        let commandBindingSchemaId = try asciiBounded(try field(map, "command_binding_schema_id", "AbiSchemaManifestView.command_binding_schema_id"), 64, "AbiSchemaManifestView.command_binding_schema_id")
        let capabilityBindingSchemaId = try asciiBounded(try field(map, "capability_binding_schema_id", "AbiSchemaManifestView.capability_binding_schema_id"), 64, "AbiSchemaManifestView.capability_binding_schema_id")
        let evidenceRustAbiId = try asciiBounded(try field(map, "evidence_rust_abi_id", "AbiSchemaManifestView.evidence_rust_abi_id"), 64, "AbiSchemaManifestView.evidence_rust_abi_id")
        let evidenceTimelineSchemaId = try asciiBounded(try field(map, "evidence_timeline_schema_id", "AbiSchemaManifestView.evidence_timeline_schema_id"), 64, "AbiSchemaManifestView.evidence_timeline_schema_id")
        let mailboxReceiptSchemaId = try asciiBounded(try field(map, "mailbox_receipt_schema_id", "AbiSchemaManifestView.mailbox_receipt_schema_id"), 64, "AbiSchemaManifestView.mailbox_receipt_schema_id")
        let operationEnvelopeSchemaId = try asciiBounded(try field(map, "operation_envelope_schema_id", "AbiSchemaManifestView.operation_envelope_schema_id"), 64, "AbiSchemaManifestView.operation_envelope_schema_id")
        return AbiSchemaManifestView(
            readBindingSchemaId: readBindingSchemaId,
            commandBindingSchemaId: commandBindingSchemaId,
            capabilityBindingSchemaId: capabilityBindingSchemaId,
            evidenceRustAbiId: evidenceRustAbiId,
            evidenceTimelineSchemaId: evidenceTimelineSchemaId,
            mailboxReceiptSchemaId: mailboxReceiptSchemaId,
            operationEnvelopeSchemaId: operationEnvelopeSchemaId
        )
    }

    private static func decodeDeploymentManifestView(_ value: Any?, _ context: String) throws -> DeploymentManifestView {
        let map = try object(value, context)
        try knownKeys(map, ["environment", "rendezvous_endpoint", "relay_url"], context)
        let environment = try asciiBounded(try field(map, "environment", "DeploymentManifestView.environment"), 32, "DeploymentManifestView.environment")
        let rendezvousEndpoint = try asciiBounded(try field(map, "rendezvous_endpoint", "DeploymentManifestView.rendezvous_endpoint"), 1024, "DeploymentManifestView.rendezvous_endpoint")
        let relayUrl = try asciiBounded(try field(map, "relay_url", "DeploymentManifestView.relay_url"), 2048, "DeploymentManifestView.relay_url")
        return DeploymentManifestView(
            environment: environment,
            rendezvousEndpoint: rendezvousEndpoint,
            relayUrl: relayUrl
        )
    }

    private static func decodeBuildManifestView(_ value: Any?, _ context: String) throws -> BuildManifestView {
        let map = try object(value, context)
        try knownKeys(map, ["package_version", "protocol", "abi_schema", "deployment"], context)
        let packageVersion = try asciiBounded(try field(map, "package_version", "BuildManifestView.package_version"), 32, "BuildManifestView.package_version")
        let `protocol` = try decodeProtocolManifestView(try field(map, "protocol", "BuildManifestView.protocol"), "BuildManifestView.protocol")
        let abiSchema = try decodeAbiSchemaManifestView(try field(map, "abi_schema", "BuildManifestView.abi_schema"), "BuildManifestView.abi_schema")
        let deployment = try decodeDeploymentManifestView(try field(map, "deployment", "BuildManifestView.deployment"), "BuildManifestView.deployment")
        return BuildManifestView(
            packageVersion: packageVersion,
            `protocol`: `protocol`,
            abiSchema: abiSchema,
            deployment: deployment
        )
    }

    private static func decodeReadBody(_ value: Any?, _ context: String) throws -> ReadBody {
        let map = try object(value, context)
        try knownKeys(map, ["kind", "value"], context)
        guard let kind = try field(map, "kind", context) as? String else {
            throw ReadContractError(kind: .shape, context: context)
        }
        switch kind {
        case "card_update":
            return .cardUpdate(try decodeCardUpdateView(payload(map, "ReadBody.card_update"), "ReadBody.card_update"))
        case "lag":
            return .lag(try decodeLagView(payload(map, "ReadBody.lag"), "ReadBody.lag"))
        case "closed":
            return .closed(try decodeClosedView(payload(map, "ReadBody.closed"), "ReadBody.closed"))
        case "subscribe_rejected":
            return .subscribeRejected(try decodeSubscribeRejectedView(payload(map, "ReadBody.subscribe_rejected"), "ReadBody.subscribe_rejected"))
        case "evidence":
            return .evidence(try decodeEvidenceTimelineView(payload(map, "ReadBody.evidence"), "ReadBody.evidence"))
        case "build_manifest":
            return .buildManifest(try decodeBuildManifestView(payload(map, "ReadBody.build_manifest"), "ReadBody.build_manifest"))
        default:
            throw ReadContractError(kind: .unknownVariant, context: context)
        }
    }

    private static func decodeReadFrame(_ value: Any?, _ context: String) throws -> ReadFrame {
        let map = try object(value, context)
        try knownKeys(map, ["schema", "body"], context)
        let body = try decodeReadBody(try field(map, "body", "ReadFrame.body"), "ReadFrame.body")
        return ReadFrame(
            body: body
        )
    }
}
}
