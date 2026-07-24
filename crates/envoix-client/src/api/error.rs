//! Setup/input error for the lightweight application facade.

use std::fmt;

/// An error produced before a canonical Manifest v2 session starts.
///
/// Runtime transfer failures are emitted by the typed core/FFI cause model;
/// this facade error is intentionally limited to invalid local configuration
/// and pairing input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferError {
    message: String,
}

impl TransferError {
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransferError {}
