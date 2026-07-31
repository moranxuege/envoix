// @generated from schema/duty.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.

use serde_json::{Map, Value};

pub const DUTY_SCHEMA_ID: &str = "envoix/binding/duty/4";
pub const DUTY_MAX_FRAME_BYTES: usize = 131072;

const U63_MAX: u64 = 9_223_372_036_854_775_807;

/// Typed codec failure. It carries only static schema context, never a
/// fragment of the (possibly hostile) input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DutyError {
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
pub enum NoticeView {
    TransferComplete,
    TransferFailed,
    ActionNeeded,
}

impl NoticeView {
    pub const ALL: [Self; 3] = [
        Self::TransferComplete,
        Self::TransferFailed,
        Self::ActionNeeded,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockDirectiveView {
    Hold,
    Release,
}

impl LockDirectiveView {
    pub const ALL: [Self; 2] = [
        Self::Hold,
        Self::Release,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyProvenanceView {
    pub card: String,
    pub generation: u32,
    pub request: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationWorkView {
    pub staged: String,
    pub display_name: String,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForegroundWorkView {
    pub active_transfers: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationWorkView {
    pub notice: NoticeView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockWorkView {
    pub directive: LockDirectiveView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkView {
    SourceHandle,
    Grant,
    Staging,
    Publication(PublicationWorkView),
    Courier,
    Foreground(ForegroundWorkView),
    Notification(NotificationWorkView),
    Lock(LockWorkView),
    OpenShare,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyOrderView {
    pub provenance: DutyProvenanceView,
    pub work: WorkView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceRetentionView {
    Process,
    Persisted,
}

impl SourceRetentionView {
    pub const ALL: [Self; 2] = [
        Self::Process,
        Self::Persisted,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceSeekabilityView {
    Seekable,
    SequentialOnly,
}

impl SourceSeekabilityView {
    pub const ALL: [Self; 2] = [
        Self::Seekable,
        Self::SequentialOnly,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcquiredItemView {
    pub item: u32,
    pub retention: SourceRetentionView,
    pub seekability: SourceSeekabilityView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAcquiredView {
    pub items: Vec<AcquiredItemView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFailureView {
    Unreadable,
    PermissionLost,
    StorageFault,
    Internal,
}

impl SourceFailureView {
    pub const ALL: [Self; 4] = [
        Self::Unreadable,
        Self::PermissionLost,
        Self::StorageFault,
        Self::Internal,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFailedView {
    pub reason: SourceFailureView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceReportView {
    Acquired(SourceAcquiredView),
    Failed(SourceFailedView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DutyAnswerView {
    Outcome(OutcomeCodeView),
    Source(SourceReportView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyReportView {
    pub provenance: DutyProvenanceView,
    pub answer: DutyAnswerView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DutyBody {
    Order(DutyOrderView),
    Report(DutyReportView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutyFrame {
    pub body: DutyBody,
}

/// Decodes and validates one frame. Every failure is a typed
/// [`DutyError`]; no input, however hostile, panics or misparses.
pub fn decode_duty_frame(bytes: &[u8]) -> Result<DutyFrame, DutyError> {
    if bytes.len() > DUTY_MAX_FRAME_BYTES {
        return Err(DutyError::FrameTooLarge);
    }
    let value = strict_json(bytes)?;
    decode_duty_frame_value(&value, "DutyFrame")
}

/// Encodes one frame, stamping the schema envelope and enforcing the
/// same bounds the decoder checks.
pub fn encode_duty_frame(frame: &DutyFrame) -> Result<Vec<u8>, DutyError> {
    let value = encode_duty_frame_value(frame)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| DutyError::MalformedJson)?;
    if bytes.len() > DUTY_MAX_FRAME_BYTES {
        return Err(DutyError::FrameTooLarge);
    }
    Ok(bytes)
}

/// Parses JSON while rejecting duplicate object keys at any depth and
/// trailing input. A duplicated key is the smuggling shape: a first-wins
/// upstream parser would see a different value than a last-wins one applies.
fn strict_json(bytes: &[u8]) -> Result<Value, DutyError> {
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
        .map_err(|_| DutyError::MalformedJson)?;
    deserializer.end().map_err(|_| DutyError::MalformedJson)?;
    Ok(value)
}

fn frame_object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, DutyError> {
    value.as_object().ok_or(DutyError::Shape { context })
}

fn known_keys(map: &Map<String, Value>, allowed: &[&str], context: &'static str) -> Result<(), DutyError> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(DutyError::UnknownField { context });
        }
    }
    Ok(())
}

fn field<'a>(map: &'a Map<String, Value>, key: &str, context: &'static str) -> Result<&'a Value, DutyError> {
    map.get(key).ok_or(DutyError::Shape { context })
}

fn integer(value: &Value, max: u64, context: &'static str) -> Result<u64, DutyError> {
    let number = value.as_u64().ok_or(DutyError::Shape { context })?;
    if number > max {
        return Err(DutyError::Range { context });
    }
    Ok(number)
}

fn encode_u63(number: u64, context: &'static str) -> Result<Value, DutyError> {
    if number > U63_MAX {
        return Err(DutyError::Range { context });
    }
    Ok(Value::from(number))
}

fn integer_u32(value: &Value, context: &'static str) -> Result<u32, DutyError> {
    let number = integer(value, 4_294_967_295, context)?;
    u32::try_from(number).map_err(|_| DutyError::Range { context })
}

fn hex_chars(text: &str) -> bool {
    text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_fixed(value: &Value, chars: usize, context: &'static str) -> Result<String, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    encode_hex_fixed(text, chars, context)?;
    Ok(text.to_owned())
}

fn encode_hex_fixed(text: &str, chars: usize, context: &'static str) -> Result<Value, DutyError> {
    if text.len() != chars || !hex_chars(text) {
        return Err(DutyError::Bound { context });
    }
    Ok(Value::from(text))
}

fn utf8_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    encode_utf8_bounded(text, max_bytes, context)?;
    Ok(text.to_owned())
}

fn encode_utf8_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, DutyError> {
    if text.len() > max_bytes {
        return Err(DutyError::Bound { context });
    }
    Ok(Value::from(text))
}

fn payload<'a>(map: &'a Map<String, Value>, context: &'static str) -> Result<&'a Value, DutyError> {
    match map.get("value") {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(DutyError::Shape { context }),
    }
}

fn unit_payload(map: &Map<String, Value>, context: &'static str) -> Result<(), DutyError> {
    match map.get("value") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(DutyError::Shape { context }),
    }
}

fn decode_outcome_code_view_value(value: &Value, context: &'static str) -> Result<OutcomeCodeView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
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
        _ => Err(DutyError::UnknownVariant { context }),
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

fn decode_notice_view_value(value: &Value, context: &'static str) -> Result<NoticeView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    match text {
        "transfer_complete" => Ok(NoticeView::TransferComplete),
        "transfer_failed" => Ok(NoticeView::TransferFailed),
        "action_needed" => Ok(NoticeView::ActionNeeded),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_notice_view_value(value: &NoticeView) -> Value {
    Value::from(match value {
        NoticeView::TransferComplete => "transfer_complete",
        NoticeView::TransferFailed => "transfer_failed",
        NoticeView::ActionNeeded => "action_needed",
    })
}

fn decode_lock_directive_view_value(value: &Value, context: &'static str) -> Result<LockDirectiveView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    match text {
        "hold" => Ok(LockDirectiveView::Hold),
        "release" => Ok(LockDirectiveView::Release),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_lock_directive_view_value(value: &LockDirectiveView) -> Value {
    Value::from(match value {
        LockDirectiveView::Hold => "hold",
        LockDirectiveView::Release => "release",
    })
}

fn decode_duty_provenance_view_value(value: &Value, context: &'static str) -> Result<DutyProvenanceView, DutyError> {
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

fn encode_duty_provenance_view_value(value: &DutyProvenanceView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "DutyProvenanceView.card")?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    map.insert("request".to_owned(), encode_hex_fixed(&value.request, 32, "DutyProvenanceView.request")?);
    Ok(Value::Object(map))
}

fn decode_publication_work_view_value(value: &Value, context: &'static str) -> Result<PublicationWorkView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["staged", "display_name", "total_bytes"], context)?;
    let staged = utf8_bounded(field(map, "staged", "PublicationWorkView.staged")?, 512, "PublicationWorkView.staged")?;
    let display_name = utf8_bounded(field(map, "display_name", "PublicationWorkView.display_name")?, 255, "PublicationWorkView.display_name")?;
    let total_bytes = integer(field(map, "total_bytes", "PublicationWorkView.total_bytes")?, U63_MAX, "PublicationWorkView.total_bytes")?;
    Ok(PublicationWorkView {
        staged,
        display_name,
        total_bytes,
    })
}

fn encode_publication_work_view_value(value: &PublicationWorkView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("staged".to_owned(), encode_utf8_bounded(&value.staged, 512, "PublicationWorkView.staged")?);
    map.insert("display_name".to_owned(), encode_utf8_bounded(&value.display_name, 255, "PublicationWorkView.display_name")?);
    map.insert("total_bytes".to_owned(), encode_u63(value.total_bytes, "PublicationWorkView.total_bytes")?);
    Ok(Value::Object(map))
}

fn decode_foreground_work_view_value(value: &Value, context: &'static str) -> Result<ForegroundWorkView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["active_transfers"], context)?;
    let active_transfers = integer_u32(field(map, "active_transfers", "ForegroundWorkView.active_transfers")?, "ForegroundWorkView.active_transfers")?;
    Ok(ForegroundWorkView {
        active_transfers,
    })
}

fn encode_foreground_work_view_value(value: &ForegroundWorkView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("active_transfers".to_owned(), Value::from(value.active_transfers));
    Ok(Value::Object(map))
}

fn decode_notification_work_view_value(value: &Value, context: &'static str) -> Result<NotificationWorkView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["notice"], context)?;
    let notice = decode_notice_view_value(field(map, "notice", "NotificationWorkView.notice")?, "NotificationWorkView.notice")?;
    Ok(NotificationWorkView {
        notice,
    })
}

fn encode_notification_work_view_value(value: &NotificationWorkView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("notice".to_owned(), encode_notice_view_value(&value.notice));
    Ok(Value::Object(map))
}

fn decode_lock_work_view_value(value: &Value, context: &'static str) -> Result<LockWorkView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["directive"], context)?;
    let directive = decode_lock_directive_view_value(field(map, "directive", "LockWorkView.directive")?, "LockWorkView.directive")?;
    Ok(LockWorkView {
        directive,
    })
}

fn encode_lock_work_view_value(value: &LockWorkView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("directive".to_owned(), encode_lock_directive_view_value(&value.directive));
    Ok(Value::Object(map))
}

fn decode_work_view_value(value: &Value, context: &'static str) -> Result<WorkView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(DutyError::Shape { context })?;
    match kind {
        "source_handle" => {
            unit_payload(map, "WorkView.source_handle")?;
            Ok(WorkView::SourceHandle)
        }
        "grant" => {
            unit_payload(map, "WorkView.grant")?;
            Ok(WorkView::Grant)
        }
        "staging" => {
            unit_payload(map, "WorkView.staging")?;
            Ok(WorkView::Staging)
        }
        "publication" => Ok(WorkView::Publication(decode_publication_work_view_value(payload(map, "WorkView.publication")?, "WorkView.publication")?)),
        "courier" => {
            unit_payload(map, "WorkView.courier")?;
            Ok(WorkView::Courier)
        }
        "foreground" => Ok(WorkView::Foreground(decode_foreground_work_view_value(payload(map, "WorkView.foreground")?, "WorkView.foreground")?)),
        "notification" => Ok(WorkView::Notification(decode_notification_work_view_value(payload(map, "WorkView.notification")?, "WorkView.notification")?)),
        "lock" => Ok(WorkView::Lock(decode_lock_work_view_value(payload(map, "WorkView.lock")?, "WorkView.lock")?)),
        "open_share" => {
            unit_payload(map, "WorkView.open_share")?;
            Ok(WorkView::OpenShare)
        }
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_work_view_value(value: &WorkView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    match value {
        WorkView::SourceHandle => {
            map.insert("kind".to_owned(), Value::from("source_handle"));
        }
        WorkView::Grant => {
            map.insert("kind".to_owned(), Value::from("grant"));
        }
        WorkView::Staging => {
            map.insert("kind".to_owned(), Value::from("staging"));
        }
        WorkView::Publication(payload) => {
            map.insert("kind".to_owned(), Value::from("publication"));
            map.insert("value".to_owned(), encode_publication_work_view_value(payload)?);
        }
        WorkView::Courier => {
            map.insert("kind".to_owned(), Value::from("courier"));
        }
        WorkView::Foreground(payload) => {
            map.insert("kind".to_owned(), Value::from("foreground"));
            map.insert("value".to_owned(), encode_foreground_work_view_value(payload)?);
        }
        WorkView::Notification(payload) => {
            map.insert("kind".to_owned(), Value::from("notification"));
            map.insert("value".to_owned(), encode_notification_work_view_value(payload)?);
        }
        WorkView::Lock(payload) => {
            map.insert("kind".to_owned(), Value::from("lock"));
            map.insert("value".to_owned(), encode_lock_work_view_value(payload)?);
        }
        WorkView::OpenShare => {
            map.insert("kind".to_owned(), Value::from("open_share"));
        }
    }
    Ok(Value::Object(map))
}

fn decode_duty_order_view_value(value: &Value, context: &'static str) -> Result<DutyOrderView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["provenance", "work"], context)?;
    let provenance = decode_duty_provenance_view_value(field(map, "provenance", "DutyOrderView.provenance")?, "DutyOrderView.provenance")?;
    let work = decode_work_view_value(field(map, "work", "DutyOrderView.work")?, "DutyOrderView.work")?;
    Ok(DutyOrderView {
        provenance,
        work,
    })
}

fn encode_duty_order_view_value(value: &DutyOrderView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("provenance".to_owned(), encode_duty_provenance_view_value(&value.provenance)?);
    map.insert("work".to_owned(), encode_work_view_value(&value.work)?);
    Ok(Value::Object(map))
}

fn decode_source_retention_view_value(value: &Value, context: &'static str) -> Result<SourceRetentionView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    match text {
        "process" => Ok(SourceRetentionView::Process),
        "persisted" => Ok(SourceRetentionView::Persisted),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_source_retention_view_value(value: &SourceRetentionView) -> Value {
    Value::from(match value {
        SourceRetentionView::Process => "process",
        SourceRetentionView::Persisted => "persisted",
    })
}

fn decode_source_seekability_view_value(value: &Value, context: &'static str) -> Result<SourceSeekabilityView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    match text {
        "seekable" => Ok(SourceSeekabilityView::Seekable),
        "sequential_only" => Ok(SourceSeekabilityView::SequentialOnly),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_source_seekability_view_value(value: &SourceSeekabilityView) -> Value {
    Value::from(match value {
        SourceSeekabilityView::Seekable => "seekable",
        SourceSeekabilityView::SequentialOnly => "sequential_only",
    })
}

fn decode_acquired_item_view_value(value: &Value, context: &'static str) -> Result<AcquiredItemView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["item", "retention", "seekability"], context)?;
    let item = integer_u32(field(map, "item", "AcquiredItemView.item")?, "AcquiredItemView.item")?;
    let retention = decode_source_retention_view_value(field(map, "retention", "AcquiredItemView.retention")?, "AcquiredItemView.retention")?;
    let seekability = decode_source_seekability_view_value(field(map, "seekability", "AcquiredItemView.seekability")?, "AcquiredItemView.seekability")?;
    Ok(AcquiredItemView {
        item,
        retention,
        seekability,
    })
}

fn encode_acquired_item_view_value(value: &AcquiredItemView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("item".to_owned(), Value::from(value.item));
    map.insert("retention".to_owned(), encode_source_retention_view_value(&value.retention));
    map.insert("seekability".to_owned(), encode_source_seekability_view_value(&value.seekability));
    Ok(Value::Object(map))
}

fn decode_source_acquired_view_value(value: &Value, context: &'static str) -> Result<SourceAcquiredView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["items"], context)?;
    let items = {
        let items = field(map, "items", "SourceAcquiredView.items")?.as_array().ok_or(DutyError::Shape { context: "SourceAcquiredView.items" })?;
        if items.len() > 1024 {
            return Err(DutyError::Bound { context: "SourceAcquiredView.items" });
        }
        let mut collected = Vec::with_capacity(items.len());
        for item in items {
            collected.push(decode_acquired_item_view_value(item, "SourceAcquiredView.items")?);
        }
        collected
    };
    Ok(SourceAcquiredView {
        items,
    })
}

fn encode_source_acquired_view_value(value: &SourceAcquiredView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("items".to_owned(), {
        if value.items.len() > 1024 {
            return Err(DutyError::Bound { context: "SourceAcquiredView.items" });
        }
        let mut items = Vec::with_capacity(value.items.len());
        for item in &value.items {
            items.push(encode_acquired_item_view_value(item)?);
        }
        Value::Array(items)
    });
    Ok(Value::Object(map))
}

fn decode_source_failure_view_value(value: &Value, context: &'static str) -> Result<SourceFailureView, DutyError> {
    let text = value.as_str().ok_or(DutyError::Shape { context })?;
    match text {
        "unreadable" => Ok(SourceFailureView::Unreadable),
        "permission_lost" => Ok(SourceFailureView::PermissionLost),
        "storage_fault" => Ok(SourceFailureView::StorageFault),
        "internal" => Ok(SourceFailureView::Internal),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_source_failure_view_value(value: &SourceFailureView) -> Value {
    Value::from(match value {
        SourceFailureView::Unreadable => "unreadable",
        SourceFailureView::PermissionLost => "permission_lost",
        SourceFailureView::StorageFault => "storage_fault",
        SourceFailureView::Internal => "internal",
    })
}

fn decode_source_failed_view_value(value: &Value, context: &'static str) -> Result<SourceFailedView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["reason"], context)?;
    let reason = decode_source_failure_view_value(field(map, "reason", "SourceFailedView.reason")?, "SourceFailedView.reason")?;
    Ok(SourceFailedView {
        reason,
    })
}

fn encode_source_failed_view_value(value: &SourceFailedView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("reason".to_owned(), encode_source_failure_view_value(&value.reason));
    Ok(Value::Object(map))
}

fn decode_source_report_view_value(value: &Value, context: &'static str) -> Result<SourceReportView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(DutyError::Shape { context })?;
    match kind {
        "acquired" => Ok(SourceReportView::Acquired(decode_source_acquired_view_value(payload(map, "SourceReportView.acquired")?, "SourceReportView.acquired")?)),
        "failed" => Ok(SourceReportView::Failed(decode_source_failed_view_value(payload(map, "SourceReportView.failed")?, "SourceReportView.failed")?)),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_source_report_view_value(value: &SourceReportView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    match value {
        SourceReportView::Acquired(payload) => {
            map.insert("kind".to_owned(), Value::from("acquired"));
            map.insert("value".to_owned(), encode_source_acquired_view_value(payload)?);
        }
        SourceReportView::Failed(payload) => {
            map.insert("kind".to_owned(), Value::from("failed"));
            map.insert("value".to_owned(), encode_source_failed_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_duty_answer_view_value(value: &Value, context: &'static str) -> Result<DutyAnswerView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(DutyError::Shape { context })?;
    match kind {
        "outcome" => Ok(DutyAnswerView::Outcome(decode_outcome_code_view_value(payload(map, "DutyAnswerView.outcome")?, "DutyAnswerView.outcome")?)),
        "source" => Ok(DutyAnswerView::Source(decode_source_report_view_value(payload(map, "DutyAnswerView.source")?, "DutyAnswerView.source")?)),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_duty_answer_view_value(value: &DutyAnswerView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    match value {
        DutyAnswerView::Outcome(payload) => {
            map.insert("kind".to_owned(), Value::from("outcome"));
            map.insert("value".to_owned(), encode_outcome_code_view_value(payload));
        }
        DutyAnswerView::Source(payload) => {
            map.insert("kind".to_owned(), Value::from("source"));
            map.insert("value".to_owned(), encode_source_report_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_duty_report_view_value(value: &Value, context: &'static str) -> Result<DutyReportView, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["provenance", "answer"], context)?;
    let provenance = decode_duty_provenance_view_value(field(map, "provenance", "DutyReportView.provenance")?, "DutyReportView.provenance")?;
    let answer = decode_duty_answer_view_value(field(map, "answer", "DutyReportView.answer")?, "DutyReportView.answer")?;
    Ok(DutyReportView {
        provenance,
        answer,
    })
}

fn encode_duty_report_view_value(value: &DutyReportView) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("provenance".to_owned(), encode_duty_provenance_view_value(&value.provenance)?);
    map.insert("answer".to_owned(), encode_duty_answer_view_value(&value.answer)?);
    Ok(Value::Object(map))
}

fn decode_duty_body_value(value: &Value, context: &'static str) -> Result<DutyBody, DutyError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(DutyError::Shape { context })?;
    match kind {
        "order" => Ok(DutyBody::Order(decode_duty_order_view_value(payload(map, "DutyBody.order")?, "DutyBody.order")?)),
        "report" => Ok(DutyBody::Report(decode_duty_report_view_value(payload(map, "DutyBody.report")?, "DutyBody.report")?)),
        _ => Err(DutyError::UnknownVariant { context }),
    }
}

fn encode_duty_body_value(value: &DutyBody) -> Result<Value, DutyError> {
    let mut map = Map::new();
    match value {
        DutyBody::Order(payload) => {
            map.insert("kind".to_owned(), Value::from("order"));
            map.insert("value".to_owned(), encode_duty_order_view_value(payload)?);
        }
        DutyBody::Report(payload) => {
            map.insert("kind".to_owned(), Value::from("report"));
            map.insert("value".to_owned(), encode_duty_report_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_duty_frame_value(value: &Value, context: &'static str) -> Result<DutyFrame, DutyError> {
    let map = frame_object(value, context)?;
    match map.get("schema").and_then(Value::as_str) {
        Some(schema) if schema == DUTY_SCHEMA_ID => {}
        Some(_) => return Err(DutyError::UnknownSchema),
        None => return Err(DutyError::Shape { context: "DutyFrame.schema" }),
    }
    known_keys(map, &["schema", "body"], context)?;
    let body = decode_duty_body_value(field(map, "body", "DutyFrame.body")?, "DutyFrame.body")?;
    Ok(DutyFrame {
        body,
    })
}

fn encode_duty_frame_value(value: &DutyFrame) -> Result<Value, DutyError> {
    let mut map = Map::new();
    map.insert("schema".to_owned(), Value::from(DUTY_SCHEMA_ID));
    map.insert("body".to_owned(), encode_duty_body_value(&value.body)?);
    Ok(Value::Object(map))
}

