use std::fmt;

use envoix_outcomes::OutcomeCode;

use crate::message::MessageKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingError {
    InvalidCodeLength {
        actual: usize,
        maximum: usize,
    },
    EntropyUnavailable,
    TruncatedMessageHeader {
        actual: usize,
    },
    UnknownMessageType {
        wire_id: u8,
    },
    MessageTooLarge {
        declared: usize,
        maximum: usize,
    },
    TruncatedMessage {
        declared: usize,
        actual: usize,
    },
    TrailingMessageBytes {
        count: usize,
    },
    InvalidMessageLength {
        kind: MessageKind,
        actual: usize,
    },
    UnexpectedMessage {
        expected: MessageKind,
        actual: MessageKind,
    },
    SpakeRejected,
    ConfirmationFailed,
    DescriptorTooLarge {
        actual: usize,
        maximum: usize,
    },
    InvalidDescriptor,
    NonceExhausted,
    AuthenticationFailed,
    DataTokenMismatch,
}

impl PairingError {
    pub const fn outcome_code(&self) -> Option<OutcomeCode> {
        match self {
            Self::SpakeRejected
            | Self::ConfirmationFailed
            | Self::AuthenticationFailed
            | Self::DataTokenMismatch => Some(OutcomeCode::Unauthenticated),
            _ => None,
        }
    }
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCodeLength { actual, maximum } => write!(
                formatter,
                "pairing code length {actual} must be between 1 and {maximum} bytes"
            ),
            Self::EntropyUnavailable => formatter.write_str("entropy source unavailable"),
            Self::TruncatedMessageHeader { actual } => {
                write!(formatter, "pairing message header has only {actual} bytes")
            }
            Self::UnknownMessageType { wire_id } => {
                write!(formatter, "unknown pairing message type {wire_id}")
            }
            Self::MessageTooLarge { declared, maximum } => write!(
                formatter,
                "pairing message length {declared} exceeds maximum {maximum}"
            ),
            Self::TruncatedMessage { declared, actual } => write!(
                formatter,
                "pairing message declares {declared} bytes but only {actual} are present"
            ),
            Self::TrailingMessageBytes { count } => {
                write!(formatter, "pairing message has {count} trailing bytes")
            }
            Self::InvalidMessageLength { kind, actual } => {
                write!(
                    formatter,
                    "pairing message {kind:?} has invalid length {actual}"
                )
            }
            Self::UnexpectedMessage { expected, actual } => write!(
                formatter,
                "expected pairing message {expected:?}, received {actual:?}"
            ),
            Self::SpakeRejected => formatter.write_str("SPAKE2 message rejected"),
            Self::ConfirmationFailed => formatter.write_str("pairing confirmation failed"),
            Self::DescriptorTooLarge { actual, maximum } => write!(
                formatter,
                "peer descriptor length {actual} exceeds maximum {maximum}"
            ),
            Self::InvalidDescriptor => formatter.write_str("peer descriptor is malformed"),
            Self::NonceExhausted => formatter.write_str("sealed-descriptor nonce space exhausted"),
            Self::AuthenticationFailed => {
                formatter.write_str("sealed descriptor authentication failed")
            }
            Self::DataTokenMismatch => {
                formatter.write_str("sealed descriptor data token did not match")
            }
        }
    }
}

impl std::error::Error for PairingError {}
