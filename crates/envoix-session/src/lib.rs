//! Manifest v2 session orchestration and concrete iroh wiring.

mod candidates;
mod connection;
mod datagram_transport;
mod endpoint;
mod identity;
mod manifest_v2_session;
mod native_transport;
mod nearby_invite;
mod room;
mod room_control;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use envoix_auth::{
    AuthenticationOutcome, PairingConfig, REMEMBERED_PRESENCE_TAG_LEN,
    REMEMBERED_PRESENCE_TAG_PREFIX, RememberedCredential, RememberedSession, authenticate_receiver,
    authenticate_receiver_with_remember, authenticate_sender, authenticate_sender_with_remember,
};
use envoix_error::CoreError;
pub use envoix_error::RendezvousCause;
use envoix_protocol::PeerDescriptor;
pub use envoix_protocol::TransferProtocol;
pub use envoix_rendezvous_iroh::{generate_code, split_code};
pub use envoix_transfer::{
    AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES, AddSourceResult, CanonicalTransferJob,
    DEFAULT_INVENTORY_PAGE_SIZE, DeliveryAuthorityErrorV2, DestinationDecisionV2,
    DestinationModeV2, DestinationPlanErrorV2, DestinationPlanStoreV2, DestinationRequestV2,
    DestinationWritePlanV2, EventSink, InventoryCursor, InventoryItem, InventoryPage,
    InventorySummary, JobLifecycle, LocalDestinationProviderV2, LocalSourceOrigin,
    MAX_INVENTORY_PAGE_SIZE, ManifestV2DataError, ManifestV2DataPlane, ManifestV2DeliveryAuthority,
    ManifestV2PayloadSink, ManifestV2ProgressPhase, ManifestV2ProgressSink, ManifestV2ResultGate,
    NoopEventSink, NoopManifestV2ResultGate, POST_SAVE_RESERVE_BYTES, PreparedFileSource,
    ProviderSourceIssue, ReceiverDataPlaneLedgerV2, ReceiverDataPlaneStoreV2,
    ReceiverDataPlaneSummaryV2, ReceiverDeliveryRecordV2, ReceiverDeliveryStoreV2, SavedEntryV2,
    SenderDataPlaneSummaryV2, SenderDeliveryRecordV2, SenderDeliveryStoreV2, SenderResumeIntentV2,
    SenderTransferPhaseV2, SourceDecision, SourceIssue, SourceIssueKind, SourceItemId,
    SourceSelectionInfo, SourceSelectionState, StorageDomainIdentityV2, TransferCancelToken,
    TransferEvent, TransferJobError, TransferJobStore, TransferStage, VerifiedEntryV2,
    local_allocatable_bytes, sender_resume_intent,
};
pub use envoix_types::{DataPath, TransferDirection};
use iroh::Endpoint;
pub use iroh::EndpointAddr;
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;

