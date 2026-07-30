// @generated from schema/capability.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.

use serde_json::{Map, Value};

use envoix_types::Secret;

pub const CAPABILITY_SCHEMA_ID: &str = "envoix/binding/capability/2";
pub const CAPABILITY_MAX_FRAME_BYTES: usize = 65536;

const U63_MAX: u64 = 9_223_372_036_854_775_807;

/// Typed codec failure. It carries only static schema context, never a
/// fragment of the (possibly hostile) input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    FrameTooLarge,
    MalformedJson,
    UnknownSchema,
    Shape { context: &'static str },
    UnknownField { context: &'static str },
    UnknownVariant { context: &'static str },
    Range { context: &'static str },
    Bound { context: &'static str },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedTextView {
    pub text: Secret<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclinedView {
    Cancelled,
    Refused,
    Unsupported,
}

impl DeclinedView {
    pub const ALL: [Self; 3] = [
        Self::Cancelled,
        Self::Refused,
        Self::Unsupported,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclinedReasonView {
    pub reason: DeclinedView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanInviteStepView {
    Requested,
    Provided(ScannedTextView),
    Declined(DeclinedReasonView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanInviteExchangeView {
    pub step: ScanInviteStepView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAcquisitionKeyView {
    pub card: String,
    pub generation: u32,
    pub request: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickedSourceView {
    pub display_name: String,
    pub reported_size: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickSourceFailureView {
    PickerUnavailable,
    MetadataUnavailable,
    Internal,
}

impl PickSourceFailureView {
    pub const ALL: [Self; 3] = [
        Self::PickerUnavailable,
        Self::MetadataUnavailable,
        Self::Internal,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickSourceFailureReasonView {
    pub reason: PickSourceFailureView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PickSourceStepView {
    Requested,
    Provided(PickedSourceView),
    Declined(DeclinedReasonView),
    Failed(PickSourceFailureReasonView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickSourceExchangeView {
    pub acquisition: SourceAcquisitionKeyView,
    pub step: PickSourceStepView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityExchangeView {
    ScanInvite(ScanInviteExchangeView),
    PickSource(PickSourceExchangeView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityBody {
    Exchange(CapabilityExchangeView),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityFrame {
    pub body: CapabilityBody,
}

/// Decodes and validates one frame. Every failure is a typed
/// [`CapabilityError`]; no input, however hostile, panics or misparses.
pub fn decode_capability_frame(bytes: &[u8]) -> Result<CapabilityFrame, CapabilityError> {
    if bytes.len() > CAPABILITY_MAX_FRAME_BYTES {
        return Err(CapabilityError::FrameTooLarge);
    }
    let value = strict_json(bytes)?;
    decode_capability_frame_value(&value, "CapabilityFrame")
}

/// Encodes one frame, stamping the schema envelope and enforcing the
/// same bounds the decoder checks.
pub fn encode_capability_frame(frame: &CapabilityFrame) -> Result<Vec<u8>, CapabilityError> {
    let value = encode_capability_frame_value(frame)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| CapabilityError::MalformedJson)?;
    if bytes.len() > CAPABILITY_MAX_FRAME_BYTES {
        return Err(CapabilityError::FrameTooLarge);
    }
    Ok(bytes)
}

/// Parses JSON while rejecting duplicate object keys at any depth and
/// trailing input. A duplicated key is the smuggling shape: a first-wins
/// upstream parser would see a different value than a last-wins one applies.
fn strict_json(bytes: &[u8]) -> Result<Value, CapabilityError> {
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
        .map_err(|_| CapabilityError::MalformedJson)?;
    deserializer.end().map_err(|_| CapabilityError::MalformedJson)?;
    Ok(value)
}

fn frame_object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, CapabilityError> {
    value.as_object().ok_or(CapabilityError::Shape { context })
}

fn known_keys(map: &Map<String, Value>, allowed: &[&str], context: &'static str) -> Result<(), CapabilityError> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(CapabilityError::UnknownField { context });
        }
    }
    Ok(())
}

fn field<'a>(map: &'a Map<String, Value>, key: &str, context: &'static str) -> Result<&'a Value, CapabilityError> {
    map.get(key).ok_or(CapabilityError::Shape { context })
}

fn integer(value: &Value, max: u64, context: &'static str) -> Result<u64, CapabilityError> {
    let number = value.as_u64().ok_or(CapabilityError::Shape { context })?;
    if number > max {
        return Err(CapabilityError::Range { context });
    }
    Ok(number)
}

fn encode_u63(number: u64, context: &'static str) -> Result<Value, CapabilityError> {
    if number > U63_MAX {
        return Err(CapabilityError::Range { context });
    }
    Ok(Value::from(number))
}

fn integer_u32(value: &Value, context: &'static str) -> Result<u32, CapabilityError> {
    let number = integer(value, 4_294_967_295, context)?;
    u32::try_from(number).map_err(|_| CapabilityError::Range { context })
}

fn hex_chars(text: &str) -> bool {
    text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_fixed(value: &Value, chars: usize, context: &'static str) -> Result<String, CapabilityError> {
    let text = value.as_str().ok_or(CapabilityError::Shape { context })?;
    encode_hex_fixed(text, chars, context)?;
    Ok(text.to_owned())
}

fn encode_hex_fixed(text: &str, chars: usize, context: &'static str) -> Result<Value, CapabilityError> {
    if text.len() != chars || !hex_chars(text) {
        return Err(CapabilityError::Bound { context });
    }
    Ok(Value::from(text))
}

fn utf8_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, CapabilityError> {
    let text = value.as_str().ok_or(CapabilityError::Shape { context })?;
    encode_utf8_bounded(text, max_bytes, context)?;
    Ok(text.to_owned())
}

fn encode_utf8_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, CapabilityError> {
    if text.len() > max_bytes {
        return Err(CapabilityError::Bound { context });
    }
    Ok(Value::from(text))
}

fn payload<'a>(map: &'a Map<String, Value>, context: &'static str) -> Result<&'a Value, CapabilityError> {
    match map.get("value") {
        Some(value) if !value.is_null() => Ok(value),
        _ => Err(CapabilityError::Shape { context }),
    }
}

fn unit_payload(map: &Map<String, Value>, context: &'static str) -> Result<(), CapabilityError> {
    match map.get("value") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(CapabilityError::Shape { context }),
    }
}

fn decode_scanned_text_view_value(value: &Value, context: &'static str) -> Result<ScannedTextView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["text"], context)?;
    let text = Secret::new(utf8_bounded(field(map, "text", "ScannedTextView.text")?, 16384, "ScannedTextView.text")?);
    Ok(ScannedTextView {
        text,
    })
}

fn encode_scanned_text_view_value(value: &ScannedTextView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("text".to_owned(), encode_utf8_bounded(value.text.expose(), 16384, "ScannedTextView.text")?);
    Ok(Value::Object(map))
}

fn decode_declined_view_value(value: &Value, context: &'static str) -> Result<DeclinedView, CapabilityError> {
    let text = value.as_str().ok_or(CapabilityError::Shape { context })?;
    match text {
        "cancelled" => Ok(DeclinedView::Cancelled),
        "refused" => Ok(DeclinedView::Refused),
        "unsupported" => Ok(DeclinedView::Unsupported),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_declined_view_value(value: &DeclinedView) -> Value {
    Value::from(match value {
        DeclinedView::Cancelled => "cancelled",
        DeclinedView::Refused => "refused",
        DeclinedView::Unsupported => "unsupported",
    })
}

fn decode_declined_reason_view_value(value: &Value, context: &'static str) -> Result<DeclinedReasonView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["reason"], context)?;
    let reason = decode_declined_view_value(field(map, "reason", "DeclinedReasonView.reason")?, "DeclinedReasonView.reason")?;
    Ok(DeclinedReasonView {
        reason,
    })
}

fn encode_declined_reason_view_value(value: &DeclinedReasonView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("reason".to_owned(), encode_declined_view_value(&value.reason));
    Ok(Value::Object(map))
}

fn decode_scan_invite_step_view_value(value: &Value, context: &'static str) -> Result<ScanInviteStepView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CapabilityError::Shape { context })?;
    match kind {
        "requested" => {
            unit_payload(map, "ScanInviteStepView.requested")?;
            Ok(ScanInviteStepView::Requested)
        }
        "provided" => Ok(ScanInviteStepView::Provided(decode_scanned_text_view_value(payload(map, "ScanInviteStepView.provided")?, "ScanInviteStepView.provided")?)),
        "declined" => Ok(ScanInviteStepView::Declined(decode_declined_reason_view_value(payload(map, "ScanInviteStepView.declined")?, "ScanInviteStepView.declined")?)),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_scan_invite_step_view_value(value: &ScanInviteStepView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    match value {
        ScanInviteStepView::Requested => {
            map.insert("kind".to_owned(), Value::from("requested"));
        }
        ScanInviteStepView::Provided(payload) => {
            map.insert("kind".to_owned(), Value::from("provided"));
            map.insert("value".to_owned(), encode_scanned_text_view_value(payload)?);
        }
        ScanInviteStepView::Declined(payload) => {
            map.insert("kind".to_owned(), Value::from("declined"));
            map.insert("value".to_owned(), encode_declined_reason_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_scan_invite_exchange_view_value(value: &Value, context: &'static str) -> Result<ScanInviteExchangeView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["step"], context)?;
    let step = decode_scan_invite_step_view_value(field(map, "step", "ScanInviteExchangeView.step")?, "ScanInviteExchangeView.step")?;
    Ok(ScanInviteExchangeView {
        step,
    })
}

fn encode_scan_invite_exchange_view_value(value: &ScanInviteExchangeView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("step".to_owned(), encode_scan_invite_step_view_value(&value.step)?);
    Ok(Value::Object(map))
}

fn decode_source_acquisition_key_view_value(value: &Value, context: &'static str) -> Result<SourceAcquisitionKeyView, CapabilityError> {
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

fn encode_source_acquisition_key_view_value(value: &SourceAcquisitionKeyView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("card".to_owned(), encode_hex_fixed(&value.card, 16, "SourceAcquisitionKeyView.card")?);
    map.insert("generation".to_owned(), Value::from(value.generation));
    map.insert("request".to_owned(), encode_hex_fixed(&value.request, 32, "SourceAcquisitionKeyView.request")?);
    Ok(Value::Object(map))
}

fn decode_picked_source_view_value(value: &Value, context: &'static str) -> Result<PickedSourceView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["display_name", "reported_size"], context)?;
    let display_name = utf8_bounded(field(map, "display_name", "PickedSourceView.display_name")?, 1020, "PickedSourceView.display_name")?;
    let reported_size = match field(map, "reported_size", "PickedSourceView.reported_size")? {
        Value::Null => None,
        present => Some(integer(present, U63_MAX, "PickedSourceView.reported_size")?),
    };
    Ok(PickedSourceView {
        display_name,
        reported_size,
    })
}

fn encode_picked_source_view_value(value: &PickedSourceView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("display_name".to_owned(), encode_utf8_bounded(&value.display_name, 1020, "PickedSourceView.display_name")?);
    map.insert(
        "reported_size".to_owned(),
        match &value.reported_size {
            None => Value::Null,
            Some(inner) => encode_u63(*inner, "PickedSourceView.reported_size")?,
        },
    );
    Ok(Value::Object(map))
}

fn decode_pick_source_failure_view_value(value: &Value, context: &'static str) -> Result<PickSourceFailureView, CapabilityError> {
    let text = value.as_str().ok_or(CapabilityError::Shape { context })?;
    match text {
        "picker_unavailable" => Ok(PickSourceFailureView::PickerUnavailable),
        "metadata_unavailable" => Ok(PickSourceFailureView::MetadataUnavailable),
        "internal" => Ok(PickSourceFailureView::Internal),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_pick_source_failure_view_value(value: &PickSourceFailureView) -> Value {
    Value::from(match value {
        PickSourceFailureView::PickerUnavailable => "picker_unavailable",
        PickSourceFailureView::MetadataUnavailable => "metadata_unavailable",
        PickSourceFailureView::Internal => "internal",
    })
}

fn decode_pick_source_failure_reason_view_value(value: &Value, context: &'static str) -> Result<PickSourceFailureReasonView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["reason"], context)?;
    let reason = decode_pick_source_failure_view_value(field(map, "reason", "PickSourceFailureReasonView.reason")?, "PickSourceFailureReasonView.reason")?;
    Ok(PickSourceFailureReasonView {
        reason,
    })
}

fn encode_pick_source_failure_reason_view_value(value: &PickSourceFailureReasonView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("reason".to_owned(), encode_pick_source_failure_view_value(&value.reason));
    Ok(Value::Object(map))
}

fn decode_pick_source_step_view_value(value: &Value, context: &'static str) -> Result<PickSourceStepView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CapabilityError::Shape { context })?;
    match kind {
        "requested" => {
            unit_payload(map, "PickSourceStepView.requested")?;
            Ok(PickSourceStepView::Requested)
        }
        "provided" => Ok(PickSourceStepView::Provided(decode_picked_source_view_value(payload(map, "PickSourceStepView.provided")?, "PickSourceStepView.provided")?)),
        "declined" => Ok(PickSourceStepView::Declined(decode_declined_reason_view_value(payload(map, "PickSourceStepView.declined")?, "PickSourceStepView.declined")?)),
        "failed" => Ok(PickSourceStepView::Failed(decode_pick_source_failure_reason_view_value(payload(map, "PickSourceStepView.failed")?, "PickSourceStepView.failed")?)),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_pick_source_step_view_value(value: &PickSourceStepView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    match value {
        PickSourceStepView::Requested => {
            map.insert("kind".to_owned(), Value::from("requested"));
        }
        PickSourceStepView::Provided(payload) => {
            map.insert("kind".to_owned(), Value::from("provided"));
            map.insert("value".to_owned(), encode_picked_source_view_value(payload)?);
        }
        PickSourceStepView::Declined(payload) => {
            map.insert("kind".to_owned(), Value::from("declined"));
            map.insert("value".to_owned(), encode_declined_reason_view_value(payload)?);
        }
        PickSourceStepView::Failed(payload) => {
            map.insert("kind".to_owned(), Value::from("failed"));
            map.insert("value".to_owned(), encode_pick_source_failure_reason_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_pick_source_exchange_view_value(value: &Value, context: &'static str) -> Result<PickSourceExchangeView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["acquisition", "step"], context)?;
    let acquisition = decode_source_acquisition_key_view_value(field(map, "acquisition", "PickSourceExchangeView.acquisition")?, "PickSourceExchangeView.acquisition")?;
    let step = decode_pick_source_step_view_value(field(map, "step", "PickSourceExchangeView.step")?, "PickSourceExchangeView.step")?;
    Ok(PickSourceExchangeView {
        acquisition,
        step,
    })
}

fn encode_pick_source_exchange_view_value(value: &PickSourceExchangeView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("acquisition".to_owned(), encode_source_acquisition_key_view_value(&value.acquisition)?);
    map.insert("step".to_owned(), encode_pick_source_step_view_value(&value.step)?);
    Ok(Value::Object(map))
}

fn decode_capability_exchange_view_value(value: &Value, context: &'static str) -> Result<CapabilityExchangeView, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CapabilityError::Shape { context })?;
    match kind {
        "scan_invite" => Ok(CapabilityExchangeView::ScanInvite(decode_scan_invite_exchange_view_value(payload(map, "CapabilityExchangeView.scan_invite")?, "CapabilityExchangeView.scan_invite")?)),
        "pick_source" => Ok(CapabilityExchangeView::PickSource(decode_pick_source_exchange_view_value(payload(map, "CapabilityExchangeView.pick_source")?, "CapabilityExchangeView.pick_source")?)),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_capability_exchange_view_value(value: &CapabilityExchangeView) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    match value {
        CapabilityExchangeView::ScanInvite(payload) => {
            map.insert("kind".to_owned(), Value::from("scan_invite"));
            map.insert("value".to_owned(), encode_scan_invite_exchange_view_value(payload)?);
        }
        CapabilityExchangeView::PickSource(payload) => {
            map.insert("kind".to_owned(), Value::from("pick_source"));
            map.insert("value".to_owned(), encode_pick_source_exchange_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_capability_body_value(value: &Value, context: &'static str) -> Result<CapabilityBody, CapabilityError> {
    let map = frame_object(value, context)?;
    known_keys(map, &["kind", "value"], context)?;
    let kind = field(map, "kind", context)?
        .as_str()
        .ok_or(CapabilityError::Shape { context })?;
    match kind {
        "exchange" => Ok(CapabilityBody::Exchange(decode_capability_exchange_view_value(payload(map, "CapabilityBody.exchange")?, "CapabilityBody.exchange")?)),
        _ => Err(CapabilityError::UnknownVariant { context }),
    }
}

fn encode_capability_body_value(value: &CapabilityBody) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    match value {
        CapabilityBody::Exchange(payload) => {
            map.insert("kind".to_owned(), Value::from("exchange"));
            map.insert("value".to_owned(), encode_capability_exchange_view_value(payload)?);
        }
    }
    Ok(Value::Object(map))
}

fn decode_capability_frame_value(value: &Value, context: &'static str) -> Result<CapabilityFrame, CapabilityError> {
    let map = frame_object(value, context)?;
    match map.get("schema").and_then(Value::as_str) {
        Some(schema) if schema == CAPABILITY_SCHEMA_ID => {}
        Some(_) => return Err(CapabilityError::UnknownSchema),
        None => return Err(CapabilityError::Shape { context: "CapabilityFrame.schema" }),
    }
    known_keys(map, &["schema", "body"], context)?;
    let body = decode_capability_body_value(field(map, "body", "CapabilityFrame.body")?, "CapabilityFrame.body")?;
    Ok(CapabilityFrame {
        body,
    })
}

fn encode_capability_frame_value(value: &CapabilityFrame) -> Result<Value, CapabilityError> {
    let mut map = Map::new();
    map.insert("schema".to_owned(), Value::from(CAPABILITY_SCHEMA_ID));
    map.insert("body".to_owned(), encode_capability_body_value(&value.body)?);
    Ok(Value::Object(map))
}

