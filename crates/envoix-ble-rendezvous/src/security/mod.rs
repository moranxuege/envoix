//! BLE rendezvous security: ephemeral key agreement, SAS confirmation,
//! transcript binding, and session key derivation.
//!
//! ## Modules
//!
//! - [`mode`] — Versioned security-mode enum (`Insecure`, `AuthenticatedV1`)
//! - [`sas`] — 6-digit Short Authentication String computation and display
//! - [`transcript`] — Length-prefixed authenticated transcript builder
//! - [`key_exchange`] — Ephemeral X25519 key pair generation and DH agreement
//! - [`authenticator`] — Full state machine driving the SAS-verified handshake

pub mod authenticator;
pub mod key_exchange;
pub mod mode;
pub mod sas;
pub mod transcript;

/// Role labels for confirmation MAC domain separation.
pub(crate) const INITIATOR_CONFIRM_LABEL: &[u8] = b"initiator-confirm";
pub(crate) const RESPONDER_CONFIRM_LABEL: &[u8] = b"responder-confirm";

/// Errors from the BLE rendezvous security layer.
#[derive(Debug, thiserror::Error)]
pub enum BleError {
    #[error("entropy source unavailable")]
    Entropy,

    #[error("invalid peer public key (low-order point or identity element)")]
    InvalidPublicKey,

    #[error("SAS confirmation MAC mismatch — tampered handshake or wrong peer")]
    SasConfirmMismatch,

    #[error("unknown or unsupported security mode byte: {0}")]
    UnknownSecurityMode(u8),

    #[error("peer rejected the SAS comparison")]
    SasRejected,

    #[error("timeout waiting for handshake message")]
    Timeout,

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("protocol error: {0}")]
    Protocol(String),
}

impl From<BleError> for envoix_error::CoreError {
    fn from(error: BleError) -> Self {
        match error {
            BleError::Entropy => envoix_error::CoreError::Crypto(error.to_string()),
            BleError::InvalidPublicKey => envoix_error::CoreError::Crypto(error.to_string()),
            BleError::SasConfirmMismatch => envoix_error::CoreError::Crypto(error.to_string()),
            BleError::UnknownSecurityMode(m) => {
                envoix_error::CoreError::Protocol(format!("unknown BLE security mode: {m}"))
            }
            BleError::SasRejected => envoix_error::CoreError::Protocol(error.to_string()),
            BleError::Timeout => envoix_error::CoreError::Transport(error.to_string()),
            BleError::Crypto(msg) => envoix_error::CoreError::Crypto(msg),
            BleError::Protocol(msg) => envoix_error::CoreError::Protocol(msg),
        }
    }
}