pub use candidates::CandidateFilter;
use connection::IrohFrameConnection;
pub use datagram_transport::PlatformDatagramTransport;
pub use endpoint::{
    BindAddrs, BoundEndpoint, DEFAULT_DATA_STREAM_WINDOW, MAX_DATA_STREAM_WINDOW,
    MIN_DATA_STREAM_WINDOW, parse_broker_addr,
};
use endpoint::{
    build_dial_endpoint, build_manifest_v2_accept_endpoint,
    build_manifest_v2_advertising_accept_endpoint, peer_addr_from_descriptor,
};
pub use identity::{IdentityConfig, MemoryIdentity};
pub use manifest_v2_session::{
    PendingManifestV2Receive, PendingNativeManifestV2Receive, ReceiverManifestV2SessionSummary,
    SenderManifestV2SessionSummary, receive_manifest_v2_offer,
    receive_manifest_v2_offer_over_datagram_transport,
    receive_manifest_v2_offer_over_native_transport, receive_manifest_v2_offer_with_authentication,
    send_manifest_v2_manual, send_manifest_v2_over_datagram_transport,
    send_manifest_v2_over_native_transport, send_manifest_v2_to_endpoint_addr,
    send_manifest_v2_to_endpoint_addr_with_authentication,
};
pub use native_transport::{
    NATIVE_TRANSPORT_IO_CHUNK_BYTES, NativeFrameConnection, NativeTransportRead,
    NativeTransportRole, PlatformDuplexTransport,
};
pub use nearby_invite::{
    MAX_NEARBY_INVITE_BYTES, NEARBY_INVITE_ALPN, NearbyInvite, NearbyInviteEndpoint,
    NearbyInviteInbox, start_nearby_invite_inbox,
};
pub use room::{
    receive_manifest_v2_offer_via_remembered, receive_manifest_v2_offer_via_room,
    receive_manifest_v2_offer_via_room_hybrid,
    receive_manifest_v2_offer_via_room_hybrid_with_authentication,
    receive_manifest_v2_offer_via_room_with_authentication, send_manifest_v2_via_remembered,
    send_manifest_v2_via_room, send_manifest_v2_via_room_hybrid,
    send_manifest_v2_via_room_hybrid_with_authentication,
    send_manifest_v2_via_room_with_authentication,
};
pub use room_control::{
    ROOM_CONTROL_ALPN, RelationshipUpgradeRejection, RememberedRoomControlConnectError,
    RememberedRoomControlRole, RoomCloseReason, RoomControlEvent, RoomControlInvite,
    RoomControlSession, RoomLifetimePolicy, RoomLifetimeState, RoomOfferRejection,
    RoomTransferOffer, connect_remembered_room_control, connect_room_control,
};

const AUTH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_AUTH_FAILURES: u32 = 50;
const MAX_PRE_AUTH_CONNECTION_FAILURES: u32 = 8;
const MDNS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const MDNS_CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

pub type SessionError = CoreError;

/// Secure persistence hook invoked after data-plane authentication and before
/// any Manifest frame.
pub trait AuthenticationHandler: Send + Sync {
    /// Only InviteV2 sessions may request the mutual Remember extension.
    fn remember_consent(&self) -> bool {
        false
    }

    /// Persist a negotiated credential or remembered-generation rotation.
    /// Returning an error closes the authenticated connection before transfer.
    fn on_authenticated(&self, outcome: AuthenticationOutcome) -> Result<(), SessionError>;
}

/// Default handler for sessions that do not create or rotate relationships.
pub struct NoopAuthenticationHandler;

impl AuthenticationHandler for NoopAuthenticationHandler {
    fn on_authenticated(&self, _outcome: AuthenticationOutcome) -> Result<(), SessionError> {
        Ok(())
    }
}

/// Client retry policy for broker-owned Room outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RendezvousRetryPolicy {
    /// PAKE/control attempts after a creator and joiner have been matched.
    pub pairing_attempts: usize,
    /// Rejections with acceptable server retry guidance that may be retried.
    pub server_retries: usize,
    /// A larger server delay ends the operation instead of retrying early.
    pub max_retry_after: Duration,
}

impl Default for RendezvousRetryPolicy {
    fn default() -> Self {
        Self {
            pairing_attempts: 4,
            server_retries: 4,
            max_retry_after: Duration::from_secs(30),
        }
    }
}

/// Runtime transport policy. Manifest block size and compression belong to the
/// sealed job and are deliberately absent from this session configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionConfig {
    pub identity: IdentityConfig,
    pub relay: Option<String>,
    pub relay_only: bool,
    pub direct_only: bool,
    pub candidates: CandidateFilter,
    pub data_stream_window: u32,
    pub rendezvous_retry: RendezvousRetryPolicy,
}

impl SessionConfig {
    pub(crate) fn data_relay(&self) -> Option<String> {
        if self.direct_only {
            None
        } else {
            self.relay.clone()
        }
    }
}

pub async fn bind_iroh_manifest_v2_endpoint(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_manifest_v2_accept_endpoint(
            listen_addrs.into(),
            identity,
            relay,
            relay_only,
            candidates,
            window,
        )
        .await?,
        candidates: candidates.clone(),
    })
}

