//! Canonical Manifest v2 application facade.

mod credential_store;
mod error;
mod invite;
mod options;
mod source;

use std::path::Path;
use std::time::Duration;

pub use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, EntryContentDigestV2, JobIdV2, ManifestEntryKindV2,
};
pub use envoix_protocol::manifest_v2_frames::RootPlanV2;
pub use envoix_session::*;

pub use credential_store::DesktopCredentialStore;
pub use error::TransferError;
pub use invite::{
    BootstrapKind, Capabilities, CreatedInvitation, InvitationAuthContext, InvitationBootstrap,
    InvitationError, InvitationErrorCode, InvitationPublicContext, InvitationSide, InviteV2,
    RoomCode, TransferRole, ValidatedInvitation, create_invitation, parse_invitation_for_role,
    parse_invitation_for_routing, parse_room_code,
};
pub use options::{PathPolicy, TransferOptions};
pub use source::{
    InvitationLease, InviteSecretRef, PeerSource, RememberedCredentialRef, SharedTokenRef,
    TransferMode, acquire_invitation, acquire_remembered_credential, acquire_shared_token,
    register_remembered_credential,
};

/// Runtime settings shared by every Manifest v2 route.
#[derive(Clone, Debug)]
pub struct Client {
    pub identity: IdentityConfig,
    pub candidates: CandidateFilter,
    pub data_stream_window: u32,
    pub rendezvous_retry: RendezvousRetryPolicy,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            identity: IdentityConfig::default(),
            candidates: CandidateFilter::default(),
            data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
            rendezvous_retry: RendezvousRetryPolicy::default(),
        }
    }
}

impl Client {
    /// Loads optional transport tuning without changing job semantics.
    pub fn from_runtime_sources(config_path: Option<&Path>) -> Result<Self, TransferError> {
        let mut client = Self::default();
        let Some(path) = config_path else {
            return Ok(client);
        };
        let config = crate::RuntimeConfig::read(path).map_err(setup_error)?;
        if let Some(window) = config.data_stream_window {
            client.data_stream_window = crate::parse_window(&window).map_err(setup_error)?;
        }
        if let Some(candidates) = config.candidates {
            client.candidates = CandidateFilter::from_lists(&candidates.allow, &candidates.deny)
                .map_err(setup_error)?;
        }
        if let Some(attempts) = config.rendezvous_pairing_attempts {
            if attempts == 0 {
                return Err(TransferError::input(
                    "rendezvous_pairing_attempts must be non-zero",
                ));
            }
            client.rendezvous_retry.pairing_attempts = attempts;
        }
        if let Some(retries) = config.rendezvous_server_retries {
            client.rendezvous_retry.server_retries = retries;
        }
        if let Some(seconds) = config.rendezvous_max_retry_after_seconds {
            if seconds == 0 {
                return Err(TransferError::input(
                    "rendezvous_max_retry_after_seconds must be non-zero",
                ));
            }
            client.rendezvous_retry.max_retry_after = Duration::from_secs(seconds);
        }
        Ok(client)
    }

    /// Produces a concrete session policy for one route.
    pub fn session_config(&self, options: &TransferOptions) -> SessionConfig {
        SessionConfig {
            identity: self.identity.clone(),
            relay: options.relay.clone(),
            relay_only: options.path == PathPolicy::RelayOnly,
            direct_only: options.path == PathPolicy::DirectOnly,
            candidates: self.candidates.clone(),
            data_stream_window: self.data_stream_window,
            rendezvous_retry: self.rendezvous_retry,
        }
    }
}

fn setup_error(error: envoix_error::CoreError) -> TransferError {
    TransferError::input(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn runtime_config_applies_rendezvous_retry_policy() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            "rendezvous_pairing_attempts = 3\n\
             rendezvous_server_retries = 2\n\
             rendezvous_max_retry_after_seconds = 7\n"
        )
        .unwrap();
        let client = Client::from_runtime_sources(Some(file.path())).unwrap();
        assert_eq!(client.rendezvous_retry.pairing_attempts, 3);
        assert_eq!(client.rendezvous_retry.server_retries, 2);
        assert_eq!(
            client.rendezvous_retry.max_retry_after,
            Duration::from_secs(7)
        );
    }

    #[test]
    fn runtime_config_rejects_zero_retry_bounds() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "rendezvous_pairing_attempts = 0").unwrap();
        assert!(Client::from_runtime_sources(Some(file.path())).is_err());
    }
}
