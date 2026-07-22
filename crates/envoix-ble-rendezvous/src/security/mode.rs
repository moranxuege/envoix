use serde::{Deserialize, Serialize};

/// Versioned BLE rendezvous security mode.
///
/// Mode `0` is the existing experimental insecure carrier (no authentication).
/// Mode `1` is the authenticated ephemeral key agreement with SAS confirmation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[repr(u8)]
pub enum BleRendezvousSecurity {
    /// Insecure/unauthenticated — experimental only, not for production.
    Insecure = 0,
    /// Ephemeral X25519 key agreement + 6-digit SAS confirmation + transcript
    /// binding. Requires user to compare and confirm the code on both devices
    /// before the invitation envelope is delivered.
    AuthenticatedV1 = 1,
}

impl BleRendezvousSecurity {
    /// The wire-encoded byte for this mode.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from a wire byte. Unknown values are rejected — no silent default.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Insecure),
            1 => Some(Self::AuthenticatedV1),
            _ => None,
        }
    }
}

/// Minimum SAS required: a confirmed-matching 6-digit code.
pub const SAS_DIGITS: u32 = 6;
pub const SAS_MODULUS: u32 = 1_000_000;
