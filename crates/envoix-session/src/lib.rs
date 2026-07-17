//! Session orchestration for transfer setup and concrete iroh wiring.

mod candidates;
mod connection;
mod endpoint;
mod identity;
mod room;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use envoix_auth::{PairingConfig, authenticate_receiver, authenticate_sender};
use envoix_error::CoreError;
pub use envoix_protocol::TransferProtocol;
use envoix_protocol::{FrameConnection, PeerDescriptor};
pub use envoix_transfer::{
    DEFAULT_CHUNK_SIZE, EventSink, MAX_CHUNK_SIZE, MIN_CHUNK_SIZE, ManifestEventSink,
    ManifestNoopEventSink, ManifestSendRequest, ManifestTransferEngine, ManifestTransferEvent,
    ManifestTransferSummary, NoopEventSink, PEER_INTERRUPT_MESSAGE, PEER_PAUSE_MESSAGE,
    TransferCancelToken, TransferEngine, TransferEvent, TransferSummary, USER_INTERRUPT_MESSAGE,
    USER_PAUSE_MESSAGE, discard_manifest_resume_state, validate_chunk_size,
};
pub use envoix_types::TransferDirection;
// Re-exported so the client facade reaches rendezvous-code helpers through its
// own service layer instead of depending on envoix-rendezvous-iroh directly.
pub use envoix_rendezvous_iroh::{generate_code, split_code};
use iroh::Endpoint;
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;

pub use candidates::CandidateFilter;
use connection::IrohFrameConnection;
pub use endpoint::{
    BindAddrs, BoundEndpoint, DEFAULT_DATA_STREAM_WINDOW, MAX_DATA_STREAM_WINDOW,
    MIN_DATA_STREAM_WINDOW, parse_broker_addr,
};
use endpoint::{
    build_accept_endpoint, build_advertising_accept_endpoint, build_dial_endpoint,
    build_transfer_accept_endpoint, build_transfer_advertising_accept_endpoint,
    peer_addr_from_descriptor,
};
pub use identity::{IdentityConfig, MemoryIdentity};
pub use iroh::EndpointAddr;
pub use room::{
    receive_file_via_room, receive_transfer_via_room, send_file_via_room, send_manifest_via_room,
};

const MAX_AUTH_FAILURES: u32 = 50;
/// Grace period for one auth handshake. An accepted (or dialed) peer that goes
/// silent must not pin the session: without a bound, cancel only takes effect
/// on transport failure, and a receiver's failure counter never counts an auth
/// that refuses to *end*.
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);
const MDNS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-candidate connect budget in the mDNS send loop: a stale or unreachable
/// endpoint fails this fast instead of hanging until the full transport timeout.
const MDNS_CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// Error type returned by session orchestration.
pub type SessionError = CoreError;

/// Stable diagnostic code for an older peer that rejects Manifest v1 during
/// ALPN negotiation, before authentication or payload transfer.
pub const MANIFEST_UNSUPPORTED_PEER_CODE: &str = "manifest.unsupported_peer";

/// Event sink used by a receiver that can negotiate either transfer protocol.
///
/// Existing single-file entry points continue to accept [`EventSink`]. New
/// negotiated entry points require both event families so the receiver can
/// route only after ALPN negotiation without dropping lifecycle information.
pub trait SessionEventSink: EventSink + ManifestEventSink {}

impl<T> SessionEventSink for T where T: EventSink + ManifestEventSink {}

/// Event sink that ignores both single-file and Manifest lifecycle events.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopSessionEventSink;

impl EventSink for NoopSessionEventSink {
    fn on_event(&self, _event: TransferEvent) {}
}

impl ManifestEventSink for NoopSessionEventSink {
    fn on_manifest_event(&self, _event: ManifestTransferEvent) {}
}

/// Successful result from an ALPN-negotiated receive session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTransferSummary {
    /// Existing `envoix/1` single-file transfer result.
    SingleFile(TransferSummary),
    /// Additive `envoix/manifest/1` transfer-set result.
    Manifest(ManifestTransferSummary),
}

impl SessionTransferSummary {
    /// Returns the protocol that produced this result.
    pub const fn protocol(&self) -> TransferProtocol {
        match self {
            Self::SingleFile(_) => TransferProtocol::SingleFileV1,
            Self::Manifest(_) => TransferProtocol::ManifestV1,
        }
    }
}

