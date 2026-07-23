use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InviteField {
    Code,
    Broker,
    Relay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognizedInvalid {
    UnsupportedPayloadVersion { found: u32, expected: u32 },
    LegacyPairDeepLink,
    UnsupportedEnvoixDialect,
    BareRoomCode,
    NonCanonicalOuterForm,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InviteError {
    InputTooLong { actual: usize, maximum: usize },
    EncodedPayloadTooLong { actual: usize, maximum: usize },
    DecodedPayloadTooLong { actual: usize, maximum: usize },
    MalformedBase64,
    MalformedPayload,
    InvalidField(InviteField),
    RecognizedInvalid(RecognizedInvalid),
    NotEnvoixInvite,
    EntropyUnavailable,
    UnusableEntropy,
    EncodingFailed,
}

impl fmt::Display for InviteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLong { actual, maximum } => {
                write!(
                    formatter,
                    "invite input length {actual} exceeds maximum {maximum}"
                )
            }
            Self::EncodedPayloadTooLong { actual, maximum } => write!(
                formatter,
                "encoded invite payload length {actual} exceeds maximum {maximum}"
            ),
            Self::DecodedPayloadTooLong { actual, maximum } => write!(
                formatter,
                "decoded invite payload length {actual} exceeds maximum {maximum}"
            ),
            Self::MalformedBase64 => formatter.write_str("invite payload is not valid base64url"),
            Self::MalformedPayload => formatter.write_str("invite payload is malformed"),
            Self::InvalidField(field) => write!(formatter, "invite field {field:?} is invalid"),
            Self::RecognizedInvalid(reason) => {
                write!(formatter, "recognized but unsupported invite: {reason}")
            }
            Self::NotEnvoixInvite => formatter.write_str("input is not an Envoix invite"),
            Self::EntropyUnavailable => formatter.write_str("entropy source unavailable"),
            Self::UnusableEntropy => formatter.write_str("entropy source repeatedly rejected"),
            Self::EncodingFailed => formatter.write_str("invite payload encoding failed"),
        }
    }
}

impl fmt::Display for RecognizedInvalid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPayloadVersion { found, expected } => write!(
                formatter,
                "payload version {found} is unsupported; expected {expected}"
            ),
            Self::LegacyPairDeepLink => formatter.write_str("legacy pair deep link"),
            Self::UnsupportedEnvoixDialect => formatter.write_str("unknown Envoix invite dialect"),
            Self::BareRoomCode => formatter.write_str("bare room code"),
            Self::NonCanonicalOuterForm => formatter.write_str("non-canonical Envoix outer form"),
        }
    }
}

impl std::error::Error for InviteError {}
