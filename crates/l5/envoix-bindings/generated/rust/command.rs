// @generated from schema/command.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.

use serde_json::{Map, Value};

use envoix_types::Secret;

pub const COMMAND_SCHEMA_ID: &str = "envoix/binding/command/4";
pub const COMMAND_MAX_FRAME_BYTES: usize = 1048576;

// Contract rules frozen by schema/command.schema.
pub const NEWEST_ATTACHMENT_COMMANDS: bool = true;
pub const RETRY_HORIZON_COMPLETIONS: u32 = 256;
pub const SUPERSESSION_INERT_PRE_ACCEPTANCE_ONLY: bool = true;

const U63_MAX: u64 = 9_223_372_036_854_775_807;

/// Typed codec failure. It carries only static schema context, never a
/// fragment of the (possibly hostile) input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError {
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
pub enum CommandView {
    Pause,
    Cancel,
    Resume,
    Remove,
    RePickSource,
}

impl CommandView {
    pub const ALL: [Self; 5] = [
        Self::Pause,
        Self::Cancel,
        Self::Resume,
        Self::Remove,
        Self::RePickSource,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseCauseView {
    Local,
    Peer,
    Lost,
}

impl PauseCauseView {
    pub const ALL: [Self; 3] = [
        Self::Local,
        Self::Peer,
        Self::Lost,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PausedStateView {
    pub origin: PauseCauseView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispositionView {
    Preparing,
    Waiting,
    Connecting,
    Verifying,
    Transferring,
    Confirming,
    Paused(PausedStateView),
    Unconfirmed,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitView {
    pub card: String,
    pub epoch: u64,
    pub command_id: String,
    pub command: CommandView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendSourceView {
    pub display_name: String,
    pub total: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinInviteView {
    pub invite: Secret<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateIntentView {
    Send(SendSourceView),
    Join(JoinInviteView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateView {
    pub intent: CreateIntentView,
    pub request_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontendIntentView {
    Command(SubmitView),
    Create(CreateView),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionView {
    UnknownCard,
    StaleEpoch,
    Superseded,
    AtCapacity,
    RuntimeStopped,
    Interrupted,
    Internal,
}

impl RejectionView {
    pub const ALL: [Self; 7] = [
        Self::UnknownCard,
        Self::StaleEpoch,
        Self::Superseded,
        Self::AtCapacity,
        Self::RuntimeStopped,
        Self::Interrupted,
        Self::Internal,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcceptanceView {
    Accepted,
    Duplicate(DispositionView),
    Conflict(CommandView),
    Rejected(RejectionView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandAcceptanceView {
    pub command_id: String,
    pub acceptance: AcceptanceView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionView {
    Committed(DispositionView),
    CommitFailed(DispositionView),
    Interrupted,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandCompletionView {
    pub command_id: String,
    pub completion: CompletionView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateRefusalView {
    InviteNotRecognized,
    InviteBareRoomCode,
    InviteMalformed,
    InviteTooLong,
    InviteUnsupported,
    InviteRoleUnsupported,
    NameTooLong,
    StorageFault,
    Internal,
}

impl CreateRefusalView {
    pub const ALL: [Self; 9] = [
        Self::InviteNotRecognized,
        Self::InviteBareRoomCode,
        Self::InviteMalformed,
        Self::InviteTooLong,
        Self::InviteUnsupported,
        Self::InviteRoleUnsupported,
        Self::NameTooLong,
        Self::StorageFault,
        Self::Internal,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardCreatedView {
    pub card: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateOutcomeView {
    Created(CardCreatedView),
    Refused(CreateRefusalView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateResultView {
    pub outcome: CreateOutcomeView,
    pub request_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandBody {
    Intent(FrontendIntentView),
    Acceptance(CommandAcceptanceView),
    Completion(CommandCompletionView),
    CreateResult(CreateResultView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandFrame {
    pub body: CommandBody,
}

/// Decodes and validates one frame. Every failure is a typed
/// [`CommandError`]; no input, however hostile, panics or misparses.
pub fn decode_command_frame(bytes: &[u8]) -> Result<CommandFrame, CommandError> {
    if bytes.len() > COMMAND_MAX_FRAME_BYTES {
        return Err(CommandError::FrameTooLarge);
    }
    let value = strict_json(bytes)?;
    decode_command_frame_value(&value, "CommandFrame")
}

/// Encodes one frame, stamping the schema envelope and enforcing the
/// same bounds the decoder checks.
pub fn encode_command_frame(frame: &CommandFrame) -> Result<Vec<u8>, CommandError> {
    let value = encode_command_frame_value(frame)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| CommandError::MalformedJson)?;
    if bytes.len() > COMMAND_MAX_FRAME_BYTES {
        return Err(CommandError::FrameTooLarge);
    }
    Ok(bytes)
}

/// Parses JSON while rejecting duplicate object keys at any depth and
/// trailing input. A duplicated key is the smuggling shape: a first-wins
/// upstream parser would see a different value than a last-wins one applies.
fn strict_json(bytes: &[u8]) -> Result<Value, CommandError> {
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
        .map_err(|_| CommandError::MalformedJson)?;
    deserializer.end().map_err(|_| CommandError::MalformedJson)?;
    Ok(value)
}

fn frame_object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, CommandError> {
    value.as_object().ok_or(CommandError::Shape { context })
}

fn known_keys(map: &Map<String, Value>, allowed: &[&str], context: &'static str) -> Result<(), CommandError> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(CommandError::UnknownField { context });
        }
    }
    Ok(())
}

fn field<'a>(map: &'a Map<String, Value>, key: &str, context: &'static str) -> Result<&'a Value, CommandError> {
    map.get(key).ok_or(CommandError::Shape { context })
}

fn integer(value: &Value, max: u64, context: &'static str) -> Result<u64, CommandError> {
    let number = value.as_u64().ok_or(CommandError::Shape { context })?;
    if number > max {
        return Err(CommandError::Range { context });
    }
    Ok(number)
}

fn encode_u63(number: u64, context: &'static str) -> Result<Value, CommandError> {
    if number > U63_MAX {
        return Err(CommandError::Range { context });
    }
    Ok(Value::from(number))
}

fn hex_chars(text: &str) -> bool {
    text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_fixed(value: &Value, chars: usize, context: &'static str) -> Result<String, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    encode_hex_fixed(text, chars, context)?;
    Ok(text.to_owned())
}

fn encode_hex_fixed(text: &str, chars: usize, context: &'static str) -> Result<Value, CommandError> {
    if text.len() != chars || !hex_chars(text) {
        return Err(CommandError::Bound { context });
    }
    Ok(Value::from(text))
}

fn utf8_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    encode_utf8_bounded(text, max_bytes, context)?;
    Ok(text.to_owned())
}

fn encode_utf8_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, CommandError> {
    if text.len() > max_bytes {
        return Err(CommandError::Bound { context });
    }
    Ok(Value::from(text))
}

fn payload<'a>(map: &'a Map<String, Value>, context: &'static str) -> Result<&'a Value, CommandError> {
    match map.get("value") {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(CommandError::Shape { context }),
    }
}

fn unit_payload(map: &Map<String, Value>, context: &'static str) -> Result<(), CommandError> {
    match map.get("value") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(CommandError::Shape { context }),
    }
}

fn decode_command_view_value(value: &Value, context: &'static str) -> Result<CommandView, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    match text {
        "pause" => Ok(CommandView::Pause),
        "cancel" => Ok(CommandView::Cancel),
        "resume" => Ok(CommandView::Resume),
        "remove" => Ok(CommandView::Remove),
        "re_pick_source" => Ok(CommandView::RePickSource),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_command_view_value(value: &CommandView) -> Value {
    Value::from(match value {
        CommandView::Pause => "pause",
        CommandView::Cancel => "cancel",
        CommandView::Resume => "resume",
        CommandView::Remove => "remove",
        CommandView::RePickSource => "re_pick_source",
    })
}

fn decode_pause_cause_view_value(value: &Value, context: &'static str) -> Result<PauseCauseView, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    match text {
        "local" => Ok(PauseCauseView::Local),
        "peer" => Ok(PauseCauseView::Peer),
        "lost" => Ok(PauseCauseView::Lost),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_pause_cause_view_value(value: &PauseCauseView) -> Value {
    Value::from(match value {
        PauseCauseView::Local => "local",
        PauseCauseView::Peer => "peer",
        PauseCauseView::Lost => "lost",
    })
}

fn decode_paused_state_view_value(value: &Value, context: &'static str) -> Result<PausedStateView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["origin"], context)?;
    let origin = decode_pause_cause_view_value(field(map, "origin", "PausedStateView.origin")?, "PausedStateView.origin")?;
    Ok(PausedStateView {
        origin,
    })
}

fn encode_paused_state_view_value(value: &PausedStateView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("origin".to_owned(), encode_pause_cause_view_value(&value.origin));
    Ok(Value::Object(map))
}

fn decode_disposition_view_value(value: &Value, context: &'static str) -> Result<DispositionView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "preparing" => {
            unit_payload(map, "DispositionView.preparing")?;
            Ok(DispositionView::Preparing)
        }
        "waiting" => {
            unit_payload(map, "DispositionView.waiting")?;
            Ok(DispositionView::Waiting)
        }
        "connecting" => {
            unit_payload(map, "DispositionView.connecting")?;
            Ok(DispositionView::Connecting)
        }
        "verifying" => {
            unit_payload(map, "DispositionView.verifying")?;
            Ok(DispositionView::Verifying)
        }
        "transferring" => {
            unit_payload(map, "DispositionView.transferring")?;
            Ok(DispositionView::Transferring)
        }
        "confirming" => {
            unit_payload(map, "DispositionView.confirming")?;
            Ok(DispositionView::Confirming)
        }
        "paused" => Ok(DispositionView::Paused(decode_paused_state_view_value(payload(map, "DispositionView.paused")?, "DispositionView.paused")?)),
        "unconfirmed" => {
            unit_payload(map, "DispositionView.unconfirmed")?;
            Ok(DispositionView::Unconfirmed)
        }
        "completed" => {
            unit_payload(map, "DispositionView.completed")?;
            Ok(DispositionView::Completed)
        }
        "failed" => {
            unit_payload(map, "DispositionView.failed")?;
            Ok(DispositionView::Failed)
        }
        "cancelled" => {
            unit_payload(map, "DispositionView.cancelled")?;
            Ok(DispositionView::Cancelled)
        }
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_disposition_view_value(value: &DispositionView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        DispositionView::Preparing => {
            map.insert("kind".to_owned(), Value::from("preparing"));
        }
        DispositionView::Waiting => {
            map.insert("kind".to_owned(), Value::from("waiting"));
        }
        DispositionView::Connecting => {
            map.insert("kind".to_owned(), Value::from("connecting"));
        }
        DispositionView::Verifying => {
            map.insert("kind".to_owned(), Value::from("verifying"));
        }
        DispositionView::Transferring => {
            map.insert("kind".to_owned(), Value::from("transferring"));
        }
        DispositionView::Confirming => {
            map.insert("kind".to_owned(), Value::from("confirming"));
        }
        DispositionView::Paused(payload) => {
            map.insert("kind".to_owned(), Value::from("paused"));
            map.insert("value".to_owned(), encode_paused_state_view_value(payload)?);
        }
        DispositionView::Unconfirmed => {
            map.insert("kind".to_owned(), Value::from("unconfirmed"));
        }
        DispositionView::Completed => {
            map.insert("kind".to_owned(), Value::from("completed"));
        }
        DispositionView::Failed => {
            map.insert("kind".to_owned(), Value::from("failed"));
        }
        DispositionView::Cancelled => {
            map.insert("kind".to_owned(), Value::from("cancelled"));
        }
    }
    Ok(Value::Object(map))
}

fn decode_submit_view_value(value: &Value, context: &'static str) -> Result<SubmitView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card", "epoch", "command_id", "command"], context)?;
    let card = hex_fixed(field(map, "card", "SubmitView.card")?, 16, "SubmitView.card")?;
    let epoch = integer(field(map, "epoch", "SubmitView.epoch")?, U63_MAX, "SubmitView.epoch")?;
    let command_id = hex_fixed(field(map, "command_id", "SubmitView.command_id")?, 32, "SubmitView.command_id")?;
    let command = decode_command_view_value(field(map, "command", "SubmitView.command")?, "SubmitView.command")?;
    Ok(SubmitView {
        card,
        epoch,
        command_id,
        command,
    })
}

fn encode_submit_view_value(value: &SubmitView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "SubmitView.card")?);
    map.insert("epoch".to_owned(), encode_u63(value.epoch, "SubmitView.epoch")?);
    map.insert("command_id".to_owned(), encode_hex_fixed(&value.command_id, 32, "SubmitView.command_id")?);
    map.insert("command".to_owned(), encode_command_view_value(&value.command));
    Ok(Value::Object(map))
}

fn decode_send_source_view_value(value: &Value, context: &'static str) -> Result<SendSourceView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["display_name", "total"], context)?;
    let display_name = utf8_bounded(field(map, "display_name", "SendSourceView.display_name")?, 1020, "SendSourceView.display_name")?;
    let total = integer(field(map, "total", "SendSourceView.total")?, U63_MAX, "SendSourceView.total")?;
    Ok(SendSourceView {
        display_name,
        total,
    })
}

fn encode_send_source_view_value(value: &SendSourceView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("display_name".to_owned(), encode_utf8_bounded(&value.display_name, 1020, "SendSourceView.display_name")?);
    map.insert("total".to_owned(), encode_u63(value.total, "SendSourceView.total")?);
    Ok(Value::Object(map))
}

fn decode_join_invite_view_value(value: &Value, context: &'static str) -> Result<JoinInviteView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["invite"], context)?;
    let invite = Secret::new(utf8_bounded(field(map, "invite", "JoinInviteView.invite")?, 16384, "JoinInviteView.invite")?);
    Ok(JoinInviteView {
        invite,
    })
}

fn encode_join_invite_view_value(value: &JoinInviteView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("invite".to_owned(), encode_utf8_bounded(value.invite.expose(), 16384, "JoinInviteView.invite")?);
    Ok(Value::Object(map))
}

fn decode_create_intent_view_value(value: &Value, context: &'static str) -> Result<CreateIntentView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "send" => Ok(CreateIntentView::Send(decode_send_source_view_value(payload(map, "CreateIntentView.send")?, "CreateIntentView.send")?)),
        "join" => Ok(CreateIntentView::Join(decode_join_invite_view_value(payload(map, "CreateIntentView.join")?, "CreateIntentView.join")?)),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_create_intent_view_value(value: &CreateIntentView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        CreateIntentView::Send(payload) => {
            map.insert("kind".to_owned(), Value::from("send"));
            map.insert("value".to_owned(), encode_send_source_view_value(payload)?);
        }
        CreateIntentView::Join(payload) => {
            map.insert("kind".to_owned(), Value::from("join"));
            map.insert("value".to_owned(), encode_join_invite_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_create_view_value(value: &Value, context: &'static str) -> Result<CreateView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["intent", "request_id"], context)?;
    let intent = decode_create_intent_view_value(field(map, "intent", "CreateView.intent")?, "CreateView.intent")?;
    let request_id = hex_fixed(field(map, "request_id", "CreateView.request_id")?, 32, "CreateView.request_id")?;
    Ok(CreateView {
        intent,
        request_id,
    })
}

fn encode_create_view_value(value: &CreateView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("intent".to_owned(), encode_create_intent_view_value(&value.intent)?);
    map.insert("request_id".to_owned(), encode_hex_fixed(&value.request_id, 32, "CreateView.request_id")?);
    Ok(Value::Object(map))
}

fn decode_frontend_intent_view_value(value: &Value, context: &'static str) -> Result<FrontendIntentView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "command" => Ok(FrontendIntentView::Command(decode_submit_view_value(payload(map, "FrontendIntentView.command")?, "FrontendIntentView.command")?)),
        "create" => Ok(FrontendIntentView::Create(decode_create_view_value(payload(map, "FrontendIntentView.create")?, "FrontendIntentView.create")?)),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_frontend_intent_view_value(value: &FrontendIntentView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        FrontendIntentView::Command(payload) => {
            map.insert("kind".to_owned(), Value::from("command"));
            map.insert("value".to_owned(), encode_submit_view_value(payload)?);
        }
        FrontendIntentView::Create(payload) => {
            map.insert("kind".to_owned(), Value::from("create"));
            map.insert("value".to_owned(), encode_create_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_rejection_view_value(value: &Value, context: &'static str) -> Result<RejectionView, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    match text {
        "unknown_card" => Ok(RejectionView::UnknownCard),
        "stale_epoch" => Ok(RejectionView::StaleEpoch),
        "superseded" => Ok(RejectionView::Superseded),
        "at_capacity" => Ok(RejectionView::AtCapacity),
        "runtime_stopped" => Ok(RejectionView::RuntimeStopped),
        "interrupted" => Ok(RejectionView::Interrupted),
        "internal" => Ok(RejectionView::Internal),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_rejection_view_value(value: &RejectionView) -> Value {
    Value::from(match value {
        RejectionView::UnknownCard => "unknown_card",
        RejectionView::StaleEpoch => "stale_epoch",
        RejectionView::Superseded => "superseded",
        RejectionView::AtCapacity => "at_capacity",
        RejectionView::RuntimeStopped => "runtime_stopped",
        RejectionView::Interrupted => "interrupted",
        RejectionView::Internal => "internal",
    })
}

fn decode_acceptance_view_value(value: &Value, context: &'static str) -> Result<AcceptanceView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "accepted" => {
            unit_payload(map, "AcceptanceView.accepted")?;
            Ok(AcceptanceView::Accepted)
        }
        "duplicate" => Ok(AcceptanceView::Duplicate(decode_disposition_view_value(payload(map, "AcceptanceView.duplicate")?, "AcceptanceView.duplicate")?)),
        "conflict" => Ok(AcceptanceView::Conflict(decode_command_view_value(payload(map, "AcceptanceView.conflict")?, "AcceptanceView.conflict")?)),
        "rejected" => Ok(AcceptanceView::Rejected(decode_rejection_view_value(payload(map, "AcceptanceView.rejected")?, "AcceptanceView.rejected")?)),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_acceptance_view_value(value: &AcceptanceView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        AcceptanceView::Accepted => {
            map.insert("kind".to_owned(), Value::from("accepted"));
        }
        AcceptanceView::Duplicate(payload) => {
            map.insert("kind".to_owned(), Value::from("duplicate"));
            map.insert("value".to_owned(), encode_disposition_view_value(payload)?);
        }
        AcceptanceView::Conflict(payload) => {
            map.insert("kind".to_owned(), Value::from("conflict"));
            map.insert("value".to_owned(), encode_command_view_value(payload));
        }
        AcceptanceView::Rejected(payload) => {
            map.insert("kind".to_owned(), Value::from("rejected"));
            map.insert("value".to_owned(), encode_rejection_view_value(payload));
        }
    }
    Ok(Value::Object(map))
}

fn decode_command_acceptance_view_value(value: &Value, context: &'static str) -> Result<CommandAcceptanceView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["command_id", "acceptance"], context)?;
    let command_id = hex_fixed(field(map, "command_id", "CommandAcceptanceView.command_id")?, 32, "CommandAcceptanceView.command_id")?;
    let acceptance = decode_acceptance_view_value(field(map, "acceptance", "CommandAcceptanceView.acceptance")?, "CommandAcceptanceView.acceptance")?;
    Ok(CommandAcceptanceView {
        command_id,
        acceptance,
    })
}

fn encode_command_acceptance_view_value(value: &CommandAcceptanceView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("command_id".to_owned(), encode_hex_fixed(&value.command_id, 32, "CommandAcceptanceView.command_id")?);
    map.insert("acceptance".to_owned(), encode_acceptance_view_value(&value.acceptance)?);
    Ok(Value::Object(map))
}

fn decode_completion_view_value(value: &Value, context: &'static str) -> Result<CompletionView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "committed" => Ok(CompletionView::Committed(decode_disposition_view_value(payload(map, "CompletionView.committed")?, "CompletionView.committed")?)),
        "commit_failed" => Ok(CompletionView::CommitFailed(decode_disposition_view_value(payload(map, "CompletionView.commit_failed")?, "CompletionView.commit_failed")?)),
        "interrupted" => {
            unit_payload(map, "CompletionView.interrupted")?;
            Ok(CompletionView::Interrupted)
        }
        "internal" => {
            unit_payload(map, "CompletionView.internal")?;
            Ok(CompletionView::Internal)
        }
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_completion_view_value(value: &CompletionView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        CompletionView::Committed(payload) => {
            map.insert("kind".to_owned(), Value::from("committed"));
            map.insert("value".to_owned(), encode_disposition_view_value(payload)?);
        }
        CompletionView::CommitFailed(payload) => {
            map.insert("kind".to_owned(), Value::from("commit_failed"));
            map.insert("value".to_owned(), encode_disposition_view_value(payload)?);
        }
        CompletionView::Interrupted => {
            map.insert("kind".to_owned(), Value::from("interrupted"));
        }
        CompletionView::Internal => {
            map.insert("kind".to_owned(), Value::from("internal"));
        }
    }
    Ok(Value::Object(map))
}

fn decode_command_completion_view_value(value: &Value, context: &'static str) -> Result<CommandCompletionView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["command_id", "completion"], context)?;
    let command_id = hex_fixed(field(map, "command_id", "CommandCompletionView.command_id")?, 32, "CommandCompletionView.command_id")?;
    let completion = decode_completion_view_value(field(map, "completion", "CommandCompletionView.completion")?, "CommandCompletionView.completion")?;
    Ok(CommandCompletionView {
        command_id,
        completion,
    })
}

fn encode_command_completion_view_value(value: &CommandCompletionView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("command_id".to_owned(), encode_hex_fixed(&value.command_id, 32, "CommandCompletionView.command_id")?);
    map.insert("completion".to_owned(), encode_completion_view_value(&value.completion)?);
    Ok(Value::Object(map))
}

fn decode_create_refusal_view_value(value: &Value, context: &'static str) -> Result<CreateRefusalView, CommandError> {
    let text = value.as_str().ok_or(CommandError::Shape { context })?;
    match text {
        "invite_not_recognized" => Ok(CreateRefusalView::InviteNotRecognized),
        "invite_bare_room_code" => Ok(CreateRefusalView::InviteBareRoomCode),
        "invite_malformed" => Ok(CreateRefusalView::InviteMalformed),
        "invite_too_long" => Ok(CreateRefusalView::InviteTooLong),
        "invite_unsupported" => Ok(CreateRefusalView::InviteUnsupported),
        "invite_role_unsupported" => Ok(CreateRefusalView::InviteRoleUnsupported),
        "name_too_long" => Ok(CreateRefusalView::NameTooLong),
        "storage_fault" => Ok(CreateRefusalView::StorageFault),
        "internal" => Ok(CreateRefusalView::Internal),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_create_refusal_view_value(value: &CreateRefusalView) -> Value {
    Value::from(match value {
        CreateRefusalView::InviteNotRecognized => "invite_not_recognized",
        CreateRefusalView::InviteBareRoomCode => "invite_bare_room_code",
        CreateRefusalView::InviteMalformed => "invite_malformed",
        CreateRefusalView::InviteTooLong => "invite_too_long",
        CreateRefusalView::InviteUnsupported => "invite_unsupported",
        CreateRefusalView::InviteRoleUnsupported => "invite_role_unsupported",
        CreateRefusalView::NameTooLong => "name_too_long",
        CreateRefusalView::StorageFault => "storage_fault",
        CreateRefusalView::Internal => "internal",
    })
}

fn decode_card_created_view_value(value: &Value, context: &'static str) -> Result<CardCreatedView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["card"], context)?;
    let card = hex_fixed(field(map, "card", "CardCreatedView.card")?, 16, "CardCreatedView.card")?;
    Ok(CardCreatedView {
        card,
    })
}

fn encode_card_created_view_value(value: &CardCreatedView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "CardCreatedView.card")?);
    Ok(Value::Object(map))
}

fn decode_create_outcome_view_value(value: &Value, context: &'static str) -> Result<CreateOutcomeView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "created" => Ok(CreateOutcomeView::Created(decode_card_created_view_value(payload(map, "CreateOutcomeView.created")?, "CreateOutcomeView.created")?)),
        "refused" => Ok(CreateOutcomeView::Refused(decode_create_refusal_view_value(payload(map, "CreateOutcomeView.refused")?, "CreateOutcomeView.refused")?)),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_create_outcome_view_value(value: &CreateOutcomeView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        CreateOutcomeView::Created(payload) => {
            map.insert("kind".to_owned(), Value::from("created"));
            map.insert("value".to_owned(), encode_card_created_view_value(payload)?);
        }
        CreateOutcomeView::Refused(payload) => {
            map.insert("kind".to_owned(), Value::from("refused"));
            map.insert("value".to_owned(), encode_create_refusal_view_value(payload));
        }
    }
    Ok(Value::Object(map))
}

fn decode_create_result_view_value(value: &Value, context: &'static str) -> Result<CreateResultView, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["outcome", "request_id"], context)?;
    let outcome = decode_create_outcome_view_value(field(map, "outcome", "CreateResultView.outcome")?, "CreateResultView.outcome")?;
    let request_id = hex_fixed(field(map, "request_id", "CreateResultView.request_id")?, 32, "CreateResultView.request_id")?;
    Ok(CreateResultView {
        outcome,
        request_id,
    })
}

fn encode_create_result_view_value(value: &CreateResultView) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("outcome".to_owned(), encode_create_outcome_view_value(&value.outcome)?);
    map.insert("request_id".to_owned(), encode_hex_fixed(&value.request_id, 32, "CreateResultView.request_id")?);
    Ok(Value::Object(map))
}

fn decode_command_body_value(value: &Value, context: &'static str) -> Result<CommandBody, CommandError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CommandError::Shape { context })?;
    match kind {
        "intent" => Ok(CommandBody::Intent(decode_frontend_intent_view_value(payload(map, "CommandBody.intent")?, "CommandBody.intent")?)),
        "acceptance" => Ok(CommandBody::Acceptance(decode_command_acceptance_view_value(payload(map, "CommandBody.acceptance")?, "CommandBody.acceptance")?)),
        "completion" => Ok(CommandBody::Completion(decode_command_completion_view_value(payload(map, "CommandBody.completion")?, "CommandBody.completion")?)),
        "create_result" => Ok(CommandBody::CreateResult(decode_create_result_view_value(payload(map, "CommandBody.create_result")?, "CommandBody.create_result")?)),
        _ => Err(CommandError::UnknownVariant { context }),
    }
}

fn encode_command_body_value(value: &CommandBody) -> Result<Value, CommandError> {
    let mut map = Map::new();
    match value {
        CommandBody::Intent(payload) => {
            map.insert("kind".to_owned(), Value::from("intent"));
            map.insert("value".to_owned(), encode_frontend_intent_view_value(payload)?);
        }
        CommandBody::Acceptance(payload) => {
            map.insert("kind".to_owned(), Value::from("acceptance"));
            map.insert("value".to_owned(), encode_command_acceptance_view_value(payload)?);
        }
        CommandBody::Completion(payload) => {
            map.insert("kind".to_owned(), Value::from("completion"));
            map.insert("value".to_owned(), encode_command_completion_view_value(payload)?);
        }
        CommandBody::CreateResult(payload) => {
            map.insert("kind".to_owned(), Value::from("create_result"));
            map.insert("value".to_owned(), encode_create_result_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_command_frame_value(value: &Value, context: &'static str) -> Result<CommandFrame, CommandError> {
    let map = frame_object(value, context)?;
    match map.get("schema").and_then(Value::as_str) {
        Some(schema) if schema == COMMAND_SCHEMA_ID => {}
        Some(_) => return Err(CommandError::UnknownSchema),
        None => return Err(CommandError::Shape { context: "CommandFrame.schema" }),
    }
    known_keys(map, &["schema", "body"], context)?;
    let body = decode_command_body_value(field(map, "body", "CommandFrame.body")?, "CommandFrame.body")?;
    Ok(CommandFrame {
        body,
    })
}

fn encode_command_frame_value(value: &CommandFrame) -> Result<Value, CommandError> {
    let mut map = Map::new();
    map.insert("schema".to_owned(), Value::from(COMMAND_SCHEMA_ID));
    map.insert("body".to_owned(), encode_command_body_value(&value.body)?);
    Ok(Value::Object(map))
}

