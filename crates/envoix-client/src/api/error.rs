//! Structured errors for the public API: what kind of failure, at which
//! phase of the transfer, with the details - so a UI can say "rendezvous
//! server unreachable" instead of "connection lost".

use std::fmt;

use envoix_error::CoreError;
use envoix_session::{USER_INTERRUPT_MESSAGE, USER_PAUSE_MESSAGE};
use serde::Serialize;

/// Where in a transfer's life an error occurred, derived from the last
/// lifecycle event the transfer emitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Validating input and binding local endpoints.
    Setup,
    /// Listening/advertising, waiting for the peer to dial.
    Waiting,
    /// Rendezvous pairing through the broker.
    Pairing,
    /// Establishing and authenticating the peer connection.
    Connecting,
    /// Moving or verifying file data.
    Transfer,
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Setup => "setup",
            Self::Waiting => "waiting for peer",
            Self::Pairing => "pairing",
            Self::Connecting => "connecting",
            Self::Transfer => "transfer",
        })
    }
}

/// The kind of failure, mirroring the internal error taxonomy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Invalid caller input.
    Input,
    /// Local I/O failure.
    Io,
    /// Peer spoke the protocol wrong.
    Protocol,
    /// Network/transport failure.
    Transport,
    /// Cryptographic failure (pairing, verification).
    Crypto,
    /// Local storage failure.
    Storage,
    /// Peer discovery failure.
    Discovery,
    /// Transfer-level failure (includes errors reported by the peer).
    Transfer,
    /// The user cancelled the transfer.
    Cancelled,
    /// The user paused the transfer (resumable intent, not a failure).
    Paused,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Input => "invalid input",
            Self::Io => "I/O error",
            Self::Protocol => "protocol error",
            Self::Transport => "transport error",
            Self::Crypto => "crypto error",
            Self::Storage => "storage error",
            Self::Discovery => "discovery error",
            Self::Transfer => "transfer error",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
        })
    }
}

/// A transfer failure with enough structure for UIs and retry policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TransferError {
    /// Where in the transfer's life the failure happened.
    pub phase: Phase,
    /// What kind of failure it was.
    pub kind: ErrorKind,
    /// Human-readable details.
    pub message: String,
}

impl TransferError {
    /// An invalid-input failure (always during setup).
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            phase: Phase::Setup,
            kind: ErrorKind::Input,
            message: message.into(),
        }
    }

    /// A user cancellation observed at `phase`.
    pub fn cancelled(phase: Phase) -> Self {
        Self {
            phase,
            kind: ErrorKind::Cancelled,
            message: "interrupted before completion".into(),
        }
    }

    /// Classifies an internal error, attaching the phase the transfer had
    /// reached when it surfaced.
    pub(crate) fn from_core(error: CoreError, phase: Phase) -> Self {
        let (kind, message) = match error {
            CoreError::InvalidInput(message) => (ErrorKind::Input, message),
            CoreError::Io(message) => (ErrorKind::Io, message),
            CoreError::Protocol(message) => (ErrorKind::Protocol, message),
            CoreError::Transport(message) => (ErrorKind::Transport, message),
            CoreError::Crypto(message) => (ErrorKind::Crypto, message),
            CoreError::Storage(message) => (ErrorKind::Storage, message),
            CoreError::Discovery(message) => (ErrorKind::Discovery, message),
            CoreError::Transfer(message) if message == USER_INTERRUPT_MESSAGE => {
                (ErrorKind::Cancelled, message)
            }
            CoreError::Transfer(message) if message == USER_PAUSE_MESSAGE => {
                (ErrorKind::Paused, message)
            }
            CoreError::Transfer(message) => (ErrorKind::Transfer, message),
            CoreError::Cancelled => (ErrorKind::Cancelled, "operation cancelled".into()),
        };
        Self {
            phase,
            kind,
            message,
        }
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.kind, self.phase, self.message
        )
    }
}

impl std::error::Error for TransferError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_kind_and_keeps_phase() {
        let error = TransferError::from_core(
            CoreError::Transport("connection lost".into()),
            Phase::Pairing,
        );
        assert_eq!(error.kind, ErrorKind::Transport);
        assert_eq!(error.phase, Phase::Pairing);
        assert_eq!(
            error.to_string(),
            "transport error during pairing: connection lost"
        );
    }

    #[test]
    fn maps_user_interrupt_to_cancelled() {
        let error = TransferError::from_core(
            CoreError::Transfer(USER_INTERRUPT_MESSAGE.into()),
            Phase::Transfer,
        );
        assert_eq!(error.kind, ErrorKind::Cancelled);
    }
}
