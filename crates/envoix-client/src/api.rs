//! Canonical Manifest v2 application facade.

mod error;
mod invite;
mod options;
mod source;
mod transport;

use std::path::Path;

pub use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, EntryContentDigestV2, JobIdV2, ManifestEntryKindV2,
};
pub use envoix_protocol::manifest_v2_frames::RootPlanV2;
pub use envoix_session::*;

pub use error::TransferError;
pub use invite::{Invite, Role};
pub use options::{PathPolicy, TransferOptions};
pub use source::{PeerSource, TransferMode};
pub use transport::{
    TransportAvailability, TransportCandidate, TransportPreference, TransportProvider,
    TransportSelection, TransportSelectionError, TransportSelectionReason, TransportSelector,
};

/// Runtime settings shared by every Manifest v2 route.
#[derive(Clone, Debug)]
pub struct Client {
    pub identity: IdentityConfig,
    pub candidates: CandidateFilter,
    pub data_stream_window: u32,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            identity: IdentityConfig::default(),
            candidates: CandidateFilter::default(),
            data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
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
        }
    }
}

fn setup_error(error: envoix_error::CoreError) -> TransferError {
    TransferError::input(error.to_string())
}
