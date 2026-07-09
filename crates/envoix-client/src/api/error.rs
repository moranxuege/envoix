//! Structured errors for the public API: what kind of failure, at which
//! phase of the transfer, with the details - so a UI can say "rendezvous
//! server unreachable" instead of "connection lost".

use std::fmt;

use envoix_error::CoreError;
use envoix_session::{TransferDirection, USER_INTERRUPT_MESSAGE};
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
        })
    }
}

/// Stable failure code for native UI, retry policy, and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    UserCanceled,
    PeerCanceled,
    NetworkLost,
    PeerUnreachable,
    AuthenticationFailed,
    PermissionDenied,
    DiskFull,
    HashMismatch,
    ProtocolError,
    DestinationConflict,
    UnsupportedFeature,
    Timeout,
    InternalError,
    Unknown,
}

/// Broad product category for grouping failure handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Protocol,
    Unsupported,
    Internal,
    Unknown,
}

/// Where the failure originated, when known.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureOrigin {
    Local,
    Peer,
    Unknown,
}

/// Product-facing phase with room for more detail than the internal phase cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Setup,
    Binding,
    Advertising,
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
    Acknowledging,
    CleaningUp,
}

/// Suggested next user action. The UI decides how to present it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    UpdateApp,
    SwitchPairingMethod,
    DiscardPartial,
    None,
}

