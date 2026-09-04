//! Typed operating-system boundaries exposed to the application engine.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::storage::VaultReference;

pub const MAX_VAULT_SECRET_BYTES: usize = 64 * 1024;

/// Stable platform failure categories. Product policy must never parse a
/// platform diagnostic string to decide recovery or terminal state.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PlatformPortError {
    #[error("platform capability is unavailable")]
    Unavailable,
    #[error("platform capability is temporarily limited")]
    Limited,
    #[error("platform permission was denied")]
    PermissionDenied,
    #[error("platform interaction is required")]
    InteractionRequired,
    #[error("platform request is invalid")]
    InvalidRequest,
    #[error("platform data is corrupt")]
    CorruptData,
    #[error("platform operation was canceled")]
    Canceled,
}

/// Opaque secret material exchanged only with a trusted vault adapter.
///
/// This type deliberately implements neither serialization nor cloning and
/// clears its allocation on drop. Presentation and control protocols must use
/// [`VaultReference`] instead.
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Result<Self, PlatformPortError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.is_empty() || bytes.len() > MAX_VAULT_SECRET_BYTES {
            return Err(PlatformPortError::InvalidRequest);
        }
        Ok(Self(bytes))
    }

    /// Exposes bytes only to the trusted implementation of a vault operation.
    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes(<redacted>)")
    }
}

/// Secure credential storage owned by the Engine host.
///
/// Implementations must not prompt while servicing background or polling
/// work. A vault that needs user interaction returns `InteractionRequired` so
/// the host can request it explicitly.
pub trait SecureVaultPort: Send + Sync {
    fn contains(&self, reference: &VaultReference) -> Result<bool, PlatformPortError>;

    fn store(
        &self,
        reference: &VaultReference,
        secret: &SecretBytes,
    ) -> Result<(), PlatformPortError>;

    fn load(&self, reference: &VaultReference) -> Result<Option<SecretBytes>, PlatformPortError>;

    fn delete(&self, reference: &VaultReference) -> Result<(), PlatformPortError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformCapability {
    SecureVault,
    FileSource,
    FileDestination,
    NearbyDiscovery,
    ClipboardRead,
    ClipboardWrite,
    BackgroundExecution,
    Notifications,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityAvailability {
    Available,
    Limited,
    Unavailable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformCapabilities {
    values: BTreeMap<PlatformCapability, CapabilityAvailability>,
}

impl PlatformCapabilities {
    pub fn new(
        values: impl IntoIterator<Item = (PlatformCapability, CapabilityAvailability)>,
    ) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub fn availability(&self, capability: PlatformCapability) -> CapabilityAvailability {
        self.values
            .get(&capability)
            .copied()
            .unwrap_or(CapabilityAvailability::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_bytes_are_bounded_and_redacted() {
        let secret = SecretBytes::new(b"opaque credential".to_vec()).unwrap();
        assert_eq!(secret.expose(), b"opaque credential");
        assert_eq!(format!("{secret:?}"), "SecretBytes(<redacted>)");
        assert_eq!(
            SecretBytes::new(Vec::new()).unwrap_err(),
            PlatformPortError::InvalidRequest
        );
        assert_eq!(
            SecretBytes::new(vec![0; MAX_VAULT_SECRET_BYTES + 1]).unwrap_err(),
            PlatformPortError::InvalidRequest
        );
    }
}