pub async fn bind_iroh_manifest_v2_endpoint_enable_mdns(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_manifest_v2_advertising_accept_endpoint(
            listen_addrs.into(),
            identity,
            &None,
            false,
            candidates,
            window,
        )
        .await?,
        candidates: candidates.clone(),
    })
}

/// Sends one sealed job to the first mDNS endpoint that authenticates and
/// accepts Manifest v2. Discovery never selects another data-plane engine.
pub async fn send_manifest_v2_enable_mdns(
    job: CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    if job.manifest().is_none() {
        return Err(CoreError::InvalidInput(
            "transfer job must be sealed before mDNS discovery".into(),
        ));
    }
    let discovery_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let send_events = events.clone();
    let send_cancel = cancel.clone();
    let result = send_to_first_mdns_peer(
        &discovery_endpoint,
        events.as_ref(),
        &cancel,
        move |peer_addr| {
            let job = job.clone();
            let state_directory = state_directory.clone();
            let config = config.clone();
            let events = send_events.clone();
            let cancel = send_cancel.clone();
            async move {
                send_manifest_v2_to_endpoint_addr(
                    peer_addr,
                    &job,
                    state_directory,
                    config,
                    pairing,
                    events,
                    &cancel,
                )
                .await
            }
        },
    )
    .await;
    discovery_endpoint.close().await;
    result
}

async fn send_to_first_mdns_peer<T, F, Fut>(
    local_endpoint: &Endpoint,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
    mut send: F,
) -> Result<T, SessionError>
where
    F: FnMut(EndpointAddr) -> Fut,
    Fut: Future<Output = Result<T, SessionError>>,
{
    let mdns = MdnsAddressLookup::builder()
        .advertise(false)
        .build(local_endpoint.id())
        .map_err(|error| CoreError::Discovery(error.to_string()))?;
    local_endpoint
        .address_lookup()
        .map_err(|error| CoreError::Discovery(error.to_string()))?
        .add(mdns.clone());

    let mut discoveries = mdns.subscribe().await;
    let mut tried = std::collections::HashSet::new();
    let mut deadline = tokio::time::Instant::now() + MDNS_DISCOVERY_TIMEOUT;
    let mut last_error = None;
    loop {
        let event = tokio::select! {
            result = tokio::time::timeout_at(deadline, discoveries.next()) => match result {
                Ok(Some(event)) => event,
                Ok(None) | Err(_) => break,
            },
            () = cancel.cancelled() => return Err(interrupted_error(cancel)),
        };
        let DiscoveryEvent::Discovered {
            endpoint_info: discovered_peer,
            ..
        } = event
        else {
            continue;
        };
        if discovered_peer.endpoint_id == local_endpoint.id()
            || !tried.insert(discovered_peer.endpoint_id)
        {
            continue;
        }
        match tokio::time::timeout(
            MDNS_CONNECT_TIMEOUT,
            send(discovered_peer.to_endpoint_addr()),
        )
        .await
        {
            Ok(Ok(summary)) => return Ok(summary),
            Ok(Err(error)) => {
                events.on_event(TransferEvent::Diagnostic {
                    message: "mDNS candidate failed; trying the next candidate".into(),
                });
                last_error = Some(error);
            }
            Err(_) => {
                events.on_event(TransferEvent::Diagnostic {
                    message: "mDNS candidate timed out; trying the next candidate".into(),
                });
                last_error = Some(CoreError::Transport(
                    "mDNS candidate connection timed out".into(),
                ));
            }
        }
        if cancel.is_cancelled() {
            return Err(interrupted_error(cancel));
        }
        deadline = tokio::time::Instant::now() + MDNS_DISCOVERY_TIMEOUT;
    }
    Err(last_error.unwrap_or_else(|| {
        CoreError::Discovery(format!(
            "no iroh mDNS peers discovered within {} seconds",
            MDNS_DISCOVERY_TIMEOUT.as_secs()
        ))
    }))
}

