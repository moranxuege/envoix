use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::code::{RoomCode, looks_like_bare_room_code};
use crate::identifiers::{
    DEEP_LINK_OUTER_PREFIX, INVITE_PAYLOAD_VERSION, QR_OUTER_PREFIX, URI_SCHEME,
};
use crate::{InviteError, InviteField, RecognizedInvalid};

pub const MAX_INVITE_INPUT_LENGTH: usize = 8 * 1024;
pub const MAX_ENCODED_PAYLOAD_LENGTH: usize = 6 * 1024;
pub const MAX_DECODED_PAYLOAD_LENGTH: usize = 4 * 1024;
pub const MAX_BROKER_LENGTH: usize = 1024;
pub const MAX_RELAY_LENGTH: usize = 2048;

const DEEP_LINK_VERSION_PATH: &str = "invite/v";
const LEGACY_PAIR_PATH: &str = "pair/";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Send,
    Receive,
}

impl Role {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Send => Self::Receive,
            Self::Receive => Self::Send,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Invite {
    code: RoomCode,
    broker: String,
    relay: String,
    role: Role,
}

impl Invite {
    pub fn new(
        code: impl Into<String>,
        broker: impl Into<String>,
        relay: impl Into<String>,
        role: Role,
    ) -> Result<Self, InviteError> {
        let code = RoomCode::parse(code)?;
        let broker =
            validate_endpoint_field(broker.into(), MAX_BROKER_LENGTH, InviteField::Broker)?;
        let relay = validate_endpoint_field(relay.into(), MAX_RELAY_LENGTH, InviteField::Relay)?;
        Ok(Self {
            code,
            broker,
            relay,
            role,
        })
    }

    pub const fn code(&self) -> &RoomCode {
        &self.code
    }

    pub fn broker(&self) -> &str {
        &self.broker
    }

    pub fn relay(&self) -> &str {
        &self.relay
    }

    pub const fn role(&self) -> Role {
        self.role
    }
}

impl fmt::Debug for Invite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Invite")
            .field("code", &self.code)
            .field("broker", &self.broker)
            .field("relay", &self.relay)
            .field("role", &self.role)
            .finish()
    }
}

#[derive(Deserialize, Serialize)]
struct InvitePayload {
    version: u32,
    code: String,
    broker: String,
    relay: String,
    role: Role,
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

pub fn encode_qr(invite: &Invite) -> Result<String, InviteError> {
    Ok(format!("{QR_OUTER_PREFIX}{}", encode_payload(invite)?))
}

pub fn encode_deep_link(invite: &Invite) -> Result<String, InviteError> {
    Ok(format!(
        "{DEEP_LINK_OUTER_PREFIX}{DEEP_LINK_VERSION_PATH}{INVITE_PAYLOAD_VERSION}/{}",
        encode_payload(invite)?
    ))
}

pub fn route_invite(input: &str) -> Result<Invite, InviteError> {
    if input.len() > MAX_INVITE_INPUT_LENGTH {
        return Err(InviteError::InputTooLong {
            actual: input.len(),
            maximum: MAX_INVITE_INPUT_LENGTH,
        });
    }
    let input = input.trim();

    if let Some(rest) = input.strip_prefix(DEEP_LINK_OUTER_PREFIX) {
        return route_deep_link(rest);
    }
    if let Some(encoded) = input.strip_prefix(QR_OUTER_PREFIX) {
        return decode_payload(encoded);
    }
    if starts_with_envoix_scheme_case_insensitive(input) {
        return Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::NonCanonicalOuterForm,
        ));
    }
    if looks_like_bare_room_code(input) {
        return Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::BareRoomCode,
        ));
    }
    Err(InviteError::NotEnvoixInvite)
}

fn route_deep_link(rest: &str) -> Result<Invite, InviteError> {
    if rest.starts_with(LEGACY_PAIR_PATH) {
        return Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::LegacyPairDeepLink,
        ));
    }
    if let Some(versioned_payload) = rest.strip_prefix(DEEP_LINK_VERSION_PATH)
        && let Some((version, encoded)) = versioned_payload.split_once('/')
        && let Ok(found) = version.parse::<u32>()
    {
        if found != INVITE_PAYLOAD_VERSION {
            return Err(InviteError::RecognizedInvalid(
                RecognizedInvalid::UnsupportedPayloadVersion {
                    found,
                    expected: INVITE_PAYLOAD_VERSION,
                },
            ));
        }
        if version != INVITE_PAYLOAD_VERSION.to_string() {
            return Err(InviteError::RecognizedInvalid(
                RecognizedInvalid::NonCanonicalOuterForm,
            ));
        }
        return decode_payload(encoded);
    }
    Err(InviteError::RecognizedInvalid(
        RecognizedInvalid::UnsupportedEnvoixDialect,
    ))
}

fn encode_payload(invite: &Invite) -> Result<String, InviteError> {
    let payload = InvitePayload {
        version: INVITE_PAYLOAD_VERSION,
        code: invite.code.as_str().to_owned(),
        broker: invite.broker.clone(),
        relay: invite.relay.clone(),
        role: invite.role,
    };
    let json = serde_json::to_vec(&payload).map_err(|_| InviteError::EncodingFailed)?;
    if json.len() > MAX_DECODED_PAYLOAD_LENGTH {
        return Err(InviteError::DecodedPayloadTooLong {
            actual: json.len(),
            maximum: MAX_DECODED_PAYLOAD_LENGTH,
        });
    }
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_payload(encoded: &str) -> Result<Invite, InviteError> {
    if encoded.len() > MAX_ENCODED_PAYLOAD_LENGTH {
        return Err(InviteError::EncodedPayloadTooLong {
            actual: encoded.len(),
            maximum: MAX_ENCODED_PAYLOAD_LENGTH,
        });
    }
    let json = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| InviteError::MalformedBase64)?;
    if json.len() > MAX_DECODED_PAYLOAD_LENGTH {
        return Err(InviteError::DecodedPayloadTooLong {
            actual: json.len(),
            maximum: MAX_DECODED_PAYLOAD_LENGTH,
        });
    }
    let version: VersionProbe =
        serde_json::from_slice(&json).map_err(|_| InviteError::MalformedPayload)?;
    if version.version != INVITE_PAYLOAD_VERSION {
        return Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::UnsupportedPayloadVersion {
                found: version.version,
                expected: INVITE_PAYLOAD_VERSION,
            },
        ));
    }
    let payload: InvitePayload =
        serde_json::from_slice(&json).map_err(|_| InviteError::MalformedPayload)?;
    Invite::new(payload.code, payload.broker, payload.relay, payload.role)
}

fn validate_endpoint_field(
    value: String,
    maximum: usize,
    field: InviteField,
) -> Result<String, InviteError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(InviteError::InvalidField(field))
    } else {
        Ok(value)
    }
}

fn starts_with_envoix_scheme_case_insensitive(input: &str) -> bool {
    let marker = format!("{URI_SCHEME}:");
    input
        .get(..marker.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&marker))
}