/// Machine-readable transfer failure suitable for native clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TransferFailure {
    pub code: FailureCode,
    pub category: FailureCategory,
    pub phase: FailurePhase,
    pub origin: FailureOrigin,
    pub direction: Option<TransferDirection>,
    pub transfer_id: Option<String>,
    pub attempt_id: Option<String>,
    pub retryable: bool,
    pub recovery_action: RecoveryAction,
    pub user_message_key: String,
    pub diagnostic_message: String,
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

    /// A transport failure observed at `phase`.
    pub fn transport(phase: Phase, message: impl Into<String>) -> Self {
        Self {
            phase,
            kind: ErrorKind::Transport,
            message: message.into(),
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
            CoreError::Transfer(message) if message == "transfer interrupted by peer" => {
                (ErrorKind::Cancelled, message)
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

    /// Converts the error into a stable UI/diagnostic failure object.
    pub fn to_failure(&self, direction: Option<TransferDirection>) -> TransferFailure {
        let code = self.failure_code();
        TransferFailure {
            code,
            category: failure_category(code),
            phase: self.failure_phase(code),
            origin: self.failure_origin(code),
            direction,
            transfer_id: None,
            attempt_id: None,
            retryable: retryable(code),
            recovery_action: recovery_action(code, &self.message),
            user_message_key: user_message_key(code).to_string(),
            diagnostic_message: self.to_string(),
        }
    }

    fn failure_code(&self) -> FailureCode {
        let message = self.message.to_ascii_lowercase();
        match self.kind {
            ErrorKind::Cancelled if contains_any(&message, &["peer", "other device"]) => {
                FailureCode::PeerCanceled
            }
            ErrorKind::Cancelled => FailureCode::UserCanceled,
            ErrorKind::Input if contains_any(&message, &["not supported", "unsupported"]) => {
                FailureCode::UnsupportedFeature
            }
            ErrorKind::Input => FailureCode::Unknown,
            ErrorKind::Io | ErrorKind::Storage => classify_storage_message(&message),
            ErrorKind::Protocol => FailureCode::ProtocolError,
            ErrorKind::Transport => classify_network_message(&message),
            ErrorKind::Crypto if contains_any(&message, &["hash", "mismatch", "verify"]) => {
                FailureCode::HashMismatch
            }
            ErrorKind::Crypto => FailureCode::AuthenticationFailed,
            ErrorKind::Discovery => classify_discovery_message(&message),
            ErrorKind::Transfer => classify_transfer_message(&message),
        }
    }

    fn failure_phase(&self, code: FailureCode) -> FailurePhase {
        let message = self.message.to_ascii_lowercase();
        if code == FailureCode::HashMismatch || contains_any(&message, &["hash", "verify"]) {
            return FailurePhase::Verifying;
        }
        if contains_any(&message, &["confirm completion", "completeack", "ack"]) {
            return FailurePhase::Acknowledging;
        }
        if contains_any(&message, &["finalize", "commit", "rename"]) {
            return FailurePhase::Committing;
        }
        match self.phase {
            Phase::Setup => FailurePhase::Setup,
            Phase::Waiting => FailurePhase::Advertising,
            Phase::Pairing => FailurePhase::Pairing,
            Phase::Connecting => {
                if code == FailureCode::AuthenticationFailed {
                    FailurePhase::Authenticating
                } else {
                    FailurePhase::Connecting
                }
            }
            Phase::Transfer => FailurePhase::Transferring,
        }
    }

    fn failure_origin(&self, code: FailureCode) -> FailureOrigin {
        let message = self.message.to_ascii_lowercase();
        match code {
            FailureCode::PeerCanceled => FailureOrigin::Peer,
            FailureCode::UserCanceled
            | FailureCode::PermissionDenied
            | FailureCode::DiskFull
            | FailureCode::DestinationConflict => FailureOrigin::Local,
            _ if contains_any(&message, &["peer reported", "by peer", "closed by peer"]) => {
                FailureOrigin::Peer
            }
            _ => FailureOrigin::Unknown,
        }
    }
}

fn classify_storage_message(message: &str) -> FailureCode {
    if contains_any(
        message,
        &["permission", "denied", "operation not permitted"],
    ) {
        FailureCode::PermissionDenied
    } else if contains_any(message, &["no space", "disk full", "quota"]) {
        FailureCode::DiskFull
    } else if contains_any(message, &["already exists", "conflict", "destination"]) {
        FailureCode::DestinationConflict
    } else {
        FailureCode::InternalError
    }
}

fn classify_network_message(message: &str) -> FailureCode {
    if contains_any(message, &["timed out", "timeout", "deadline"]) {
        FailureCode::Timeout
    } else if contains_any(
        message,
        &["unreachable", "no route", "not found", "no peer"],
    ) {
        FailureCode::PeerUnreachable
    } else {
        FailureCode::NetworkLost
    }
}

fn classify_discovery_message(message: &str) -> FailureCode {
    if contains_any(message, &["timed out", "timeout", "within"]) {
        FailureCode::Timeout
    } else {
        FailureCode::PeerUnreachable
    }
}

fn classify_transfer_message(message: &str) -> FailureCode {
    if contains_any(message, &["interrupted by peer"]) {
        FailureCode::PeerCanceled
    } else if contains_any(message, &["interrupted by user"]) {
        FailureCode::UserCanceled
    } else if contains_any(
        message,
        &["timed out", "timeout", "deadline", "confirm completion"],
    ) {
        FailureCode::Timeout
    } else if contains_any(message, &["hash", "mismatch", "verification failed"]) {
        FailureCode::HashMismatch
    } else if contains_any(message, &["unsupported", "not supported"]) {
        FailureCode::UnsupportedFeature
    } else if contains_any(message, &["closed by peer", "connection lost", "reset"]) {
        FailureCode::NetworkLost
    } else if contains_any(message, &["peer reported"]) {
        FailureCode::ProtocolError
    } else {
        FailureCode::Unknown
    }
}

fn failure_category(code: FailureCode) -> FailureCategory {
    match code {
        FailureCode::UserCanceled | FailureCode::PeerCanceled => FailureCategory::User,
        FailureCode::NetworkLost | FailureCode::PeerUnreachable | FailureCode::Timeout => {
            FailureCategory::Network
        }
        FailureCode::AuthenticationFailed => FailureCategory::Authentication,
        FailureCode::PermissionDenied => FailureCategory::Permission,
        FailureCode::DiskFull | FailureCode::DestinationConflict => FailureCategory::Storage,
        FailureCode::HashMismatch => FailureCategory::Integrity,
        FailureCode::ProtocolError => FailureCategory::Protocol,
        FailureCode::UnsupportedFeature => FailureCategory::Unsupported,
        FailureCode::InternalError => FailureCategory::Internal,
        FailureCode::Unknown => FailureCategory::Unknown,
    }
}

fn retryable(code: FailureCode) -> bool {
    matches!(
        code,
        FailureCode::NetworkLost
            | FailureCode::PeerUnreachable
            | FailureCode::Timeout
            | FailureCode::PermissionDenied
            | FailureCode::DiskFull
            | FailureCode::DestinationConflict
    )
}

fn recovery_action(code: FailureCode, message: &str) -> RecoveryAction {
    let message = message.to_ascii_lowercase();
    match code {
        FailureCode::NetworkLost | FailureCode::PeerUnreachable | FailureCode::Timeout => {
            RecoveryAction::Retry
        }
        FailureCode::PermissionDenied if contains_any(&message, &["network", "local network"]) => {
            RecoveryAction::OpenSettings
        }
        FailureCode::PermissionDenied | FailureCode::DestinationConflict => {
            RecoveryAction::ChooseFolder
        }
        FailureCode::DiskFull => RecoveryAction::ChooseFolder,
        FailureCode::AuthenticationFailed => RecoveryAction::RePair,
        FailureCode::UnsupportedFeature => RecoveryAction::UpdateApp,
        _ => RecoveryAction::None,
    }
}

fn user_message_key(code: FailureCode) -> &'static str {
    match code {
        FailureCode::UserCanceled => "transfer.user_canceled",
        FailureCode::PeerCanceled => "transfer.peer_canceled",
        FailureCode::NetworkLost => "transfer.network_lost",
        FailureCode::PeerUnreachable => "transfer.peer_unreachable",
        FailureCode::AuthenticationFailed => "transfer.authentication_failed",
        FailureCode::PermissionDenied => "transfer.permission_denied",
        FailureCode::DiskFull => "transfer.disk_full",
        FailureCode::HashMismatch => "transfer.hash_mismatch",
        FailureCode::ProtocolError => "transfer.protocol_error",
        FailureCode::DestinationConflict => "transfer.destination_conflict",
        FailureCode::UnsupportedFeature => "transfer.unsupported_feature",
        FailureCode::Timeout => "transfer.timeout",
        FailureCode::InternalError => "transfer.internal_error",
        FailureCode::Unknown => "transfer.unknown",
    }
}

fn contains_any(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| message.contains(needle))
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

    #[test]
    fn exposes_retryable_timeout_failure() {
        let error = TransferError::from_core(
            CoreError::Transfer(
                "receiver did not confirm completion within 60 seconds; retry may resume the transfer"
                    .into(),
            ),
            Phase::Transfer,
        );
        let failure = error.to_failure(Some(TransferDirection::Send));
        assert_eq!(failure.code, FailureCode::Timeout);
        assert_eq!(failure.category, FailureCategory::Network);
        assert_eq!(failure.phase, FailurePhase::Acknowledging);
        assert_eq!(failure.direction, Some(TransferDirection::Send));
        assert!(failure.retryable);
        assert_eq!(failure.recovery_action, RecoveryAction::Retry);
    }

    #[test]
    fn exposes_peer_cancellation_failure() {
        let error = TransferError::from_core(
            CoreError::Transfer("transfer interrupted by peer".into()),
            Phase::Transfer,
        );
        let failure = error.to_failure(Some(TransferDirection::Receive));
        assert_eq!(failure.code, FailureCode::PeerCanceled);
        assert_eq!(failure.category, FailureCategory::User);
        assert_eq!(failure.origin, FailureOrigin::Peer);
        assert!(!failure.retryable);
    }

    #[test]
    fn exposes_permission_recovery_action() {
        let error = TransferError::from_core(
            CoreError::Storage("permission denied opening destination folder".into()),
            Phase::Transfer,
        );
        let failure = error.to_failure(Some(TransferDirection::Receive));
        assert_eq!(failure.code, FailureCode::PermissionDenied);
        assert_eq!(failure.category, FailureCategory::Permission);
        assert_eq!(failure.origin, FailureOrigin::Local);
        assert!(failure.retryable);
        assert_eq!(failure.recovery_action, RecoveryAction::ChooseFolder);
    }
}
