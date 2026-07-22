use std::fmt;

use zeroize::Zeroizing;

use crate::PairingError;

pub const MAX_PAIRING_CODE_SIZE: usize = 128;

pub struct PairingCode(Zeroizing<Vec<u8>>);

impl PairingCode {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, PairingError> {
        let bytes = Zeroizing::new(bytes.into());
        if bytes.is_empty() || bytes.len() > MAX_PAIRING_CODE_SIZE {
            return Err(PairingError::InvalidCodeLength {
                actual: bytes.len(),
                maximum: MAX_PAIRING_CODE_SIZE,
            });
        }
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode([redacted])")
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode([redacted])")
    }
}

pub struct DataPlaneToken(Zeroizing<[u8; 32]>);

impl DataPlaneToken {
    pub(crate) fn from_zeroizing(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self(bytes)
    }

    /// Explicitly crosses the redaction boundary for data-plane authentication.
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl PartialEq for DataPlaneToken {
    fn eq(&self, other: &Self) -> bool {
        blake3::Hash::from_bytes(*self.0) == blake3::Hash::from_bytes(*other.0)
    }
}

impl Eq for DataPlaneToken {}

impl fmt::Debug for DataPlaneToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DataPlaneToken([redacted])")
    }
}

impl fmt::Display for DataPlaneToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DataPlaneToken([redacted])")
    }
}
