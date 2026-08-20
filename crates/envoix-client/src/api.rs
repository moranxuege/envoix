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
// Temporary compatibility surface for consumers that have not moved to the
// v0.3 command/event boundary. Keep this list explicit so new session details
// cannot become application API by accident.
use envoix_session::DEFAULT_DATA_STREAM_WINDOW;
pub use envoix_session::{
    AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES, AuthenticationHandler, AuthenticationOutcome, BindAddrs,
    CandidateFilter, CanonicalTransferJob, DataPath, DestinationDecisionV2, DestinationRequestV2,
    EndpointAddr, EventSink, IdentityConfig, InventoryCursor, InventoryItem, JobLifecycle,
    LocalSourceOrigin, ManifestV2DataError, ManifestV2ProgressPhase, ManifestV2ResultGate,
    NativeTransportRead, NearbyInvite, NearbyInviteEndpoint, NearbyInviteInbox, PairingConfig,
    PendingManifestV2Receive, PendingNativeManifestV2Receive, PlatformDatagramTransport,
    PlatformDuplexTransport, ProviderSourceIssue, REMEMBERED_PRESENCE_TAG_LEN, ROOM_CONTROL_ALPN,
    ReceiverManifestV2SessionSummary, RememberedCredential, RememberedRoomControlConnectError,
    RememberedRoomControlRole, RendezvousCause, RendezvousRetryPolicy, RoomCloseReason,
    RoomControlEvent, RoomControlInvite, RoomControlSession, RoomLifetimePolicy, RoomLifetimeState,
    RoomOfferRejection, RoomTransferOffer, SavedEntryV2, SenderManifestV2SessionSummary,
    SessionConfig, SessionError, SourceDecision, SourceIssue, SourceIssueKind, SourceItemId,
    SourceSelectionInfo, SourceSelectionState, TransferCancelToken, TransferDirection,
    TransferEvent, TransferJobStore, TransferStage, connect_remembered_room_control,
    connect_room_control, local_allocatable_bytes, parse_broker_addr,
    receive_manifest_v2_offer_enable_mdns, receive_manifest_v2_offer_over_datagram_transport,
    receive_manifest_v2_offer_over_native_transport, receive_manifest_v2_offer_via_remembered,
    receive_manifest_v2_offer_via_room,
    receive_manifest_v2_offer_via_room_hybrid_with_authentication,
    receive_manifest_v2_offer_via_room_with_authentication,
    receive_manifest_v2_offer_with_bound_peer, send_manifest_v2_enable_mdns,
    send_manifest_v2_manual, send_manifest_v2_over_datagram_transport,
    send_manifest_v2_over_native_transport, send_manifest_v2_via_remembered,
    send_manifest_v2_via_room, send_manifest_v2_via_room_hybrid_with_authentication,
    send_manifest_v2_via_room_with_authentication, start_nearby_invite_inbox,
};

pub use credential_store::DesktopCredentialStore;
pub use error::TransferError;
pub use invite::{
    BootstrapKind, Capabilities, CreatedInvitation, InvitationAuthContext, InvitationBootstrap,
    InvitationError, InvitationErrorCode, InvitationPublicContext, InvitationSide, InviteV2,
    RoomCode, TransferRole, ValidatedInvitation, create_invitation, parse_invitation_for_role,
    parse_invitation_for_routing,
};
pub use options::{PathPolicy, TransferOptions};
pub use source::{
    InvitationConsumption, InvitationLease, InviteSecretRef, PeerSource, RememberedCredentialRef,
    SharedTokenRef, TransferMode, acquire_invitation, acquire_remembered_credential,
    acquire_shared_token, register_remembered_credential,
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
        let config = crate::configuration::RuntimeConfig::read(path).map_err(setup_error)?;
        if let Some(window) = config.data_stream_window {
            client.data_stream_window =
                crate::configuration::parse_window(&window).map_err(setup_error)?;
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