/// Runtime options used when wiring transports into the transfer engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionConfig {
    /// Maximum chunk payload size sent by the transfer engine.
    pub chunk_size: usize,
    /// iroh endpoint identity policy.
    pub identity: IdentityConfig,
    /// Optional relay URL for WAN/NAT reachability. `None` keeps endpoints
    /// LAN/direct only (unchanged behavior); `Some(url)` routes through a relay.
    pub relay: Option<String>,
    /// Force the relay data path by binding no IP transport, so direct/holepunch
    /// is impossible and the transfer must go through the relay (for A/B testing
    /// relay vs direct). Requires `relay` to be set.
    pub relay_only: bool,
    /// Force a direct/holepunched data path by disabling the relay for the data
    /// endpoint only: the relay is still used to reach the rendezvous broker, but
    /// the transfer itself gets no relay fallback (direct-or-fail). For A/B
    /// testing and confirming a direct path really works.
    pub direct_only: bool,
    /// CIDR filter over the candidate addresses we advertise to a peer.
    pub candidates: CandidateFilter,
    /// Per-stream QUIC flow-control window (bytes) for this session's *data*
    /// endpoints. Frozen at session creation; a transport tuning only, so it
    /// never touches the wire header, resume state, or any hash.
    pub data_stream_window: u32,
}

impl SessionConfig {
    /// The relay the *data* endpoint should use. `None` when `direct_only` (no
    /// relay fallback for the transfer), otherwise the configured relay. The
    /// rendezvous endpoint keeps using [`SessionConfig::relay`] regardless, so
    /// direct-only still reaches a NATed broker through the relay.
    fn data_relay(&self) -> Option<String> {
        if self.direct_only {
            None
        } else {
            self.relay.clone()
        }
    }
}

