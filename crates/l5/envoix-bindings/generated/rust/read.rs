// @generated from schema/read.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.

use serde_json::{Map, Value};

use envoix_types::Secret;

pub const READ_SCHEMA_ID: &str = "envoix/binding/read/10";
pub const READ_MAX_FRAME_BYTES: usize = 1048576;

const U63_MAX: u64 = 9_223_372_036_854_775_807;

/// Typed codec failure. It carries only static schema context, never a
/// fragment of the (possibly hostile) input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    FrameTooLarge,
    MalformedJson,
    UnknownSchema,
    Shape { context: &'static str },
    UnknownField { context: &'static str },
    UnknownVariant { context: &'static str },
    Range { context: &'static str },
    Bound { context: &'static str },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectionView {
    Send,
    Receive,
}

impl DirectionView {
    pub const ALL: [Self; 2] = [
        Self::Send,
        Self::Receive,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseView {
    Preparing,
    Pairing,
    Authenticating,
    Transferring,
    Confirming,
    Publishing,
    Restoring,
}

impl PhaseView {
    pub const ALL: [Self; 7] = [
        Self::Preparing,
        Self::Pairing,
        Self::Authenticating,
        Self::Transferring,
        Self::Confirming,
        Self::Publishing,
        Self::Restoring,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutcomeCodeView {
    Completed,
    Cancelled,
    Paused,
    PeerLost,
    Timeout,
    Unauthenticated,
    VersionMismatch,
    StorageFault,
    StorageFull,
    PublishFailed,
    SourceUnreadable,
    NetworkUnreachable,
    Internal,
}

impl OutcomeCodeView {
    pub const ALL: [Self; 13] = [
        Self::Completed,
        Self::Cancelled,
        Self::Paused,
        Self::PeerLost,
        Self::Timeout,
        Self::Unauthenticated,
        Self::VersionMismatch,
        Self::StorageFault,
        Self::StorageFull,
        Self::PublishFailed,
        Self::SourceUnreadable,
        Self::NetworkUnreachable,
        Self::Internal,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryabilityView {
    Retryable,
    Terminal,
    NeedsUser,
}

impl RetryabilityView {
    pub const ALL: [Self; 3] = [
        Self::Retryable,
        Self::Terminal,
        Self::NeedsUser,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryView {
    RePickSource,
    RetryLater,
    ReconnectPeer,
}

impl RecoveryView {
    pub const ALL: [Self; 3] = [
        Self::RePickSource,
        Self::RetryLater,
        Self::ReconnectPeer,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseOriginView {
    Local,
    Peer,
    Lost,
}

impl PauseOriginView {
    pub const ALL: [Self; 3] = [
        Self::Local,
        Self::Peer,
        Self::Lost,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerKindView {
    Attempt,
    Staging,
}

impl WorkerKindView {
    pub const ALL: [Self; 2] = [
        Self::Attempt,
        Self::Staging,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementIntentView {
    Pause,
    Cancel,
    Finalize,
}

impl RetirementIntentView {
    pub const ALL: [Self; 3] = [
        Self::Pause,
        Self::Cancel,
        Self::Finalize,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DutyKindView {
    SourceHandle,
    Grant,
    Staging,
    Publication,
    Courier,
    Foreground,
    Notification,
    Lock,
    OpenShare,
}

impl DutyKindView {
    pub const ALL: [Self; 9] = [
        Self::SourceHandle,
        Self::Grant,
        Self::Staging,
        Self::Publication,
        Self::Courier,
        Self::Foreground,
        Self::Notification,
        Self::Lock,
        Self::OpenShare,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityActionView {
    PostReceipt,
    AcquireSource,
}

impl CapabilityActionView {
    pub const ALL: [Self; 2] = [
        Self::PostReceipt,
        Self::AcquireSource,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKindView {
    Pause,
    Cancel,
    Resume,
    Remove,
    RePickSource,
}

impl CommandKindView {
    pub const ALL: [Self; 5] = [
        Self::Pause,
        Self::Cancel,
        Self::Resume,
        Self::Remove,
        Self::RePickSource,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedactedIdKindView {
    Record,
    Transfer,
    Artifact,
    Request,
}

impl RedactedIdKindView {
    pub const ALL: [Self; 4] = [
        Self::Record,
        Self::Transfer,
        Self::Artifact,
        Self::Request,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LosslessKindView {
    Terminal,
    CapabilityDuty,
}

impl LosslessKindView {
    pub const ALL: [Self; 2] = [
        Self::Terminal,
        Self::CapabilityDuty,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscribeRejectionView {
    UnknownCard,
    RuntimeStopped,
    EpochExhausted,
}

impl SubscribeRejectionView {
    pub const ALL: [Self; 3] = [
        Self::UnknownCard,
        Self::RuntimeStopped,
        Self::EpochExhausted,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutcomeView {
    pub code: OutcomeCodeView,
    pub phase: PhaseView,
    pub retry: RetryabilityView,
    pub recovery: Option<RecoveryView>,
    pub display: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PausedView {
    pub origin: PauseOriginView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductStateView {
    Preparing,
    Waiting,
    Connecting,
    Verifying,
    Transferring,
    Confirming,
    Paused(PausedView),
    Unconfirmed,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningView {
    pub worker: WorkerKindView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetiringView {
    pub worker: WorkerKindView,
    pub intent: RetirementIntentView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuiescenceView {
    Running(RunningView),
    Retiring(RetiringView),
    Quiescent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityView {
    pub card: String,
    pub transfer: String,
    pub artifact: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QrView {
    pub width: u16,
    pub modules: Secret<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InviteView {
    pub code: Secret<String>,
    pub code_fingerprint: String,
    pub link: Option<Secret<String>>,
    pub qr: Option<QrView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomParticipationView {
    Minted,
    Joined,
}

impl RoomParticipationView {
    pub const ALL: [Self; 2] = [
        Self::Minted,
        Self::Joined,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAcquisitionKeyView {
    pub card: String,
    pub generation: u32,
    pub request: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePromptReasonView {
    Initial,
    Unreadable,
    PermissionLost,
    StorageFault,
    StagingFailed,
    Internal,
}

impl SourcePromptReasonView {
    pub const ALL: [Self; 6] = [
        Self::Initial,
        Self::Unreadable,
        Self::PermissionLost,
        Self::StorageFault,
        Self::StagingFailed,
        Self::Internal,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSourceOfferView {
    pub acquisition: SourceAcquisitionKeyView,
    pub display_name: String,
    pub reported_size: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferContentView {
    pub offered_name: String,
    pub total: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceNotRequiredView {
    pub peer_content: Option<TransferContentView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSelectableView {
    pub acquisition: SourceAcquisitionKeyView,
    pub reason: SourcePromptReasonView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRePickRequiredView {
    pub reason: SourcePromptReasonView,
    pub previous_offer: AcceptedSourceOfferView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSelectionGateView {
    Selectable(SourceSelectableView),
    RePickRequired(SourceRePickRequiredView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAwaitingSelectionView {
    pub selection: SourceSelectionGateView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReadyView {
    pub offer: AcceptedSourceOfferView,
    pub content: TransferContentView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLifecycleView {
    NotRequired(SourceNotRequiredView),
    AwaitingSelection(SourceAwaitingSelectionView),
    Acquiring(AcceptedSourceOfferView),
    Staging(AcceptedSourceOfferView),
    Ready(SourceReadyView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickSourceActionView {
    pub acquisition: SourceAcquisitionKeyView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CardActionView {
    Command(CommandKindView),
    PickSource(PickSourceActionView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardView {
    pub identity: IdentityView,
    pub participation: RoomParticipationView,
    pub direction: DirectionView,
    pub source: SourceLifecycleView,
    pub state: ProductStateView,
    pub quiescence: QuiescenceView,
    pub generation: u32,
    pub phase: PhaseView,
    pub bytes: u64,
    pub bytes_resumed: u64,
    pub outcome: Option<OutcomeView>,
    pub allowed_actions: Vec<CardActionView>,
    pub invite: Option<InviteView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyProvenanceView {
    pub card: String,
    pub generation: u32,
    pub request: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyView {
    pub provenance: DutyProvenanceView,
    pub kind: DutyKindView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyFrameView {
    pub duty: DutyView,
    pub action: CapabilityActionView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CardUpdateKindView {
    Snapshot(CardView),
    Progress(CardView),
    State(CardView),
    Terminal(CardView),
    CapabilityDuty(DutyFrameView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardUpdateView {
    pub epoch: u64,
    pub card: String,
    pub kind: CardUpdateKindView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LagView {
    pub epoch: u64,
    pub card: String,
    pub missed: LosslessKindView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedView {
    pub epoch: u64,
    pub card: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscribeRejectedView {
    pub card: String,
    pub reason: SubscribeRejectionView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionKeyView {
    pub card: String,
    pub generation: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceProgressView {
    pub transferred: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedIdView {
    pub kind: RedactedIdKindView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceValueView {
    Phase(PhaseView),
    Progress(EvidenceProgressView),
    Outcome(OutcomeView),
    Identifier(RedactedIdView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DegradedView {
    pub dropped_events: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticsStatusView {
    Complete,
    Degraded(DegradedView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEntryView {
    pub sequence: u64,
    pub value: EvidenceValueView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceTimelineView {
    pub session: SessionKeyView,
    pub status: DiagnosticsStatusView,
    pub entries: Vec<TimelineEntryView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolManifestView {
    pub set_id: String,
    pub data_alpn: String,
    pub data_magic: String,
    pub data_wire_version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiSchemaManifestView {
    pub read_binding_schema_id: String,
    pub command_binding_schema_id: String,
    pub capability_binding_schema_id: String,
    pub evidence_rust_abi_id: String,
    pub evidence_timeline_schema_id: String,
    pub mailbox_receipt_schema_id: String,
    pub operation_envelope_schema_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentManifestView {
    pub environment: String,
    pub rendezvous_endpoint: String,
    pub relay_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildManifestView {
    pub package_version: String,
    pub protocol: ProtocolManifestView,
    pub abi_schema: AbiSchemaManifestView,
    pub deployment: DeploymentManifestView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadBody {
    CardUpdate(CardUpdateView),
    Lag(LagView),
    Closed(ClosedView),
    SubscribeRejected(SubscribeRejectedView),
    Evidence(EvidenceTimelineView),
    BuildManifest(BuildManifestView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadFrame {
    pub body: ReadBody,
}

/// What a frontend should do with one decoded frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateDecision {
    Deliver,
    DropStale,
    ContractBreach,
}

/// Client-side admission for the per-epoch card stream: one gate per
/// attachment. Frames from another epoch are stale; every epoch starts
/// with a snapshot; a lag or close ends the epoch permanently.
/// Deliberately neither `Clone` nor `Copy`: a gate is identity-bearing
/// admission state, and a silent copy would fork it.
#[derive(Debug, Eq, PartialEq)]
pub struct EpochGate {
    epoch: u64,
    saw_snapshot: bool,
    dead: bool,
}

impl EpochGate {
    pub const fn attach(epoch: u64) -> Self {
        Self {
            epoch,
            saw_snapshot: false,
            dead: false,
        }
    }

    pub fn admit(&mut self, frame: &ReadFrame) -> GateDecision {
        match &frame.body {
            ReadBody::CardUpdate(update) => {
                if update.epoch != self.epoch || self.dead {
                    return GateDecision::DropStale;
                }
                match &update.kind {
                    CardUpdateKindView::Snapshot(_) => {
                        if self.saw_snapshot {
                            GateDecision::ContractBreach
                        } else {
                            self.saw_snapshot = true;
                            GateDecision::Deliver
                        }
                    }
                    _ => {
                        if self.saw_snapshot {
                            GateDecision::Deliver
                        } else {
                            GateDecision::ContractBreach
                        }
                    }
                }
            }
            ReadBody::Lag(lag) => self.terminate(lag.epoch),
            ReadBody::Closed(closed) => self.terminate(closed.epoch),
            _ => GateDecision::Deliver,
        }
    }

    fn terminate(&mut self, epoch: u64) -> GateDecision {
        if epoch == self.epoch && !self.dead {
            self.dead = true;
            GateDecision::Deliver
        } else {
            GateDecision::DropStale
        }
    }
}

/// Decodes and validates one frame. Every failure is a typed
/// [`ReadError`]; no input, however hostile, panics or misparses.
pub fn decode_read_frame(bytes: &[u8]) -> Result<ReadFrame, ReadError> {
    if bytes.len() > READ_MAX_FRAME_BYTES {
        return Err(ReadError::FrameTooLarge);
    }
    let value = strict_json(bytes)?;
    decode_read_frame_value(&value, "ReadFrame")
}

/// Encodes one frame, stamping the schema envelope and enforcing the
/// same bounds the decoder checks.
pub fn encode_read_frame(frame: &ReadFrame) -> Result<Vec<u8>, ReadError> {
    let value = encode_read_frame_value(frame)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| ReadError::MalformedJson)?;
    if bytes.len() > READ_MAX_FRAME_BYTES {
        return Err(ReadError::FrameTooLarge);
    }
    Ok(bytes)
}

/// Parses JSON while rejecting duplicate object keys at any depth and
/// trailing input. A duplicated key is the smuggling shape: a first-wins
/// upstream parser would see a different value than a last-wins one applies.
fn strict_json(bytes: &[u8]) -> Result<Value, ReadError> {
    struct StrictValue;

    impl<'de> serde::de::DeserializeSeed<'de> for StrictValue {
        type Value = Value;

        fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
            deserializer.deserialize_any(self)
        }
    }

    impl<'de> serde::de::Visitor<'de> for StrictValue {
        type Value = Value;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a json value without duplicate object keys")
        }

        fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
            Ok(Value::Bool(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
            Ok(Value::from(value))
        }

        fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
            Ok(Value::from(value))
        }

        fn visit_f64<E>(self, value: f64) -> Result<Value, E> {
            Ok(Value::from(value))
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
            Ok(Value::from(value))
        }

        fn visit_unit<E>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = access.next_element_seed(StrictValue)? {
                items.push(item);
            }
            Ok(Value::Array(items))
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
            let mut map = Map::new();
            while let Some(key) = access.next_key::<String>()? {
                let value = access.next_value_seed(StrictValue)?;
                if map.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate object key"));
                }
            }
            Ok(Value::Object(map))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = serde::de::DeserializeSeed::deserialize(StrictValue, &mut deserializer)
        .map_err(|_| ReadError::MalformedJson)?;
    deserializer.end().map_err(|_| ReadError::MalformedJson)?;
    Ok(value)
}

fn frame_object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, ReadError> {
    value.as_object().ok_or(ReadError::Shape { context })
}

fn known_keys(map: &Map<String, Value>, allowed: &[&str], context: &'static str) -> Result<(), ReadError> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ReadError::UnknownField { context });
        }
    }
    Ok(())
}

fn field<'a>(map: &'a Map<String, Value>, key: &str, context: &'static str) -> Result<&'a Value, ReadError> {
    map.get(key).ok_or(ReadError::Shape { context })
}

fn integer(value: &Value, max: u64, context: &'static str) -> Result<u64, ReadError> {
    let number = value.as_u64().ok_or(ReadError::Shape { context })?;
    if number > max {
        return Err(ReadError::Range { context });
    }
    Ok(number)
}

fn encode_u63(number: u64, context: &'static str) -> Result<Value, ReadError> {
    if number > U63_MAX {
        return Err(ReadError::Range { context });
    }
    Ok(Value::from(number))
}

fn integer_u16(value: &Value, context: &'static str) -> Result<u16, ReadError> {
    let number = integer(value, 65_535, context)?;
    u16::try_from(number).map_err(|_| ReadError::Range { context })
}

fn integer_u32(value: &Value, context: &'static str) -> Result<u32, ReadError> {
    let number = integer(value, 4_294_967_295, context)?;
    u32::try_from(number).map_err(|_| ReadError::Range { context })
}

fn hex_chars(text: &str) -> bool {
    text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_fixed(value: &Value, chars: usize, context: &'static str) -> Result<String, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    encode_hex_fixed(text, chars, context)?;
    Ok(text.to_owned())
}

fn encode_hex_fixed(text: &str, chars: usize, context: &'static str) -> Result<Value, ReadError> {
    if text.len() != chars || !hex_chars(text) {
        return Err(ReadError::Bound { context });
    }
    Ok(Value::from(text))
}

fn hex_variable(value: &Value, max_chars: usize, context: &'static str) -> Result<String, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    encode_hex_variable(text, max_chars, context)?;
    Ok(text.to_owned())
}

fn encode_hex_variable(text: &str, max_chars: usize, context: &'static str) -> Result<Value, ReadError> {
    let valid = !text.is_empty() && text.len().is_multiple_of(2) && text.len() <= max_chars && hex_chars(text);
    if !valid {
        return Err(ReadError::Bound { context });
    }
    Ok(Value::from(text))
}

fn utf8_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    encode_utf8_bounded(text, max_bytes, context)?;
    Ok(text.to_owned())
}

fn encode_utf8_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, ReadError> {
    if text.len() > max_bytes {
        return Err(ReadError::Bound { context });
    }
    Ok(Value::from(text))
}

fn ascii_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    encode_ascii_bounded(text, max_bytes, context)?;
    Ok(text.to_owned())
}

fn encode_ascii_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, ReadError> {
    let valid = text.len() <= max_bytes && text.bytes().all(|byte| (0x20..=0x7e).contains(&byte));
    if !valid {
        return Err(ReadError::Bound { context });
    }
    Ok(Value::from(text))
}

fn payload<'a>(map: &'a Map<String, Value>, context: &'static str) -> Result<&'a Value, ReadError> {
    match map.get("value") {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(ReadError::Shape { context }),
    }
}

fn unit_payload(map: &Map<String, Value>, context: &'static str) -> Result<(), ReadError> {
    match map.get("value") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(ReadError::Shape { context }),
    }
}

fn decode_direction_view_value(value: &Value, context: &'static str) -> Result<DirectionView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "send" => Ok(DirectionView::Send),
        "receive" => Ok(DirectionView::Receive),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_direction_view_value(value: &DirectionView) -> Value {
    Value::from(match value {
        DirectionView::Send => "send",
        DirectionView::Receive => "receive",
    })
}

fn decode_phase_view_value(value: &Value, context: &'static str) -> Result<PhaseView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "preparing" => Ok(PhaseView::Preparing),
        "pairing" => Ok(PhaseView::Pairing),
        "authenticating" => Ok(PhaseView::Authenticating),
        "transferring" => Ok(PhaseView::Transferring),
        "confirming" => Ok(PhaseView::Confirming),
        "publishing" => Ok(PhaseView::Publishing),
        "restoring" => Ok(PhaseView::Restoring),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_phase_view_value(value: &PhaseView) -> Value {
    Value::from(match value {
        PhaseView::Preparing => "preparing",
        PhaseView::Pairing => "pairing",
        PhaseView::Authenticating => "authenticating",
        PhaseView::Transferring => "transferring",
        PhaseView::Confirming => "confirming",
        PhaseView::Publishing => "publishing",
        PhaseView::Restoring => "restoring",
    })
}

fn decode_outcome_code_view_value(value: &Value, context: &'static str) -> Result<OutcomeCodeView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "completed" => Ok(OutcomeCodeView::Completed),
        "cancelled" => Ok(OutcomeCodeView::Cancelled),
        "paused" => Ok(OutcomeCodeView::Paused),
        "peer_lost" => Ok(OutcomeCodeView::PeerLost),
        "timeout" => Ok(OutcomeCodeView::Timeout),
        "unauthenticated" => Ok(OutcomeCodeView::Unauthenticated),
        "version_mismatch" => Ok(OutcomeCodeView::VersionMismatch),
        "storage_fault" => Ok(OutcomeCodeView::StorageFault),
        "storage_full" => Ok(OutcomeCodeView::StorageFull),
        "publish_failed" => Ok(OutcomeCodeView::PublishFailed),
        "source_unreadable" => Ok(OutcomeCodeView::SourceUnreadable),
        "network_unreachable" => Ok(OutcomeCodeView::NetworkUnreachable),
        "internal" => Ok(OutcomeCodeView::Internal),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_outcome_code_view_value(value: &OutcomeCodeView) -> Value {
    Value::from(match value {
        OutcomeCodeView::Completed => "completed",
        OutcomeCodeView::Cancelled => "cancelled",
        OutcomeCodeView::Paused => "paused",
        OutcomeCodeView::PeerLost => "peer_lost",
        OutcomeCodeView::Timeout => "timeout",
        OutcomeCodeView::Unauthenticated => "unauthenticated",
        OutcomeCodeView::VersionMismatch => "version_mismatch",
        OutcomeCodeView::StorageFault => "storage_fault",
        OutcomeCodeView::StorageFull => "storage_full",
        OutcomeCodeView::PublishFailed => "publish_failed",
        OutcomeCodeView::SourceUnreadable => "source_unreadable",
        OutcomeCodeView::NetworkUnreachable => "network_unreachable",
        OutcomeCodeView::Internal => "internal",
    })
}

fn decode_retryability_view_value(value: &Value, context: &'static str) -> Result<RetryabilityView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "retryable" => Ok(RetryabilityView::Retryable),
        "terminal" => Ok(RetryabilityView::Terminal),
        "needs_user" => Ok(RetryabilityView::NeedsUser),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_retryability_view_value(value: &RetryabilityView) -> Value {
    Value::from(match value {
        RetryabilityView::Retryable => "retryable",
        RetryabilityView::Terminal => "terminal",
        RetryabilityView::NeedsUser => "needs_user",
    })
}

fn decode_recovery_view_value(value: &Value, context: &'static str) -> Result<RecoveryView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "re_pick_source" => Ok(RecoveryView::RePickSource),
        "retry_later" => Ok(RecoveryView::RetryLater),
        "reconnect_peer" => Ok(RecoveryView::ReconnectPeer),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_recovery_view_value(value: &RecoveryView) -> Value {
    Value::from(match value {
        RecoveryView::RePickSource => "re_pick_source",
        RecoveryView::RetryLater => "retry_later",
        RecoveryView::ReconnectPeer => "reconnect_peer",
    })
}

fn decode_pause_origin_view_value(value: &Value, context: &'static str) -> Result<PauseOriginView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "local" => Ok(PauseOriginView::Local),
        "peer" => Ok(PauseOriginView::Peer),
        "lost" => Ok(PauseOriginView::Lost),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_pause_origin_view_value(value: &PauseOriginView) -> Value {
    Value::from(match value {
        PauseOriginView::Local => "local",
        PauseOriginView::Peer => "peer",
        PauseOriginView::Lost => "lost",
    })
}

fn decode_worker_kind_view_value(value: &Value, context: &'static str) -> Result<WorkerKindView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "attempt" => Ok(WorkerKindView::Attempt),
        "staging" => Ok(WorkerKindView::Staging),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_worker_kind_view_value(value: &WorkerKindView) -> Value {
    Value::from(match value {
        WorkerKindView::Attempt => "attempt",
        WorkerKindView::Staging => "staging",
    })
}

fn decode_retirement_intent_view_value(value: &Value, context: &'static str) -> Result<RetirementIntentView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "pause" => Ok(RetirementIntentView::Pause),
        "cancel" => Ok(RetirementIntentView::Cancel),
        "finalize" => Ok(RetirementIntentView::Finalize),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_retirement_intent_view_value(value: &RetirementIntentView) -> Value {
    Value::from(match value {
        RetirementIntentView::Pause => "pause",
        RetirementIntentView::Cancel => "cancel",
        RetirementIntentView::Finalize => "finalize",
    })
}

fn decode_duty_kind_view_value(value: &Value, context: &'static str) -> Result<DutyKindView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "source_handle" => Ok(DutyKindView::SourceHandle),
        "grant" => Ok(DutyKindView::Grant),
        "staging" => Ok(DutyKindView::Staging),
        "publication" => Ok(DutyKindView::Publication),
        "courier" => Ok(DutyKindView::Courier),
        "foreground" => Ok(DutyKindView::Foreground),
        "notification" => Ok(DutyKindView::Notification),
        "lock" => Ok(DutyKindView::Lock),
        "open_share" => Ok(DutyKindView::OpenShare),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_duty_kind_view_value(value: &DutyKindView) -> Value {
    Value::from(match value {
        DutyKindView::SourceHandle => "source_handle",
        DutyKindView::Grant => "grant",
        DutyKindView::Staging => "staging",
        DutyKindView::Publication => "publication",
        DutyKindView::Courier => "courier",
        DutyKindView::Foreground => "foreground",
        DutyKindView::Notification => "notification",
        DutyKindView::Lock => "lock",
        DutyKindView::OpenShare => "open_share",
    })
}

fn decode_capability_action_view_value(value: &Value, context: &'static str) -> Result<CapabilityActionView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "post_receipt" => Ok(CapabilityActionView::PostReceipt),
        "acquire_source" => Ok(CapabilityActionView::AcquireSource),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_capability_action_view_value(value: &CapabilityActionView) -> Value {
    Value::from(match value {
        CapabilityActionView::PostReceipt => "post_receipt",
        CapabilityActionView::AcquireSource => "acquire_source",
    })
}

fn decode_command_kind_view_value(value: &Value, context: &'static str) -> Result<CommandKindView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "pause" => Ok(CommandKindView::Pause),
        "cancel" => Ok(CommandKindView::Cancel),
        "resume" => Ok(CommandKindView::Resume),
        "remove" => Ok(CommandKindView::Remove),
        "re_pick_source" => Ok(CommandKindView::RePickSource),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_command_kind_view_value(value: &CommandKindView) -> Value {
    Value::from(match value {
        CommandKindView::Pause => "pause",
        CommandKindView::Cancel => "cancel",
        CommandKindView::Resume => "resume",
        CommandKindView::Remove => "remove",
        CommandKindView::RePickSource => "re_pick_source",
    })
}

fn decode_redacted_id_kind_view_value(value: &Value, context: &'static str) -> Result<RedactedIdKindView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "record" => Ok(RedactedIdKindView::Record),
        "transfer" => Ok(RedactedIdKindView::Transfer),
        "artifact" => Ok(RedactedIdKindView::Artifact),
        "request" => Ok(RedactedIdKindView::Request),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_redacted_id_kind_view_value(value: &RedactedIdKindView) -> Value {
    Value::from(match value {
        RedactedIdKindView::Record => "record",
        RedactedIdKindView::Transfer => "transfer",
        RedactedIdKindView::Artifact => "artifact",
        RedactedIdKindView::Request => "request",
    })
}

fn decode_lossless_kind_view_value(value: &Value, context: &'static str) -> Result<LosslessKindView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "terminal" => Ok(LosslessKindView::Terminal),
        "capability_duty" => Ok(LosslessKindView::CapabilityDuty),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_lossless_kind_view_value(value: &LosslessKindView) -> Value {
    Value::from(match value {
        LosslessKindView::Terminal => "terminal",
        LosslessKindView::CapabilityDuty => "capability_duty",
    })
}

fn decode_subscribe_rejection_view_value(value: &Value, context: &'static str) -> Result<SubscribeRejectionView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "unknown_card" => Ok(SubscribeRejectionView::UnknownCard),
        "runtime_stopped" => Ok(SubscribeRejectionView::RuntimeStopped),
        "epoch_exhausted" => Ok(SubscribeRejectionView::EpochExhausted),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_subscribe_rejection_view_value(value: &SubscribeRejectionView) -> Value {
    Value::from(match value {
        SubscribeRejectionView::UnknownCard => "unknown_card",
        SubscribeRejectionView::RuntimeStopped => "runtime_stopped",
        SubscribeRejectionView::EpochExhausted => "epoch_exhausted",
    })
}

fn decode_outcome_view_value(value: &Value, context: &'static str) -> Result<OutcomeView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["code", "phase", "retry", "recovery", "display"], context)?;
    let code = decode_outcome_code_view_value(field(map, "code", "OutcomeView.code")?, "OutcomeView.code")?;
    let phase = decode_phase_view_value(field(map, "phase", "OutcomeView.phase")?, "OutcomeView.phase")?;
    let retry = decode_retryability_view_value(field(map, "retry", "OutcomeView.retry")?, "OutcomeView.retry")?;
    let recovery = match field(map, "recovery", "OutcomeView.recovery")? {
        Value::Null => None,
        present => Some(decode_recovery_view_value(present, "OutcomeView.recovery")?),
    };
    let display = utf8_bounded(field(map, "display", "OutcomeView.display")?, 160, "OutcomeView.display")?;
    Ok(OutcomeView {
        code,
        phase,
        retry,
        recovery,
        display,
    })
}

fn encode_outcome_view_value(value: &OutcomeView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("code".to_owned(), encode_outcome_code_view_value(&value.code));
    map.insert("phase".to_owned(), encode_phase_view_value(&value.phase));
    map.insert("retry".to_owned(), encode_retryability_view_value(&value.retry));
    map.insert(
        "recovery".to_owned(),
        match &value.recovery {
            None => Value::Null,
            Some(inner) => encode_recovery_view_value(inner),
        },
    );
    map.insert("display".to_owned(), encode_utf8_bounded(&value.display, 160, "OutcomeView.display")?);
    Ok(Value::Object(map))
}

fn decode_paused_view_value(value: &Value, context: &'static str) -> Result<PausedView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["origin"], context)?;
    let origin = decode_pause_origin_view_value(field(map, "origin", "PausedView.origin")?, "PausedView.origin")?;
    Ok(PausedView {
        origin,
    })
}

fn encode_paused_view_value(value: &PausedView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("origin".to_owned(), encode_pause_origin_view_value(&value.origin));
    Ok(Value::Object(map))
}

fn decode_product_state_view_value(value: &Value, context: &'static str) -> Result<ProductStateView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "preparing" => {
            unit_payload(map, "ProductStateView.preparing")?;
            Ok(ProductStateView::Preparing)
        }
        "waiting" => {
            unit_payload(map, "ProductStateView.waiting")?;
            Ok(ProductStateView::Waiting)
        }
        "connecting" => {
            unit_payload(map, "ProductStateView.connecting")?;
            Ok(ProductStateView::Connecting)
        }
        "verifying" => {
            unit_payload(map, "ProductStateView.verifying")?;
            Ok(ProductStateView::Verifying)
        }
        "transferring" => {
            unit_payload(map, "ProductStateView.transferring")?;
            Ok(ProductStateView::Transferring)
        }
        "confirming" => {
            unit_payload(map, "ProductStateView.confirming")?;
            Ok(ProductStateView::Confirming)
        }
        "paused" => Ok(ProductStateView::Paused(decode_paused_view_value(payload(map, "ProductStateView.paused")?, "ProductStateView.paused")?)),
        "unconfirmed" => {
            unit_payload(map, "ProductStateView.unconfirmed")?;
            Ok(ProductStateView::Unconfirmed)
        }
        "completed" => {
            unit_payload(map, "ProductStateView.completed")?;
            Ok(ProductStateView::Completed)
        }
        "failed" => {
            unit_payload(map, "ProductStateView.failed")?;
            Ok(ProductStateView::Failed)
        }
        "cancelled" => {
            unit_payload(map, "ProductStateView.cancelled")?;
            Ok(ProductStateView::Cancelled)
        }
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_product_state_view_value(value: &ProductStateView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        ProductStateView::Preparing => {
            map.insert("kind".to_owned(), Value::from("preparing"));
        }
        ProductStateView::Waiting => {
            map.insert("kind".to_owned(), Value::from("waiting"));
        }
        ProductStateView::Connecting => {
            map.insert("kind".to_owned(), Value::from("connecting"));
        }
        ProductStateView::Verifying => {
            map.insert("kind".to_owned(), Value::from("verifying"));
        }
        ProductStateView::Transferring => {
            map.insert("kind".to_owned(), Value::from("transferring"));
        }
        ProductStateView::Confirming => {
            map.insert("kind".to_owned(), Value::from("confirming"));
        }
        ProductStateView::Paused(payload) => {
            map.insert("kind".to_owned(), Value::from("paused"));
            map.insert("value".to_owned(), encode_paused_view_value(payload)?);
        }
        ProductStateView::Unconfirmed => {
            map.insert("kind".to_owned(), Value::from("unconfirmed"));
        }
        ProductStateView::Completed => {
            map.insert("kind".to_owned(), Value::from("completed"));
        }
        ProductStateView::Failed => {
            map.insert("kind".to_owned(), Value::from("failed"));
        }
        ProductStateView::Cancelled => {
            map.insert("kind".to_owned(), Value::from("cancelled"));
        }
    }
    Ok(Value::Object(map))
}

fn decode_running_view_value(value: &Value, context: &'static str) -> Result<RunningView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["worker"], context)?;
    let worker = decode_worker_kind_view_value(field(map, "worker", "RunningView.worker")?, "RunningView.worker")?;
    Ok(RunningView {
        worker,
    })
}

fn encode_running_view_value(value: &RunningView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("worker".to_owned(), encode_worker_kind_view_value(&value.worker));
    Ok(Value::Object(map))
}

fn decode_retiring_view_value(value: &Value, context: &'static str) -> Result<RetiringView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["worker", "intent"], context)?;
    let worker = decode_worker_kind_view_value(field(map, "worker", "RetiringView.worker")?, "RetiringView.worker")?;
    let intent = decode_retirement_intent_view_value(field(map, "intent", "RetiringView.intent")?, "RetiringView.intent")?;
    Ok(RetiringView {
        worker,
        intent,
    })
}

fn encode_retiring_view_value(value: &RetiringView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("worker".to_owned(), encode_worker_kind_view_value(&value.worker));
    map.insert("intent".to_owned(), encode_retirement_intent_view_value(&value.intent));
    Ok(Value::Object(map))
}

fn decode_quiescence_view_value(value: &Value, context: &'static str) -> Result<QuiescenceView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "running" => Ok(QuiescenceView::Running(decode_running_view_value(payload(map, "QuiescenceView.running")?, "QuiescenceView.running")?)),
        "retiring" => Ok(QuiescenceView::Retiring(decode_retiring_view_value(payload(map, "QuiescenceView.retiring")?, "QuiescenceView.retiring")?)),
        "quiescent" => {
            unit_payload(map, "QuiescenceView.quiescent")?;
            Ok(QuiescenceView::Quiescent)
        }
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_quiescence_view_value(value: &QuiescenceView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        QuiescenceView::Running(payload) => {
            map.insert("kind".to_owned(), Value::from("running"));
            map.insert("value".to_owned(), encode_running_view_value(payload)?);
        }
        QuiescenceView::Retiring(payload) => {
            map.insert("kind".to_owned(), Value::from("retiring"));
            map.insert("value".to_owned(), encode_retiring_view_value(payload)?);
        }
        QuiescenceView::Quiescent => {
            map.insert("kind".to_owned(), Value::from("quiescent"));
        }
    }
    Ok(Value::Object(map))
}

fn decode_identity_view_value(value: &Value, context: &'static str) -> Result<IdentityView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "transfer", "artifact"], context)?;
    let card = hex_fixed(field(map, "card", "IdentityView.card")?, 16, "IdentityView.card")?;
    let transfer = hex_fixed(field(map, "transfer", "IdentityView.transfer")?, 32, "IdentityView.transfer")?;
    let artifact = hex_fixed(field(map, "artifact", "IdentityView.artifact")?, 32, "IdentityView.artifact")?;
    Ok(IdentityView {
        card,
        transfer,
        artifact,
    })
}

fn encode_identity_view_value(value: &IdentityView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "IdentityView.card")?);
    map.insert("transfer".to_owned(), encode_hex_fixed(&value.transfer, 32, "IdentityView.transfer")?);
    map.insert("artifact".to_owned(), encode_hex_fixed(&value.artifact, 32, "IdentityView.artifact")?);
    Ok(Value::Object(map))
}

fn decode_qr_view_value(value: &Value, context: &'static str) -> Result<QrView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["width", "modules"], context)?;
    let width = integer_u16(field(map, "width", "QrView.width")?, "QrView.width")?;
    let modules = Secret::new(hex_variable(field(map, "modules", "QrView.modules")?, 7834, "QrView.modules")?);
    Ok(QrView {
        width,
        modules,
    })
}

fn encode_qr_view_value(value: &QrView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("width".to_owned(), Value::from(value.width));
    map.insert("modules".to_owned(), encode_hex_variable(value.modules.expose(), 7834, "QrView.modules")?);
    Ok(Value::Object(map))
}

fn decode_invite_view_value(value: &Value, context: &'static str) -> Result<InviteView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["code", "code_fingerprint", "link", "qr"], context)?;
    let code = Secret::new(utf8_bounded(field(map, "code", "InviteView.code")?, 64, "InviteView.code")?);
    let code_fingerprint = hex_fixed(field(map, "code_fingerprint", "InviteView.code_fingerprint")?, 16, "InviteView.code_fingerprint")?;
    let link = match field(map, "link", "InviteView.link")? {
        Value::Null => None,
        present => Some(Secret::new(utf8_bounded(present, 5481, "InviteView.link")?)),
    };
    let qr = match field(map, "qr", "InviteView.qr")? {
        Value::Null => None,
        present => Some(decode_qr_view_value(present, "InviteView.qr")?),
    };
    Ok(InviteView {
        code,
        code_fingerprint,
        link,
        qr,
    })
}

fn encode_invite_view_value(value: &InviteView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("code".to_owned(), encode_utf8_bounded(value.code.expose(), 64, "InviteView.code")?);
    map.insert("code_fingerprint".to_owned(), encode_hex_fixed(&value.code_fingerprint, 16, "InviteView.code_fingerprint")?);
    map.insert(
        "link".to_owned(),
        match &value.link {
            None => Value::Null,
            Some(inner) => encode_utf8_bounded(inner.expose(), 5481, "InviteView.link")?,
        },
    );
    map.insert(
        "qr".to_owned(),
        match &value.qr {
            None => Value::Null,
            Some(inner) => encode_qr_view_value(inner)?,
        },
    );
    Ok(Value::Object(map))
}

fn decode_room_participation_view_value(value: &Value, context: &'static str) -> Result<RoomParticipationView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "minted" => Ok(RoomParticipationView::Minted),
        "joined" => Ok(RoomParticipationView::Joined),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_room_participation_view_value(value: &RoomParticipationView) -> Value {
    Value::from(match value {
        RoomParticipationView::Minted => "minted",
        RoomParticipationView::Joined => "joined",
    })
}

fn decode_source_acquisition_key_view_value(value: &Value, context: &'static str) -> Result<SourceAcquisitionKeyView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "generation", "request"], context)?;
    let card = hex_fixed(field(map, "card", "SourceAcquisitionKeyView.card")?, 16, "SourceAcquisitionKeyView.card")?;
    let generation = integer_u32(field(map, "generation", "SourceAcquisitionKeyView.generation")?, "SourceAcquisitionKeyView.generation")?;
    let request = hex_fixed(field(map, "request", "SourceAcquisitionKeyView.request")?, 32, "SourceAcquisitionKeyView.request")?;
    Ok(SourceAcquisitionKeyView {
        card,
        generation,
        request,
    })
}

fn encode_source_acquisition_key_view_value(value: &SourceAcquisitionKeyView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "SourceAcquisitionKeyView.card")?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    map.insert("request".to_owned(), encode_hex_fixed(&value.request, 32, "SourceAcquisitionKeyView.request")?);
    Ok(Value::Object(map))
}

fn decode_source_prompt_reason_view_value(value: &Value, context: &'static str) -> Result<SourcePromptReasonView, ReadError> {
    let text = value.as_str().ok_or(ReadError::Shape { context })?;
    match text {
        "initial" => Ok(SourcePromptReasonView::Initial),
        "unreadable" => Ok(SourcePromptReasonView::Unreadable),
        "permission_lost" => Ok(SourcePromptReasonView::PermissionLost),
        "storage_fault" => Ok(SourcePromptReasonView::StorageFault),
        "staging_failed" => Ok(SourcePromptReasonView::StagingFailed),
        "internal" => Ok(SourcePromptReasonView::Internal),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_source_prompt_reason_view_value(value: &SourcePromptReasonView) -> Value {
    Value::from(match value {
        SourcePromptReasonView::Initial => "initial",
        SourcePromptReasonView::Unreadable => "unreadable",
        SourcePromptReasonView::PermissionLost => "permission_lost",
        SourcePromptReasonView::StorageFault => "storage_fault",
        SourcePromptReasonView::StagingFailed => "staging_failed",
        SourcePromptReasonView::Internal => "internal",
    })
}

fn decode_accepted_source_offer_view_value(value: &Value, context: &'static str) -> Result<AcceptedSourceOfferView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["acquisition", "display_name", "reported_size"], context)?;
    let acquisition = decode_source_acquisition_key_view_value(field(map, "acquisition", "AcceptedSourceOfferView.acquisition")?, "AcceptedSourceOfferView.acquisition")?;
    let display_name = utf8_bounded(field(map, "display_name", "AcceptedSourceOfferView.display_name")?, 255, "AcceptedSourceOfferView.display_name")?;
    let reported_size = match field(map, "reported_size", "AcceptedSourceOfferView.reported_size")? {
        Value::Null => None,
        present => Some(integer(present, U63_MAX, "AcceptedSourceOfferView.reported_size")?),
    };
    Ok(AcceptedSourceOfferView {
        acquisition,
        display_name,
        reported_size,
    })
}

fn encode_accepted_source_offer_view_value(value: &AcceptedSourceOfferView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("acquisition".to_owned(), encode_source_acquisition_key_view_value(&value.acquisition)?);
    map.insert("display_name".to_owned(), encode_utf8_bounded(&value.display_name, 255, "AcceptedSourceOfferView.display_name")?);
    map.insert(
        "reported_size".to_owned(),
        match &value.reported_size {
            None => Value::Null,
            Some(inner) => encode_u63(*inner, "AcceptedSourceOfferView.reported_size")?,
        },
    );
    Ok(Value::Object(map))
}

fn decode_transfer_content_view_value(value: &Value, context: &'static str) -> Result<TransferContentView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["offered_name", "total"], context)?;
    let offered_name = utf8_bounded(field(map, "offered_name", "TransferContentView.offered_name")?, 255, "TransferContentView.offered_name")?;
    let total = integer(field(map, "total", "TransferContentView.total")?, U63_MAX, "TransferContentView.total")?;
    Ok(TransferContentView {
        offered_name,
        total,
    })
}

fn encode_transfer_content_view_value(value: &TransferContentView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("offered_name".to_owned(), encode_utf8_bounded(&value.offered_name, 255, "TransferContentView.offered_name")?);
    map.insert("total".to_owned(), encode_u63(value.total, "TransferContentView.total")?);
    Ok(Value::Object(map))
}

fn decode_source_not_required_view_value(value: &Value, context: &'static str) -> Result<SourceNotRequiredView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["peer_content"], context)?;
    let peer_content = match field(map, "peer_content", "SourceNotRequiredView.peer_content")? {
        Value::Null => None,
        present => Some(decode_transfer_content_view_value(present, "SourceNotRequiredView.peer_content")?),
    };
    Ok(SourceNotRequiredView {
        peer_content,
    })
}

fn encode_source_not_required_view_value(value: &SourceNotRequiredView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert(
        "peer_content".to_owned(),
        match &value.peer_content {
            None => Value::Null,
            Some(inner) => encode_transfer_content_view_value(inner)?,
        },
    );
    Ok(Value::Object(map))
}

fn decode_source_selectable_view_value(value: &Value, context: &'static str) -> Result<SourceSelectableView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["acquisition", "reason"], context)?;
    let acquisition = decode_source_acquisition_key_view_value(field(map, "acquisition", "SourceSelectableView.acquisition")?, "SourceSelectableView.acquisition")?;
    let reason = decode_source_prompt_reason_view_value(field(map, "reason", "SourceSelectableView.reason")?, "SourceSelectableView.reason")?;
    Ok(SourceSelectableView {
        acquisition,
        reason,
    })
}

fn encode_source_selectable_view_value(value: &SourceSelectableView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("acquisition".to_owned(), encode_source_acquisition_key_view_value(&value.acquisition)?);
    map.insert("reason".to_owned(), encode_source_prompt_reason_view_value(&value.reason));
    Ok(Value::Object(map))
}

fn decode_source_re_pick_required_view_value(value: &Value, context: &'static str) -> Result<SourceRePickRequiredView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["reason", "previous_offer"], context)?;
    let reason = decode_source_prompt_reason_view_value(field(map, "reason", "SourceRePickRequiredView.reason")?, "SourceRePickRequiredView.reason")?;
    let previous_offer = decode_accepted_source_offer_view_value(field(map, "previous_offer", "SourceRePickRequiredView.previous_offer")?, "SourceRePickRequiredView.previous_offer")?;
    Ok(SourceRePickRequiredView {
        reason,
        previous_offer,
    })
}

fn encode_source_re_pick_required_view_value(value: &SourceRePickRequiredView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("reason".to_owned(), encode_source_prompt_reason_view_value(&value.reason));
    map.insert("previous_offer".to_owned(), encode_accepted_source_offer_view_value(&value.previous_offer)?);
    Ok(Value::Object(map))
}

fn decode_source_selection_gate_view_value(value: &Value, context: &'static str) -> Result<SourceSelectionGateView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "selectable" => Ok(SourceSelectionGateView::Selectable(decode_source_selectable_view_value(payload(map, "SourceSelectionGateView.selectable")?, "SourceSelectionGateView.selectable")?)),
        "re_pick_required" => Ok(SourceSelectionGateView::RePickRequired(decode_source_re_pick_required_view_value(payload(map, "SourceSelectionGateView.re_pick_required")?, "SourceSelectionGateView.re_pick_required")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_source_selection_gate_view_value(value: &SourceSelectionGateView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        SourceSelectionGateView::Selectable(payload) => {
            map.insert("kind".to_owned(), Value::from("selectable"));
            map.insert("value".to_owned(), encode_source_selectable_view_value(payload)?);
        }
        SourceSelectionGateView::RePickRequired(payload) => {
            map.insert("kind".to_owned(), Value::from("re_pick_required"));
            map.insert("value".to_owned(), encode_source_re_pick_required_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_source_awaiting_selection_view_value(value: &Value, context: &'static str) -> Result<SourceAwaitingSelectionView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["selection"], context)?;
    let selection = decode_source_selection_gate_view_value(field(map, "selection", "SourceAwaitingSelectionView.selection")?, "SourceAwaitingSelectionView.selection")?;
    Ok(SourceAwaitingSelectionView {
        selection,
    })
}

fn encode_source_awaiting_selection_view_value(value: &SourceAwaitingSelectionView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("selection".to_owned(), encode_source_selection_gate_view_value(&value.selection)?);
    Ok(Value::Object(map))
}

fn decode_source_ready_view_value(value: &Value, context: &'static str) -> Result<SourceReadyView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["offer", "content"], context)?;
    let offer = decode_accepted_source_offer_view_value(field(map, "offer", "SourceReadyView.offer")?, "SourceReadyView.offer")?;
    let content = decode_transfer_content_view_value(field(map, "content", "SourceReadyView.content")?, "SourceReadyView.content")?;
    Ok(SourceReadyView {
        offer,
        content,
    })
}

fn encode_source_ready_view_value(value: &SourceReadyView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("offer".to_owned(), encode_accepted_source_offer_view_value(&value.offer)?);
    map.insert("content".to_owned(), encode_transfer_content_view_value(&value.content)?);
    Ok(Value::Object(map))
}

fn decode_source_lifecycle_view_value(value: &Value, context: &'static str) -> Result<SourceLifecycleView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "not_required" => Ok(SourceLifecycleView::NotRequired(decode_source_not_required_view_value(payload(map, "SourceLifecycleView.not_required")?, "SourceLifecycleView.not_required")?)),
        "awaiting_selection" => Ok(SourceLifecycleView::AwaitingSelection(decode_source_awaiting_selection_view_value(payload(map, "SourceLifecycleView.awaiting_selection")?, "SourceLifecycleView.awaiting_selection")?)),
        "acquiring" => Ok(SourceLifecycleView::Acquiring(decode_accepted_source_offer_view_value(payload(map, "SourceLifecycleView.acquiring")?, "SourceLifecycleView.acquiring")?)),
        "staging" => Ok(SourceLifecycleView::Staging(decode_accepted_source_offer_view_value(payload(map, "SourceLifecycleView.staging")?, "SourceLifecycleView.staging")?)),
        "ready" => Ok(SourceLifecycleView::Ready(decode_source_ready_view_value(payload(map, "SourceLifecycleView.ready")?, "SourceLifecycleView.ready")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_source_lifecycle_view_value(value: &SourceLifecycleView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        SourceLifecycleView::NotRequired(payload) => {
            map.insert("kind".to_owned(), Value::from("not_required"));
            map.insert("value".to_owned(), encode_source_not_required_view_value(payload)?);
        }
        SourceLifecycleView::AwaitingSelection(payload) => {
            map.insert("kind".to_owned(), Value::from("awaiting_selection"));
            map.insert("value".to_owned(), encode_source_awaiting_selection_view_value(payload)?);
        }
        SourceLifecycleView::Acquiring(payload) => {
            map.insert("kind".to_owned(), Value::from("acquiring"));
            map.insert("value".to_owned(), encode_accepted_source_offer_view_value(payload)?);
        }
        SourceLifecycleView::Staging(payload) => {
            map.insert("kind".to_owned(), Value::from("staging"));
            map.insert("value".to_owned(), encode_accepted_source_offer_view_value(payload)?);
        }
        SourceLifecycleView::Ready(payload) => {
            map.insert("kind".to_owned(), Value::from("ready"));
            map.insert("value".to_owned(), encode_source_ready_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_pick_source_action_view_value(value: &Value, context: &'static str) -> Result<PickSourceActionView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["acquisition"], context)?;
    let acquisition = decode_source_acquisition_key_view_value(field(map, "acquisition", "PickSourceActionView.acquisition")?, "PickSourceActionView.acquisition")?;
    Ok(PickSourceActionView {
        acquisition,
    })
}

fn encode_pick_source_action_view_value(value: &PickSourceActionView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("acquisition".to_owned(), encode_source_acquisition_key_view_value(&value.acquisition)?);
    Ok(Value::Object(map))
}

fn decode_card_action_view_value(value: &Value, context: &'static str) -> Result<CardActionView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "command" => Ok(CardActionView::Command(decode_command_kind_view_value(payload(map, "CardActionView.command")?, "CardActionView.command")?)),
        "pick_source" => Ok(CardActionView::PickSource(decode_pick_source_action_view_value(payload(map, "CardActionView.pick_source")?, "CardActionView.pick_source")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_card_action_view_value(value: &CardActionView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        CardActionView::Command(payload) => {
            map.insert("kind".to_owned(), Value::from("command"));
            map.insert("value".to_owned(), encode_command_kind_view_value(payload));
        }
        CardActionView::PickSource(payload) => {
            map.insert("kind".to_owned(), Value::from("pick_source"));
            map.insert("value".to_owned(), encode_pick_source_action_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_card_view_value(value: &Value, context: &'static str) -> Result<CardView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["identity", "participation", "direction", "source", "state", "quiescence", "generation", "phase", "bytes", "bytes_resumed", "outcome", "allowed_actions", "invite"], context)?;
    let identity = decode_identity_view_value(field(map, "identity", "CardView.identity")?, "CardView.identity")?;
    let participation = decode_room_participation_view_value(field(map, "participation", "CardView.participation")?, "CardView.participation")?;
    let direction = decode_direction_view_value(field(map, "direction", "CardView.direction")?, "CardView.direction")?;
    let source = decode_source_lifecycle_view_value(field(map, "source", "CardView.source")?, "CardView.source")?;
    let state = decode_product_state_view_value(field(map, "state", "CardView.state")?, "CardView.state")?;
    let quiescence = decode_quiescence_view_value(field(map, "quiescence", "CardView.quiescence")?, "CardView.quiescence")?;
    let generation = integer_u32(field(map, "generation", "CardView.generation")?, "CardView.generation")?;
    let phase = decode_phase_view_value(field(map, "phase", "CardView.phase")?, "CardView.phase")?;
    let bytes = integer(field(map, "bytes", "CardView.bytes")?, U63_MAX, "CardView.bytes")?;
    let bytes_resumed = integer(field(map, "bytes_resumed", "CardView.bytes_resumed")?, U63_MAX, "CardView.bytes_resumed")?;
    let outcome = match field(map, "outcome", "CardView.outcome")? {
        Value::Null => None,
        present => Some(decode_outcome_view_value(present, "CardView.outcome")?),
    };
    let allowed_actions = {
        let items = field(map, "allowed_actions", "CardView.allowed_actions")?.as_array().ok_or(ReadError::Shape { context: "CardView.allowed_actions" })?;
        if items.len() > 6 {
            return Err(ReadError::Bound { context: "CardView.allowed_actions" });
        }
        let mut collected = Vec::with_capacity(items.len());
        for item in items {
            collected.push(decode_card_action_view_value(item, "CardView.allowed_actions")?);
        }
        collected
    };
    let invite = match field(map, "invite", "CardView.invite")? {
        Value::Null => None,
        present => Some(decode_invite_view_value(present, "CardView.invite")?),
    };
    Ok(CardView {
        identity,
        participation,
        direction,
        source,
        state,
        quiescence,
        generation,
        phase,
        bytes,
        bytes_resumed,
        outcome,
        allowed_actions,
        invite,
    })
}

fn encode_card_view_value(value: &CardView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("identity".to_owned(), encode_identity_view_value(&value.identity)?);
    map.insert("participation".to_owned(), encode_room_participation_view_value(&value.participation));
    map.insert("direction".to_owned(), encode_direction_view_value(&value.direction));
    map.insert("source".to_owned(), encode_source_lifecycle_view_value(&value.source)?);
    map.insert("state".to_owned(), encode_product_state_view_value(&value.state)?);
    map.insert("quiescence".to_owned(), encode_quiescence_view_value(&value.quiescence)?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    map.insert("phase".to_owned(), encode_phase_view_value(&value.phase));
    map.insert("bytes".to_owned(), encode_u63(value.bytes, "CardView.bytes")?);
    map.insert("bytes_resumed".to_owned(), encode_u63(value.bytes_resumed, "CardView.bytes_resumed")?);
    map.insert(
        "outcome".to_owned(),
        match &value.outcome {
            None => Value::Null,
            Some(inner) => encode_outcome_view_value(inner)?,
        },
    );
    map.insert("allowed_actions".to_owned(), {
        if value.allowed_actions.len() > 6 {
            return Err(ReadError::Bound { context: "CardView.allowed_actions" });
        }
        let mut items = Vec::with_capacity(value.allowed_actions.len());
        for item in &value.allowed_actions {
            items.push(encode_card_action_view_value(item)?);
        }
        Value::Array(items)
    });
    map.insert(
        "invite".to_owned(),
        match &value.invite {
            None => Value::Null,
            Some(inner) => encode_invite_view_value(inner)?,
        },
    );
    Ok(Value::Object(map))
}

fn decode_duty_provenance_view_value(value: &Value, context: &'static str) -> Result<DutyProvenanceView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "generation", "request"], context)?;
    let card = hex_fixed(field(map, "card", "DutyProvenanceView.card")?, 16, "DutyProvenanceView.card")?;
    let generation = integer_u32(field(map, "generation", "DutyProvenanceView.generation")?, "DutyProvenanceView.generation")?;
    let request = hex_fixed(field(map, "request", "DutyProvenanceView.request")?, 32, "DutyProvenanceView.request")?;
    Ok(DutyProvenanceView {
        card,
        generation,
        request,
    })
}

fn encode_duty_provenance_view_value(value: &DutyProvenanceView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "DutyProvenanceView.card")?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    map.insert("request".to_owned(), encode_hex_fixed(&value.request, 32, "DutyProvenanceView.request")?);
    Ok(Value::Object(map))
}

fn decode_duty_view_value(value: &Value, context: &'static str) -> Result<DutyView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["provenance", "kind"], context)?;
    let provenance = decode_duty_provenance_view_value(field(map, "provenance", "DutyView.provenance")?, "DutyView.provenance")?;
    let kind = decode_duty_kind_view_value(field(map, "kind", "DutyView.kind")?, "DutyView.kind")?;
    Ok(DutyView {
        provenance,
        kind,
    })
}

fn encode_duty_view_value(value: &DutyView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("provenance".to_owned(), encode_duty_provenance_view_value(&value.provenance)?);
    map.insert("kind".to_owned(), encode_duty_kind_view_value(&value.kind));
    Ok(Value::Object(map))
}

fn decode_duty_frame_view_value(value: &Value, context: &'static str) -> Result<DutyFrameView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["duty", "action"], context)?;
    let duty = decode_duty_view_value(field(map, "duty", "DutyFrameView.duty")?, "DutyFrameView.duty")?;
    let action = decode_capability_action_view_value(field(map, "action", "DutyFrameView.action")?, "DutyFrameView.action")?;
    Ok(DutyFrameView {
        duty,
        action,
    })
}

fn encode_duty_frame_view_value(value: &DutyFrameView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("duty".to_owned(), encode_duty_view_value(&value.duty)?);
    map.insert("action".to_owned(), encode_capability_action_view_value(&value.action));
    Ok(Value::Object(map))
}

fn decode_card_update_kind_view_value(value: &Value, context: &'static str) -> Result<CardUpdateKindView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "snapshot" => Ok(CardUpdateKindView::Snapshot(decode_card_view_value(payload(map, "CardUpdateKindView.snapshot")?, "CardUpdateKindView.snapshot")?)),
        "progress" => Ok(CardUpdateKindView::Progress(decode_card_view_value(payload(map, "CardUpdateKindView.progress")?, "CardUpdateKindView.progress")?)),
        "state" => Ok(CardUpdateKindView::State(decode_card_view_value(payload(map, "CardUpdateKindView.state")?, "CardUpdateKindView.state")?)),
        "terminal" => Ok(CardUpdateKindView::Terminal(decode_card_view_value(payload(map, "CardUpdateKindView.terminal")?, "CardUpdateKindView.terminal")?)),
        "capability_duty" => Ok(CardUpdateKindView::CapabilityDuty(decode_duty_frame_view_value(payload(map, "CardUpdateKindView.capability_duty")?, "CardUpdateKindView.capability_duty")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_card_update_kind_view_value(value: &CardUpdateKindView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        CardUpdateKindView::Snapshot(payload) => {
            map.insert("kind".to_owned(), Value::from("snapshot"));
            map.insert("value".to_owned(), encode_card_view_value(payload)?);
        }
        CardUpdateKindView::Progress(payload) => {
            map.insert("kind".to_owned(), Value::from("progress"));
            map.insert("value".to_owned(), encode_card_view_value(payload)?);
        }
        CardUpdateKindView::State(payload) => {
            map.insert("kind".to_owned(), Value::from("state"));
            map.insert("value".to_owned(), encode_card_view_value(payload)?);
        }
        CardUpdateKindView::Terminal(payload) => {
            map.insert("kind".to_owned(), Value::from("terminal"));
            map.insert("value".to_owned(), encode_card_view_value(payload)?);
        }
        CardUpdateKindView::CapabilityDuty(payload) => {
            map.insert("kind".to_owned(), Value::from("capability_duty"));
            map.insert("value".to_owned(), encode_duty_frame_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_card_update_view_value(value: &Value, context: &'static str) -> Result<CardUpdateView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["epoch", "card", "kind"], context)?;
    let epoch = integer(field(map, "epoch", "CardUpdateView.epoch")?, U63_MAX, "CardUpdateView.epoch")?;
    let card = hex_fixed(field(map, "card", "CardUpdateView.card")?, 16, "CardUpdateView.card")?;
    let kind = decode_card_update_kind_view_value(field(map, "kind", "CardUpdateView.kind")?, "CardUpdateView.kind")?;
    Ok(CardUpdateView {
        epoch,
        card,
        kind,
    })
}

fn encode_card_update_view_value(value: &CardUpdateView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("epoch".to_owned(), encode_u63(value.epoch, "CardUpdateView.epoch")?);
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "CardUpdateView.card")?);
    map.insert("kind".to_owned(), encode_card_update_kind_view_value(&value.kind)?);
    Ok(Value::Object(map))
}

fn decode_lag_view_value(value: &Value, context: &'static str) -> Result<LagView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["epoch", "card", "missed"], context)?;
    let epoch = integer(field(map, "epoch", "LagView.epoch")?, U63_MAX, "LagView.epoch")?;
    let card = hex_fixed(field(map, "card", "LagView.card")?, 16, "LagView.card")?;
    let missed = decode_lossless_kind_view_value(field(map, "missed", "LagView.missed")?, "LagView.missed")?;
    Ok(LagView {
        epoch,
        card,
        missed,
    })
}

fn encode_lag_view_value(value: &LagView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("epoch".to_owned(), encode_u63(value.epoch, "LagView.epoch")?);
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "LagView.card")?);
    map.insert("missed".to_owned(), encode_lossless_kind_view_value(&value.missed));
    Ok(Value::Object(map))
}

fn decode_closed_view_value(value: &Value, context: &'static str) -> Result<ClosedView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["epoch", "card"], context)?;
    let epoch = integer(field(map, "epoch", "ClosedView.epoch")?, U63_MAX, "ClosedView.epoch")?;
    let card = hex_fixed(field(map, "card", "ClosedView.card")?, 16, "ClosedView.card")?;
    Ok(ClosedView {
        epoch,
        card,
    })
}

fn encode_closed_view_value(value: &ClosedView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("epoch".to_owned(), encode_u63(value.epoch, "ClosedView.epoch")?);
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "ClosedView.card")?);
    Ok(Value::Object(map))
}

fn decode_subscribe_rejected_view_value(value: &Value, context: &'static str) -> Result<SubscribeRejectedView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "reason"], context)?;
    let card = hex_fixed(field(map, "card", "SubscribeRejectedView.card")?, 16, "SubscribeRejectedView.card")?;
    let reason = decode_subscribe_rejection_view_value(field(map, "reason", "SubscribeRejectedView.reason")?, "SubscribeRejectedView.reason")?;
    Ok(SubscribeRejectedView {
        card,
        reason,
    })
}

fn encode_subscribe_rejected_view_value(value: &SubscribeRejectedView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "SubscribeRejectedView.card")?);
    map.insert("reason".to_owned(), encode_subscribe_rejection_view_value(&value.reason));
    Ok(Value::Object(map))
}

fn decode_session_key_view_value(value: &Value, context: &'static str) -> Result<SessionKeyView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "generation"], context)?;
    let card = hex_fixed(field(map, "card", "SessionKeyView.card")?, 16, "SessionKeyView.card")?;
    let generation = integer_u32(field(map, "generation", "SessionKeyView.generation")?, "SessionKeyView.generation")?;
    Ok(SessionKeyView {
        card,
        generation,
    })
}

fn encode_session_key_view_value(value: &SessionKeyView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "SessionKeyView.card")?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    Ok(Value::Object(map))
}

fn decode_evidence_progress_view_value(value: &Value, context: &'static str) -> Result<EvidenceProgressView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["transferred", "total"], context)?;
    let transferred = integer(field(map, "transferred", "EvidenceProgressView.transferred")?, U63_MAX, "EvidenceProgressView.transferred")?;
    let total = integer(field(map, "total", "EvidenceProgressView.total")?, U63_MAX, "EvidenceProgressView.total")?;
    Ok(EvidenceProgressView {
        transferred,
        total,
    })
}

fn encode_evidence_progress_view_value(value: &EvidenceProgressView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("transferred".to_owned(), encode_u63(value.transferred, "EvidenceProgressView.transferred")?);
    map.insert("total".to_owned(), encode_u63(value.total, "EvidenceProgressView.total")?);
    Ok(Value::Object(map))
}

fn decode_redacted_id_view_value(value: &Value, context: &'static str) -> Result<RedactedIdView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind"], context)?;
    let kind = decode_redacted_id_kind_view_value(field(map, "kind", "RedactedIdView.kind")?, "RedactedIdView.kind")?;
    Ok(RedactedIdView {
        kind,
    })
}

fn encode_redacted_id_view_value(value: &RedactedIdView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("kind".to_owned(), encode_redacted_id_kind_view_value(&value.kind));
    Ok(Value::Object(map))
}

fn decode_evidence_value_view_value(value: &Value, context: &'static str) -> Result<EvidenceValueView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "phase" => Ok(EvidenceValueView::Phase(decode_phase_view_value(payload(map, "EvidenceValueView.phase")?, "EvidenceValueView.phase")?)),
        "progress" => Ok(EvidenceValueView::Progress(decode_evidence_progress_view_value(payload(map, "EvidenceValueView.progress")?, "EvidenceValueView.progress")?)),
        "outcome" => Ok(EvidenceValueView::Outcome(decode_outcome_view_value(payload(map, "EvidenceValueView.outcome")?, "EvidenceValueView.outcome")?)),
        "identifier" => Ok(EvidenceValueView::Identifier(decode_redacted_id_view_value(payload(map, "EvidenceValueView.identifier")?, "EvidenceValueView.identifier")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_evidence_value_view_value(value: &EvidenceValueView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        EvidenceValueView::Phase(payload) => {
            map.insert("kind".to_owned(), Value::from("phase"));
            map.insert("value".to_owned(), encode_phase_view_value(payload));
        }
        EvidenceValueView::Progress(payload) => {
            map.insert("kind".to_owned(), Value::from("progress"));
            map.insert("value".to_owned(), encode_evidence_progress_view_value(payload)?);
        }
        EvidenceValueView::Outcome(payload) => {
            map.insert("kind".to_owned(), Value::from("outcome"));
            map.insert("value".to_owned(), encode_outcome_view_value(payload)?);
        }
        EvidenceValueView::Identifier(payload) => {
            map.insert("kind".to_owned(), Value::from("identifier"));
            map.insert("value".to_owned(), encode_redacted_id_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_degraded_view_value(value: &Value, context: &'static str) -> Result<DegradedView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["dropped_events"], context)?;
    let dropped_events = integer(field(map, "dropped_events", "DegradedView.dropped_events")?, U63_MAX, "DegradedView.dropped_events")?;
    Ok(DegradedView {
        dropped_events,
    })
}

fn encode_degraded_view_value(value: &DegradedView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("dropped_events".to_owned(), encode_u63(value.dropped_events, "DegradedView.dropped_events")?);
    Ok(Value::Object(map))
}

fn decode_diagnostics_status_view_value(value: &Value, context: &'static str) -> Result<DiagnosticsStatusView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "complete" => {
            unit_payload(map, "DiagnosticsStatusView.complete")?;
            Ok(DiagnosticsStatusView::Complete)
        }
        "degraded" => Ok(DiagnosticsStatusView::Degraded(decode_degraded_view_value(payload(map, "DiagnosticsStatusView.degraded")?, "DiagnosticsStatusView.degraded")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_diagnostics_status_view_value(value: &DiagnosticsStatusView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        DiagnosticsStatusView::Complete => {
            map.insert("kind".to_owned(), Value::from("complete"));
        }
        DiagnosticsStatusView::Degraded(payload) => {
            map.insert("kind".to_owned(), Value::from("degraded"));
            map.insert("value".to_owned(), encode_degraded_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_timeline_entry_view_value(value: &Value, context: &'static str) -> Result<TimelineEntryView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["sequence", "value"], context)?;
    let sequence = integer(field(map, "sequence", "TimelineEntryView.sequence")?, U63_MAX, "TimelineEntryView.sequence")?;
    let value = decode_evidence_value_view_value(field(map, "value", "TimelineEntryView.value")?, "TimelineEntryView.value")?;
    Ok(TimelineEntryView {
        sequence,
        value,
    })
}

fn encode_timeline_entry_view_value(value: &TimelineEntryView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("sequence".to_owned(), encode_u63(value.sequence, "TimelineEntryView.sequence")?);
    map.insert("value".to_owned(), encode_evidence_value_view_value(&value.value)?);
    Ok(Value::Object(map))
}

fn decode_evidence_timeline_view_value(value: &Value, context: &'static str) -> Result<EvidenceTimelineView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["session", "status", "entries"], context)?;
    let session = decode_session_key_view_value(field(map, "session", "EvidenceTimelineView.session")?, "EvidenceTimelineView.session")?;
    let status = decode_diagnostics_status_view_value(field(map, "status", "EvidenceTimelineView.status")?, "EvidenceTimelineView.status")?;
    let entries = {
        let items = field(map, "entries", "EvidenceTimelineView.entries")?.as_array().ok_or(ReadError::Shape { context: "EvidenceTimelineView.entries" })?;
        if items.len() > 1024 {
            return Err(ReadError::Bound { context: "EvidenceTimelineView.entries" });
        }
        let mut collected = Vec::with_capacity(items.len());
        for item in items {
            collected.push(decode_timeline_entry_view_value(item, "EvidenceTimelineView.entries")?);
        }
        collected
    };
    Ok(EvidenceTimelineView {
        session,
        status,
        entries,
    })
}

fn encode_evidence_timeline_view_value(value: &EvidenceTimelineView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("session".to_owned(), encode_session_key_view_value(&value.session)?);
    map.insert("status".to_owned(), encode_diagnostics_status_view_value(&value.status)?);
    map.insert("entries".to_owned(), {
        if value.entries.len() > 1024 {
            return Err(ReadError::Bound { context: "EvidenceTimelineView.entries" });
        }
        let mut items = Vec::with_capacity(value.entries.len());
        for item in &value.entries {
            items.push(encode_timeline_entry_view_value(item)?);
        }
        Value::Array(items)
    });
    Ok(Value::Object(map))
}

fn decode_protocol_manifest_view_value(value: &Value, context: &'static str) -> Result<ProtocolManifestView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["set_id", "data_alpn", "data_magic", "data_wire_version"], context)?;
    let set_id = ascii_bounded(field(map, "set_id", "ProtocolManifestView.set_id")?, 64, "ProtocolManifestView.set_id")?;
    let data_alpn = hex_variable(field(map, "data_alpn", "ProtocolManifestView.data_alpn")?, 64, "ProtocolManifestView.data_alpn")?;
    let data_magic = hex_variable(field(map, "data_magic", "ProtocolManifestView.data_magic")?, 32, "ProtocolManifestView.data_magic")?;
    let data_wire_version = integer_u16(field(map, "data_wire_version", "ProtocolManifestView.data_wire_version")?, "ProtocolManifestView.data_wire_version")?;
    Ok(ProtocolManifestView {
        set_id,
        data_alpn,
        data_magic,
        data_wire_version,
    })
}

fn encode_protocol_manifest_view_value(value: &ProtocolManifestView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("set_id".to_owned(), encode_ascii_bounded(&value.set_id, 64, "ProtocolManifestView.set_id")?);
    map.insert("data_alpn".to_owned(), encode_hex_variable(&value.data_alpn, 64, "ProtocolManifestView.data_alpn")?);
    map.insert("data_magic".to_owned(), encode_hex_variable(&value.data_magic, 32, "ProtocolManifestView.data_magic")?);
    map.insert("data_wire_version".to_owned(), Value::from(value.data_wire_version));
    Ok(Value::Object(map))
}

fn decode_abi_schema_manifest_view_value(value: &Value, context: &'static str) -> Result<AbiSchemaManifestView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["read_binding_schema_id", "command_binding_schema_id", "capability_binding_schema_id", "evidence_rust_abi_id", "evidence_timeline_schema_id", "mailbox_receipt_schema_id", "operation_envelope_schema_id"], context)?;
    let read_binding_schema_id = ascii_bounded(field(map, "read_binding_schema_id", "AbiSchemaManifestView.read_binding_schema_id")?, 64, "AbiSchemaManifestView.read_binding_schema_id")?;
    let command_binding_schema_id = ascii_bounded(field(map, "command_binding_schema_id", "AbiSchemaManifestView.command_binding_schema_id")?, 64, "AbiSchemaManifestView.command_binding_schema_id")?;
    let capability_binding_schema_id = ascii_bounded(field(map, "capability_binding_schema_id", "AbiSchemaManifestView.capability_binding_schema_id")?, 64, "AbiSchemaManifestView.capability_binding_schema_id")?;
    let evidence_rust_abi_id = ascii_bounded(field(map, "evidence_rust_abi_id", "AbiSchemaManifestView.evidence_rust_abi_id")?, 64, "AbiSchemaManifestView.evidence_rust_abi_id")?;
    let evidence_timeline_schema_id = ascii_bounded(field(map, "evidence_timeline_schema_id", "AbiSchemaManifestView.evidence_timeline_schema_id")?, 64, "AbiSchemaManifestView.evidence_timeline_schema_id")?;
    let mailbox_receipt_schema_id = ascii_bounded(field(map, "mailbox_receipt_schema_id", "AbiSchemaManifestView.mailbox_receipt_schema_id")?, 64, "AbiSchemaManifestView.mailbox_receipt_schema_id")?;
    let operation_envelope_schema_id = ascii_bounded(field(map, "operation_envelope_schema_id", "AbiSchemaManifestView.operation_envelope_schema_id")?, 64, "AbiSchemaManifestView.operation_envelope_schema_id")?;
    Ok(AbiSchemaManifestView {
        read_binding_schema_id,
        command_binding_schema_id,
        capability_binding_schema_id,
        evidence_rust_abi_id,
        evidence_timeline_schema_id,
        mailbox_receipt_schema_id,
        operation_envelope_schema_id,
    })
}

fn encode_abi_schema_manifest_view_value(value: &AbiSchemaManifestView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("read_binding_schema_id".to_owned(), encode_ascii_bounded(&value.read_binding_schema_id, 64, "AbiSchemaManifestView.read_binding_schema_id")?);
    map.insert("command_binding_schema_id".to_owned(), encode_ascii_bounded(&value.command_binding_schema_id, 64, "AbiSchemaManifestView.command_binding_schema_id")?);
    map.insert("capability_binding_schema_id".to_owned(), encode_ascii_bounded(&value.capability_binding_schema_id, 64, "AbiSchemaManifestView.capability_binding_schema_id")?);
    map.insert("evidence_rust_abi_id".to_owned(), encode_ascii_bounded(&value.evidence_rust_abi_id, 64, "AbiSchemaManifestView.evidence_rust_abi_id")?);
    map.insert("evidence_timeline_schema_id".to_owned(), encode_ascii_bounded(&value.evidence_timeline_schema_id, 64, "AbiSchemaManifestView.evidence_timeline_schema_id")?);
    map.insert("mailbox_receipt_schema_id".to_owned(), encode_ascii_bounded(&value.mailbox_receipt_schema_id, 64, "AbiSchemaManifestView.mailbox_receipt_schema_id")?);
    map.insert("operation_envelope_schema_id".to_owned(), encode_ascii_bounded(&value.operation_envelope_schema_id, 64, "AbiSchemaManifestView.operation_envelope_schema_id")?);
    Ok(Value::Object(map))
}

fn decode_deployment_manifest_view_value(value: &Value, context: &'static str) -> Result<DeploymentManifestView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["environment", "rendezvous_endpoint", "relay_url"], context)?;
    let environment = ascii_bounded(field(map, "environment", "DeploymentManifestView.environment")?, 32, "DeploymentManifestView.environment")?;
    let rendezvous_endpoint = ascii_bounded(field(map, "rendezvous_endpoint", "DeploymentManifestView.rendezvous_endpoint")?, 1024, "DeploymentManifestView.rendezvous_endpoint")?;
    let relay_url = ascii_bounded(field(map, "relay_url", "DeploymentManifestView.relay_url")?, 2048, "DeploymentManifestView.relay_url")?;
    Ok(DeploymentManifestView {
        environment,
        rendezvous_endpoint,
        relay_url,
    })
}

fn encode_deployment_manifest_view_value(value: &DeploymentManifestView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("environment".to_owned(), encode_ascii_bounded(&value.environment, 32, "DeploymentManifestView.environment")?);
    map.insert("rendezvous_endpoint".to_owned(), encode_ascii_bounded(&value.rendezvous_endpoint, 1024, "DeploymentManifestView.rendezvous_endpoint")?);
    map.insert("relay_url".to_owned(), encode_ascii_bounded(&value.relay_url, 2048, "DeploymentManifestView.relay_url")?);
    Ok(Value::Object(map))
}

fn decode_build_manifest_view_value(value: &Value, context: &'static str) -> Result<BuildManifestView, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["package_version", "protocol", "abi_schema", "deployment"], context)?;
    let package_version = ascii_bounded(field(map, "package_version", "BuildManifestView.package_version")?, 32, "BuildManifestView.package_version")?;
    let protocol = decode_protocol_manifest_view_value(field(map, "protocol", "BuildManifestView.protocol")?, "BuildManifestView.protocol")?;
    let abi_schema = decode_abi_schema_manifest_view_value(field(map, "abi_schema", "BuildManifestView.abi_schema")?, "BuildManifestView.abi_schema")?;
    let deployment = decode_deployment_manifest_view_value(field(map, "deployment", "BuildManifestView.deployment")?, "BuildManifestView.deployment")?;
    Ok(BuildManifestView {
        package_version,
        protocol,
        abi_schema,
        deployment,
    })
}

fn encode_build_manifest_view_value(value: &BuildManifestView) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("package_version".to_owned(), encode_ascii_bounded(&value.package_version, 32, "BuildManifestView.package_version")?);
    map.insert("protocol".to_owned(), encode_protocol_manifest_view_value(&value.protocol)?);
    map.insert("abi_schema".to_owned(), encode_abi_schema_manifest_view_value(&value.abi_schema)?);
    map.insert("deployment".to_owned(), encode_deployment_manifest_view_value(&value.deployment)?);
    Ok(Value::Object(map))
}

fn decode_read_body_value(value: &Value, context: &'static str) -> Result<ReadBody, ReadError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(ReadError::Shape { context })?;
    match kind {
        "card_update" => Ok(ReadBody::CardUpdate(decode_card_update_view_value(payload(map, "ReadBody.card_update")?, "ReadBody.card_update")?)),
        "lag" => Ok(ReadBody::Lag(decode_lag_view_value(payload(map, "ReadBody.lag")?, "ReadBody.lag")?)),
        "closed" => Ok(ReadBody::Closed(decode_closed_view_value(payload(map, "ReadBody.closed")?, "ReadBody.closed")?)),
        "subscribe_rejected" => Ok(ReadBody::SubscribeRejected(decode_subscribe_rejected_view_value(payload(map, "ReadBody.subscribe_rejected")?, "ReadBody.subscribe_rejected")?)),
        "evidence" => Ok(ReadBody::Evidence(decode_evidence_timeline_view_value(payload(map, "ReadBody.evidence")?, "ReadBody.evidence")?)),
        "build_manifest" => Ok(ReadBody::BuildManifest(decode_build_manifest_view_value(payload(map, "ReadBody.build_manifest")?, "ReadBody.build_manifest")?)),
        _ => Err(ReadError::UnknownVariant { context }),
    }
}

fn encode_read_body_value(value: &ReadBody) -> Result<Value, ReadError> {
    let mut map = Map::new();
    match value {
        ReadBody::CardUpdate(payload) => {
            map.insert("kind".to_owned(), Value::from("card_update"));
            map.insert("value".to_owned(), encode_card_update_view_value(payload)?);
        }
        ReadBody::Lag(payload) => {
            map.insert("kind".to_owned(), Value::from("lag"));
            map.insert("value".to_owned(), encode_lag_view_value(payload)?);
        }
        ReadBody::Closed(payload) => {
            map.insert("kind".to_owned(), Value::from("closed"));
            map.insert("value".to_owned(), encode_closed_view_value(payload)?);
        }
        ReadBody::SubscribeRejected(payload) => {
            map.insert("kind".to_owned(), Value::from("subscribe_rejected"));
            map.insert("value".to_owned(), encode_subscribe_rejected_view_value(payload)?);
        }
        ReadBody::Evidence(payload) => {
            map.insert("kind".to_owned(), Value::from("evidence"));
            map.insert("value".to_owned(), encode_evidence_timeline_view_value(payload)?);
        }
        ReadBody::BuildManifest(payload) => {
            map.insert("kind".to_owned(), Value::from("build_manifest"));
            map.insert("value".to_owned(), encode_build_manifest_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_read_frame_value(value: &Value, context: &'static str) -> Result<ReadFrame, ReadError> {
    let map = frame_object(value, context)?;
    match map.get("schema").and_then(Value::as_str) {
        Some(schema) if schema == READ_SCHEMA_ID => {}
        Some(_) => return Err(ReadError::UnknownSchema),
        None => return Err(ReadError::Shape { context: "ReadFrame.schema" }),
    }
    known_keys(map, &["schema", "body"], context)?;
    let body = decode_read_body_value(field(map, "body", "ReadFrame.body")?, "ReadFrame.body")?;
    Ok(ReadFrame {
        body,
    })
}

fn encode_read_frame_value(value: &ReadFrame) -> Result<Value, ReadError> {
    let mut map = Map::new();
    map.insert("schema".to_owned(), Value::from(READ_SCHEMA_ID));
    map.insert("body".to_owned(), encode_read_body_value(&value.body)?);
    Ok(Value::Object(map))
}

