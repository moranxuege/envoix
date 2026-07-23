use std::fmt;

use envoix_outcomes::OutcomeCode;

use crate::{AuthMessageKind, PeerRole};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthField {
    Nonce,
    SpakeMessage,
    Confirmation,
}

impl fmt::Display for AuthField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nonce => formatter.write_str("nonce"),
            Self::SpakeMessage => formatter.write_str("SPAKE2 message"),
            Self::Confirmation => formatter.write_str("confirmation"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthCodecError {
    TruncatedHeader {
        actual: usize,
    },
    WrongMagic,
    UnsupportedVersion {
        actual: u16,
    },
    WrongWireId {
        actual: u8,
    },
    NonZeroReservedByte,
    PayloadTooLarge {
        declared: usize,
        maximum: usize,
    },
    TruncatedFrame {
        declared: usize,
        actual: usize,
    },
    TrailingFrameBytes {
        count: usize,
    },
    UnknownMessageKind {
        wire_id: u8,
    },
    InvalidRole {
        wire_id: u8,
    },
    InvalidFieldLength {
        field: AuthField,
        actual: usize,
        expected: usize,
    },
    TruncatedPayload {
        needed: usize,
        remaining: usize,
    },
    TrailingPayloadBytes {
        count: usize,
    },
}

impl fmt::Display for AuthCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { actual } => {
                write!(formatter, "auth frame header has {actual} bytes")
            }
            Self::WrongMagic => formatter.write_str("auth frame magic is invalid"),
            Self::UnsupportedVersion { actual } => {
                write!(formatter, "auth frame version {actual} is unsupported")
            }
            Self::WrongWireId { actual } => {
                write!(formatter, "frame wire id {actual} is not authentication")
            }
            Self::NonZeroReservedByte => {
                formatter.write_str("auth frame reserved byte is non-zero")
            }
            Self::PayloadTooLarge { declared, maximum } => write!(
                formatter,
                "auth payload length {declared} exceeds maximum {maximum}"
            ),
            Self::TruncatedFrame { declared, actual } => write!(
                formatter,
                "auth frame declares {declared} payload bytes but has {actual}"
            ),
            Self::TrailingFrameBytes { count } => {
                write!(formatter, "auth frame has {count} trailing bytes")
            }
            Self::UnknownMessageKind { wire_id } => {
                write!(formatter, "auth message kind {wire_id} is unknown")
            }
            Self::InvalidRole { wire_id } => {
                write!(formatter, "auth role {wire_id} is invalid")
            }
            Self::InvalidFieldLength {
                field,
                actual,
                expected,
            } => write!(
                formatter,
                "{field} length {actual} does not equal {expected}"
            ),
            Self::TruncatedPayload { needed, remaining } => write!(
                formatter,
                "auth payload needs {needed} bytes but has {remaining}"
            ),
            Self::TrailingPayloadBytes { count } => {
                write!(formatter, "auth payload has {count} trailing bytes")
            }
        }
    }
}

impl std::error::Error for AuthCodecError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthError {
    Codec(AuthCodecError),
    EntropyUnavailable,
    SpakeRejected,
    ConfirmationFailed,
    UnexpectedMessage {
        expected: AuthMessageKind,
        actual: AuthMessageKind,
    },
    InvalidStartRole {
        actual: PeerRole,
    },
    Timeout,
    Cancelled,
    PeerClosed,
}

impl AuthError {
    pub const fn outcome_code(&self) -> OutcomeCode {
        match self {
            Self::Timeout => OutcomeCode::Timeout,
            Self::Cancelled => OutcomeCode::Cancelled,
            Self::PeerClosed => OutcomeCode::PeerLost,
            Self::EntropyUnavailable => OutcomeCode::Internal,
            Self::Codec(_)
            | Self::SpakeRejected
            | Self::ConfirmationFailed
            | Self::UnexpectedMessage { .. }
            | Self::InvalidStartRole { .. } => OutcomeCode::Unauthenticated,
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => error.fmt(formatter),
            Self::EntropyUnavailable => formatter.write_str("authentication entropy unavailable"),
            Self::SpakeRejected => formatter.write_str("SPAKE2 authentication rejected"),
            Self::ConfirmationFailed => formatter.write_str("authentication confirmation failed"),
            Self::UnexpectedMessage { expected, actual } => write!(
                formatter,
                "expected {expected} authentication message, received {actual}"
            ),
            Self::InvalidStartRole { actual } => {
                write!(
                    formatter,
                    "authentication start role must be sender, got {actual}"
                )
            }
            Self::Timeout => formatter.write_str("authentication deadline exceeded"),
            Self::Cancelled => formatter.write_str("authentication cancelled"),
            Self::PeerClosed => formatter.write_str("authentication peer closed"),
        }
    }
}

impl std::error::Error for AuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            _ => None,
        }
    }
}

impl From<AuthCodecError> for AuthError {
    fn from(error: AuthCodecError) -> Self {
        Self::Codec(error)
    }
}