pub async fn receive_manifest_v2_offer_with_bound_peer<F>(
    listen_addrs: impl Into<BindAddrs>,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    on_bound_peer: F,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let timeline = manifest_v2_session::start_manifest_v2_receive_attempt(events.clone());
    let bound_endpoint = match bind_iroh_manifest_v2_endpoint(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await
    {
        Ok(bound_endpoint) => bound_endpoint,
        Err(error) => {
            timeline.record(manifest_v2_session::failure_stage(cancel));
            return Err(error);
        }
    };
    let endpoint_addr = bound_endpoint
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;
    let peer = match bound_endpoint.peer_descriptor() {
        Ok(peer) => peer,
        Err(error) => {
            timeline.record(manifest_v2_session::failure_stage(cancel));
            bound_endpoint.local_endpoint.close().await;
            return Err(error);
        }
    };
    let relay_urls = endpoint_addr
        .relay_urls()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    on_bound_peer(peer, relay_urls);
    manifest_v2_session::receive_manifest_v2_offer_with_authentication_and_timeline(
        bound_endpoint,
        pairing,
        events,
        cancel,
        &NoopAuthenticationHandler,
        Some(timeline),
    )
    .await
}

pub async fn receive_manifest_v2_offer_enable_mdns<F>(
    listen_addrs: impl Into<BindAddrs>,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    on_bound_peer: F,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let timeline = manifest_v2_session::start_manifest_v2_receive_attempt(events.clone());
    let bound_endpoint = match bind_iroh_manifest_v2_endpoint_enable_mdns(
        listen_addrs,
        &config.identity,
        &config.candidates,
        config.data_stream_window,
    )
    .await
    {
        Ok(bound_endpoint) => bound_endpoint,
        Err(error) => {
            timeline.record(manifest_v2_session::failure_stage(cancel));
            return Err(error);
        }
    };
    let peer = match bound_endpoint.peer_descriptor() {
        Ok(peer) => peer,
        Err(error) => {
            timeline.record(manifest_v2_session::failure_stage(cancel));
            bound_endpoint.local_endpoint.close().await;
            return Err(error);
        }
    };
    on_bound_peer(peer, Vec::new());
    manifest_v2_session::receive_manifest_v2_offer_with_authentication_and_timeline(
        bound_endpoint,
        pairing,
        events,
        cancel,
        &NoopAuthenticationHandler,
        Some(timeline),
    )
    .await
}

pub(crate) async fn dial_peer_addr_for_protocol(
    local_endpoint: Endpoint,
    peer_addr: EndpointAddr,
    protocol: TransferProtocol,
) -> Result<IrohFrameConnection, SessionError> {
    let connection = local_endpoint
        .connect(peer_addr, protocol.alpn())
        .await
        .map_err(|error| CoreError::Transport(error.to_string()))?;
    let negotiated = TransferProtocol::from_alpn(connection.alpn()).ok_or_else(|| {
        CoreError::Protocol(format!(
            "unsupported negotiated ALPN {:?}",
            String::from_utf8_lossy(connection.alpn())
        ))
    })?;
    if negotiated != protocol {
        connection.close(iroh::endpoint::VarInt::from_u32(0), b"alpn mismatch");
        return Err(CoreError::Protocol(
            "negotiated ALPN differs from request".into(),
        ));
    }
    let (send, recv) = connection
        .open_bi()
        .await
        .map_err(|error| CoreError::Transport(error.to_string()))?;
    Ok(IrohFrameConnection::new(
        local_endpoint,
        connection,
        send,
        recv,
        protocol,
    ))
}

pub(crate) async fn auth_bounded<T>(
    auth: impl Future<Output = Result<T, SessionError>>,
    cancel: &TransferCancelToken,
) -> Result<T, SessionError> {
    tokio::select! {
        result = tokio::time::timeout(AUTH_TIMEOUT, auth) => match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Protocol("authentication timed out".into())),
        },
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    }
}

pub(crate) fn interrupted_error(_cancel: &TransferCancelToken) -> SessionError {
    CoreError::Cancelled
}