/// Bind an accepting iroh endpoint, routed through `relay` (a relay URL) when
/// set, so the bound endpoint stays reachable from behind NAT. With `relay_only`
/// it binds no IP transport, forcing the relay data path.
pub(crate) async fn bind_iroh_endpoint_with_relay(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_accept_endpoint(
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

/// Bind an accepting endpoint for both the compatible single-file protocol and
/// Manifest v1. The negotiated ALPN is retained on the accepted connection and
/// routed only after the existing authentication handshake succeeds.
pub(crate) async fn bind_iroh_transfer_endpoint_with_relay(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_transfer_accept_endpoint(
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

/// Bind an iroh endpoint (listen addr) and advertise it through iroh mDNS address lookup.
pub async fn bind_iroh_endpoint_enable_mdns(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_advertising_accept_endpoint(
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

/// Bind and advertise an mDNS endpoint that accepts both the compatible
/// single-file protocol and Manifest v1.
pub async fn bind_iroh_transfer_endpoint_enable_mdns(
    listen_addrs: impl Into<BindAddrs>,
    identity: &IdentityConfig,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_transfer_advertising_accept_endpoint(
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

/// Sends one file to a manually supplied peer descriptor, stopping on cancellation.
pub async fn send_file_manual(
    peer: PeerDescriptor,
    file_path: PathBuf,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let local_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let events: Arc<dyn EventSink> = Arc::from(events);
    events.on_event(TransferEvent::Connecting);
    let mut connection = match dial(local_endpoint.clone(), &peer).await {
        Ok(connection) => connection,
        Err(error) => {
            // Close the endpoint before returning, so a failed dial does not
            // drop it with active state (iroh logs an ungraceful-close error).
            local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);

    if let Err(error) = auth_bounded(authenticate_sender(&mut connection, pairing), &cancel).await {
        let _ = connection.close().await;
        local_endpoint.close().await;
        return Err(error);
    }
    let result = engine
        .send_file_with_cancel(&mut connection, file_path, resume, events.as_ref(), &cancel)
        .await;
    let _ = connection.close().await;
    local_endpoint.close().await;
    result
}

/// Sends one file to a peer addressed by its full iroh `EndpointAddr` (which
/// may carry a relay home), dialing through the configured relay when set and
/// stopping the data transfer on cancellation.
pub async fn send_file_to_endpoint_addr(
    peer_addr: EndpointAddr,
    file_path: PathBuf,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let local_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let events: Arc<dyn EventSink> = Arc::from(events);
    events.on_event(TransferEvent::Connecting);
    let mut connection = match dial_peer_addr(local_endpoint.clone(), peer_addr).await {
        Ok(connection) => connection,
        Err(error) => {
            local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);
    if let Err(error) = auth_bounded(authenticate_sender(&mut connection, pairing), &cancel).await {
        let _ = connection.close().await;
        local_endpoint.close().await;
        return Err(error);
    }
    let result = engine
        .send_file_with_cancel(&mut connection, file_path, resume, events.as_ref(), &cancel)
        .await;
    let _ = connection.close().await;
    local_endpoint.close().await;
    result
}

/// Sends one Manifest transfer set to a manually supplied peer descriptor.
///
/// The sender requests only `envoix/manifest/1`; it never falls back to the
/// single-file protocol or repackages the request when the peer is older.
pub async fn send_manifest_manual(
    peer: PeerDescriptor,
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<ManifestTransferSummary, SessionError> {
    let peer_addr = peer_addr_from_descriptor(&peer)?;
    send_manifest_to_address(peer_addr, request, resume, config, pairing, events, cancel).await
}

/// Sends one Manifest transfer set to a full iroh endpoint address.
pub async fn send_manifest_to_endpoint_addr(
    peer_addr: EndpointAddr,
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<ManifestTransferSummary, SessionError> {
    send_manifest_to_address(peer_addr, request, resume, config, pairing, events, cancel).await
}

async fn send_manifest_to_address(
    peer_addr: EndpointAddr,
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<ManifestTransferSummary, SessionError> {
    let local_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let events: Arc<dyn SessionEventSink> = Arc::from(events);
    let result = send_manifest_to_peer_addr(
        local_endpoint.clone(),
        peer_addr,
        request,
        resume,
        config,
        pairing,
        events,
        &cancel,
        None,
    )
    .await;
    local_endpoint.close().await;
    result
}

/// Sends one file to the first mDNS-discovered iroh endpoint that
/// authenticates, stopping on cancellation.
pub async fn send_file_enable_mdns(
    file_path: PathBuf,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let events: Arc<dyn EventSink> = Arc::from(events);
    let local_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let send_endpoint = local_endpoint.clone();
    let send_events = events.clone();
    let send_cancel = cancel.clone();
    let result = send_to_first_mdns_peer(
        &local_endpoint,
        events.as_ref(),
        &cancel,
        move |peer_addr| {
            let local_endpoint = send_endpoint.clone();
            let file_path = file_path.clone();
            let config = config.clone();
            let events = send_events.clone();
            let cancel = send_cancel.clone();
            async move {
                send_file_to_peer_addr(
                    local_endpoint,
                    peer_addr,
                    file_path,
                    resume,
                    config,
                    pairing,
                    events,
                    &cancel,
                    MDNS_CONNECT_TIMEOUT,
                )
                .await
            }
        },
    )
    .await;
    local_endpoint.close().await;
    result
}

/// Sends one Manifest transfer set to the first mDNS-discovered endpoint that
/// authenticates and supports Manifest v1.
pub async fn send_manifest_enable_mdns(
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<ManifestTransferSummary, SessionError> {
    let events: Arc<dyn SessionEventSink> = Arc::from(events);
    let local_endpoint = build_dial_endpoint(
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let send_endpoint = local_endpoint.clone();
    let send_events = events.clone();
    let send_cancel = cancel.clone();
    let result = send_to_first_mdns_peer(
        &local_endpoint,
        events.as_ref(),
        &cancel,
        move |peer_addr| {
            let local_endpoint = send_endpoint.clone();
            let request = request.clone();
            let config = config.clone();
            let events = send_events.clone();
            let cancel = send_cancel.clone();
            async move {
                send_manifest_to_peer_addr(
                    local_endpoint,
                    peer_addr,
                    request,
                    resume,
                    config,
                    pairing,
                    events,
                    &cancel,
                    Some(MDNS_CONNECT_TIMEOUT),
                )
                .await
            }
        },
    )
    .await;
    local_endpoint.close().await;
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
    let mut tried: std::collections::HashSet<_> = std::collections::HashSet::new();
    let mut next_deadline = tokio::time::Instant::now() + MDNS_DISCOVERY_TIMEOUT;
    let mut last_error = None;

    // Try every freshly discovered endpoint (deduped by id) with a bounded
    // connect, so a stale/unreachable candidate can't starve the live one: a
    // failed connect just moves on to the next candidate. Give up only when no
    // *new* endpoint shows up within the discovery window (re-advertisements of
    // already-tried peers don't extend it).
    loop {
        let event = tokio::select! {
            result = tokio::time::timeout_at(next_deadline, discoveries.next()) => match result {
                Ok(Some(event)) => event,
                Ok(None) | Err(_) => break,
            },
            () = cancel.cancelled() => {
                return Err(interrupted_error(cancel));
            }
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

        match send(discovered_peer.to_endpoint_addr()).await {
            Ok(summary) => return Ok(summary),
            Err(error) => {
                events.on_event(TransferEvent::Failed {
                    direction: TransferDirection::Send,
                    reason: error.to_string(),
                });
                if cancel.is_cancelled() {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
        next_deadline = tokio::time::Instant::now() + MDNS_DISCOVERY_TIMEOUT;
    }

    Err(last_error.unwrap_or_else(|| {
        CoreError::Discovery(format!(
            "no iroh mDNS peers discovered within {} seconds",
            MDNS_DISCOVERY_TIMEOUT.as_secs()
        ))
    }))
}

/// Receives one file, reporting the concrete bound peer descriptor before
/// accepting; stops while waiting or transferring if cancelled.
pub async fn receive_file_with_bound_peer<F>(
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    on_bound_peer: F,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let bound_endpoint = bind_iroh_endpoint_with_relay(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let endpoint_addr = bound_endpoint
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;
    let peer = bound_endpoint.peer_descriptor()?;
    let relay_urls = endpoint_addr
        .relay_urls()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    on_bound_peer(peer, relay_urls);
    receive_one_authenticated(bound_endpoint, output_dir, config, pairing, events, cancel).await
}

/// Receives one authenticated transfer over an endpoint that advertises both
/// supported ALPNs, reporting the bound peer descriptor before accepting.
///
/// Existing single-file callers keep using [`receive_file_with_bound_peer`].
/// This additive entry point returns a typed summary because its result shape is
/// known only after ALPN negotiation.
pub async fn receive_transfer_with_bound_peer<F>(
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    on_bound_peer: F,
    cancel: TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let bound_endpoint = bind_iroh_transfer_endpoint_with_relay(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let endpoint_addr = bound_endpoint
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;
    let peer = bound_endpoint.peer_descriptor()?;
    let relay_urls = endpoint_addr
        .relay_urls()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    on_bound_peer(peer, relay_urls);
    receive_one_authenticated_transfer(bound_endpoint, output_dir, config, pairing, events, cancel)
        .await
}

/// Receives one file over an mDNS-advertised endpoint: binds an mDNS endpoint,
/// reports the bound peer descriptor through `on_bound_peer` (so the caller can
/// advertise it), then accepts the first dialer that authenticates, ignoring
/// failed pairings. Stops on cancellation. The mDNS counterpart of
/// [`receive_file_with_bound_peer`].
pub async fn receive_file_enable_mdns<F>(
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    on_bound_peer: F,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let bound_endpoint = bind_iroh_endpoint_enable_mdns(
        listen_addrs,
        &config.identity,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let peer = bound_endpoint.peer_descriptor()?;
    on_bound_peer(peer, Vec::new());
    receive_with_auth_retries(bound_endpoint, output_dir, config, pairing, events, cancel).await
}

/// Receives one authenticated single-file or Manifest transfer over the
/// existing mDNS discovery path. Failed pairing attempts are ignored using the
/// same bounded retry policy as [`receive_file_enable_mdns`].
pub async fn receive_transfer_enable_mdns<F>(
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    on_bound_peer: F,
    cancel: TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError>
where
    F: FnOnce(PeerDescriptor, Vec<String>) + Send,
{
    let bound_endpoint = bind_iroh_transfer_endpoint_enable_mdns(
        listen_addrs,
        &config.identity,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let peer = bound_endpoint.peer_descriptor()?;
    on_bound_peer(peer, Vec::new());
    receive_transfer_with_auth_retries(bound_endpoint, output_dir, config, pairing, events, cancel)
        .await
}

/// Receives one file on an already-bound endpoint, stopping on cancellation.
pub async fn receive_one_authenticated(
    bound_endpoint: BoundEndpoint,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let events: Arc<dyn EventSink> = Arc::from(events);
    let mut connection = match accept_or_cancel(&bound_endpoint, &cancel, events.clone()).await {
        Ok(connection) => connection,
        Err(error) => {
            bound_endpoint.local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);

    if let Err(error) = auth_bounded(authenticate_receiver(&mut connection, pairing), &cancel).await
    {
        let _ = connection.close().await;
        bound_endpoint.local_endpoint.close().await;
        return Err(error);
    }
    let result = engine
        .receive_file_with_cancel(&mut connection, output_dir, events.as_ref(), &cancel)
        .await;
    close_after_receive(&mut connection, &result).await;
    bound_endpoint.local_endpoint.close().await;
    result
}

/// Receives one authenticated transfer on an already-bound dual-protocol
/// endpoint and routes it by the exact negotiated ALPN.
pub async fn receive_one_authenticated_transfer(
    bound_endpoint: BoundEndpoint,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError> {
    let events: Arc<dyn SessionEventSink> = Arc::from(events);
    let transfer_events: Arc<dyn EventSink> = events.clone();
    let mut connection =
        match accept_or_cancel(&bound_endpoint, &cancel, transfer_events.clone()).await {
            Ok(connection) => connection,
            Err(error) => {
                bound_endpoint.local_endpoint.close().await;
                return Err(error);
            }
        };
    connection.watch_path(transfer_events);

    if let Err(error) = auth_bounded(authenticate_receiver(&mut connection, pairing), &cancel).await
    {
        let _ = connection.close().await;
        bound_endpoint.local_endpoint.close().await;
        return Err(error);
    }
    let result = receive_negotiated_transfer(
        &mut connection,
        output_dir,
        &config,
        events.as_ref(),
        &cancel,
    )
    .await;
    close_after_receive(&mut connection, &result).await;
    bound_endpoint.local_endpoint.close().await;
    result
}

/// Receives one file, ignoring failed pairing attempts until one peer
/// authenticates; stops on cancellation.
pub async fn receive_with_auth_retries(
    bound_endpoint: BoundEndpoint,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let events: Arc<dyn EventSink> = Arc::from(events);
    let mut connection =
        match accept_authenticated_with_retries(&bound_endpoint, pairing, &cancel, events.clone())
            .await
        {
            Ok(connection) => connection,
            Err(error) => {
                bound_endpoint.local_endpoint.close().await;
                return Err(error);
            }
        };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);
    let result = engine
        .receive_file_with_cancel(&mut connection, output_dir, events.as_ref(), &cancel)
        .await;
    close_after_receive(&mut connection, &result).await;
    bound_endpoint.local_endpoint.close().await;
    result
}

/// Receives one negotiated single-file or Manifest transfer on an already
/// bound dual-protocol endpoint, ignoring failed pairing attempts.
pub async fn receive_transfer_with_auth_retries(
    bound_endpoint: BoundEndpoint,
    output_dir: PathBuf,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError> {
    let events: Arc<dyn SessionEventSink> = Arc::from(events);
    let transfer_events: Arc<dyn EventSink> = events.clone();
    let mut connection = match accept_authenticated_with_retries(
        &bound_endpoint,
        pairing,
        &cancel,
        transfer_events.clone(),
    )
    .await
    {
        Ok(connection) => connection,
        Err(error) => {
            bound_endpoint.local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(transfer_events);
    let result = receive_negotiated_transfer(
        &mut connection,
        output_dir,
        &config,
        events.as_ref(),
        &cancel,
    )
    .await;
    close_after_receive(&mut connection, &result).await;
    bound_endpoint.local_endpoint.close().await;
    result
}

/// Close the data connection after a receive. On success the receiver sent the
/// last frame (`CompleteAck`), so it waits for the sender to close - closing
/// first would race that close against the sender reading the ack. On failure
/// it closes actively, since there is no ack in flight to protect.
async fn receive_negotiated_transfer(
    connection: &mut IrohFrameConnection,
    output_dir: PathBuf,
    config: &SessionConfig,
    events: &dyn SessionEventSink,
    cancel: &TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError> {
    match connection.protocol() {
        TransferProtocol::SingleFileV1 => TransferEngine::new(config.chunk_size)
            .receive_file_with_cancel(connection, output_dir, events, cancel)
            .await
            .map(SessionTransferSummary::SingleFile),
        TransferProtocol::ManifestV1 => ManifestTransferEngine::new(config.chunk_size)
            .receive_manifest_with_cancel(connection, output_dir, events, cancel)
            .await
            .map(SessionTransferSummary::Manifest),
    }
}

async fn close_after_receive<T>(
    connection: &mut IrohFrameConnection,
    result: &Result<T, SessionError>,
) {
    match result {
        Ok(_) => connection.await_peer_close().await,
        Err(_) => {
            let _ = connection.close().await;
        }
    }
}

async fn accept_authenticated_with_retries(
    bound_endpoint: &BoundEndpoint,
    pairing: &PairingConfig,
    cancel: &TransferCancelToken,
    events: Arc<dyn EventSink>,
) -> Result<IrohFrameConnection, SessionError> {
    let mut failures = 0_u32;
    loop {
        let mut connection = accept_or_cancel(bound_endpoint, cancel, events.clone()).await?;
        match auth_bounded(authenticate_receiver(&mut connection, pairing), cancel).await {
            Ok(()) => return Ok(connection),
            Err(_) => {
                let _ = connection.close().await;
                failures += 1;
                if failures >= MAX_AUTH_FAILURES {
                    return Err(CoreError::Protocol(format!(
                        "too many failed pairing attempts (threshold: {MAX_AUTH_FAILURES})"
                    )));
                }
            }
        }
    }
}

async fn dial(
    local_endpoint: Endpoint,
    peer: &PeerDescriptor,
) -> Result<IrohFrameConnection, SessionError> {
    let peer_addr = peer_addr_from_descriptor(peer)?;
    dial_peer_addr(local_endpoint, peer_addr).await
}

async fn dial_peer_addr(
    local_endpoint: Endpoint,
    peer_addr: EndpointAddr,
) -> Result<IrohFrameConnection, SessionError> {
    dial_peer_addr_for_protocol(local_endpoint, peer_addr, TransferProtocol::SingleFileV1).await
}

async fn dial_peer_addr_for_protocol(
    local_endpoint: Endpoint,
    peer_addr: EndpointAddr,
    protocol: TransferProtocol,
) -> Result<IrohFrameConnection, SessionError> {
    let connection = local_endpoint
        .connect(peer_addr, protocol.alpn())
        .await
        .map_err(|error| connect_error(protocol, error))?;
    let negotiated = TransferProtocol::from_alpn(connection.alpn()).ok_or_else(|| {
        CoreError::Protocol(format!(
            "unsupported negotiated ALPN {:?}",
            String::from_utf8_lossy(connection.alpn())
        ))
    })?;
    if negotiated != protocol {
        connection.close(iroh::endpoint::VarInt::from_u32(0), b"alpn mismatch");
        return Err(CoreError::Protocol(format!(
            "requested ALPN {:?} but negotiated {:?}",
            String::from_utf8_lossy(protocol.alpn()),
            String::from_utf8_lossy(negotiated.alpn())
        )));
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

fn connect_error(protocol: TransferProtocol, error: iroh::endpoint::ConnectError) -> SessionError {
    if protocol == TransferProtocol::ManifestV1 && is_no_application_protocol(&error) {
        CoreError::Protocol(format!(
            "{MANIFEST_UNSUPPORTED_PEER_CODE}: peer does not support {}",
            String::from_utf8_lossy(protocol.alpn())
        ))
    } else {
        CoreError::Transport(error.to_string())
    }
}

fn is_no_application_protocol(error: &iroh::endpoint::ConnectError) -> bool {
    const TLS_ALERT_NO_APPLICATION_PROTOCOL: u8 = 120;

    fn is_tls_alert(error: &iroh::endpoint::ConnectionError) -> bool {
        matches!(
            error,
            iroh::endpoint::ConnectionError::ConnectionClosed(close)
                if close.error_code
                    == iroh::endpoint::TransportErrorCode::crypto(
                        TLS_ALERT_NO_APPLICATION_PROTOCOL,
                    )
        )
    }

    match error {
        iroh::endpoint::ConnectError::Connection { source, .. } => is_tls_alert(source),
        iroh::endpoint::ConnectError::Connecting { source, .. } => matches!(
            source,
            iroh::endpoint::ConnectingError::ConnectionError {
                source,
                ..
            } if is_tls_alert(source)
        ),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_file_to_peer_addr(
    local_endpoint: Endpoint,
    peer_addr: EndpointAddr,
    file_path: PathBuf,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    connect_timeout: Duration,
) -> Result<TransferSummary, SessionError> {
    events.on_event(TransferEvent::Connecting);
    // Bound the dial so a stale/unreachable mDNS candidate fails fast instead of
    // hanging until the full transport timeout; the transfer itself is unbounded.
    let mut connection = match tokio::time::timeout(
        connect_timeout,
        dial_peer_addr(local_endpoint, peer_addr),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Err(CoreError::Transport(format!(
                "connect to peer timed out after {}s",
                connect_timeout.as_secs()
            )));
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);
    if let Err(error) = auth_bounded(authenticate_sender(&mut connection, pairing), cancel).await {
        let _ = connection.close().await;
        return Err(error);
    }
    let result = engine
        .send_file_with_cancel(&mut connection, file_path, resume, events.as_ref(), cancel)
        .await;
    let _ = connection.close().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn send_manifest_to_peer_addr(
    local_endpoint: Endpoint,
    peer_addr: EndpointAddr,
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    pairing: &PairingConfig,
    events: Arc<dyn SessionEventSink>,
    cancel: &TransferCancelToken,
    connect_timeout: Option<Duration>,
) -> Result<ManifestTransferSummary, SessionError> {
    events.on_event(TransferEvent::Connecting);
    let dial = dial_peer_addr_for_protocol(local_endpoint, peer_addr, TransferProtocol::ManifestV1);
    let mut connection = match connect_timeout {
        Some(connect_timeout) => match tokio::time::timeout(connect_timeout, dial).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(CoreError::Transport(format!(
                    "connect to peer timed out after {}s",
                    connect_timeout.as_secs()
                )));
            }
        },
        None => dial.await?,
    };
    let transfer_events: Arc<dyn EventSink> = events.clone();
    connection.watch_path(transfer_events);
    if let Err(error) = auth_bounded(authenticate_sender(&mut connection, pairing), cancel).await {
        let _ = connection.close().await;
        return Err(error);
    }
    let result = ManifestTransferEngine::new(config.chunk_size)
        .send_manifest_with_cancel(&mut connection, request, resume, events.as_ref(), cancel)
        .await;
    let _ = connection.close().await;
    result
}

async fn accept_or_cancel(
    bound_endpoint: &BoundEndpoint,
    cancel: &TransferCancelToken,
    events: Arc<dyn EventSink>,
) -> Result<IrohFrameConnection, SessionError> {
    tokio::select! {
        result = bound_endpoint.accept_with_events(events.as_ref()) => result,
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    }
}

/// One auth handshake, bounded by [`AUTH_TIMEOUT`] and the cancel token. Auth
/// is strictly pre-transfer, so interrupting it is always safe.
async fn auth_bounded(
    auth: impl Future<Output = Result<(), SessionError>>,
    cancel: &TransferCancelToken,
) -> Result<(), SessionError> {
    tokio::select! {
        result = tokio::time::timeout(AUTH_TIMEOUT, auth) => match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Protocol("authentication timed out".into())),
        },
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    }
}

fn interrupted_error(cancel: &TransferCancelToken) -> SessionError {
    let message = if cancel.is_pause() {
        USER_PAUSE_MESSAGE
    } else {
        USER_INTERRUPT_MESSAGE
    };
    CoreError::Transfer(message.into())
}

#[cfg(test)]
mod auth_bound_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn wedged_auth_times_out() {
        let cancel = TransferCancelToken::new();
        let result = auth_bounded(std::future::pending(), &cancel).await;
        assert!(matches!(result, Err(CoreError::Protocol(m)) if m.contains("timed out")));
    }

    #[tokio::test]
    async fn cancel_interrupts_a_pending_auth() {
        let cancel = TransferCancelToken::new();
        cancel.pause();
        let result = auth_bounded(std::future::pending(), &cancel).await;
        assert!(matches!(result, Err(CoreError::Transfer(m)) if m == USER_PAUSE_MESSAGE));
    }

    #[tokio::test]
    async fn a_finishing_auth_passes_through() {
        let cancel = TransferCancelToken::new();
        assert!(auth_bounded(async { Ok(()) }, &cancel).await.is_ok());
    }
}
