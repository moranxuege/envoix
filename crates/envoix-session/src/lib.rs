//! Session orchestration for transfer setup and concrete iroh wiring.

mod candidates;
mod connection;
mod endpoint;
mod identity;
mod room;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use envoix_auth::{PairingConfig, authenticate_receiver, authenticate_sender};
use envoix_error::CoreError;
use envoix_protocol::{FrameConnection, PeerDescriptor};
pub use envoix_rendezvous_iroh::{generate_code, split_code};
pub use envoix_transfer::TransferEngine;
pub use envoix_transfer::{
    DEFAULT_CHUNK_SIZE, EventSink, NoopEventSink, PEER_INTERRUPT_MESSAGE, PEER_PAUSE_MESSAGE,
    TransferCancelToken, TransferEvent, TransferSummary, USER_INTERRUPT_MESSAGE,
    USER_PAUSE_MESSAGE, validate_chunk_size,
};
pub use envoix_types::TransferDirection;
use iroh::Endpoint;
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;

pub use candidates::CandidateFilter;
use connection::IrohFrameConnection;
pub use endpoint::{BindAddrs, BoundEndpoint, parse_broker_addr};
use endpoint::{
    build_accept_endpoint, build_advertising_accept_endpoint, build_dial_endpoint,
    peer_addr_from_descriptor,
};
pub use identity::IdentityConfig;
pub use iroh::EndpointAddr;
pub use room::{receive_file_via_room, send_file_via_room};

const ALPN: &[u8] = b"envoix/1";
const MAX_AUTH_FAILURES: u32 = 50;
const MDNS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-candidate connect budget for mDNS sends, so stale candidates fail fast.
const MDNS_CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// Error type returned by session orchestration.
pub type SessionError = CoreError;

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
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_accept_endpoint(
            listen_addrs.into(),
            identity,
            relay,
            relay_only,
            candidates,
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
) -> Result<BoundEndpoint, SessionError> {
    Ok(BoundEndpoint {
        local_endpoint: build_advertising_accept_endpoint(
            listen_addrs.into(),
            identity,
            &None,
            false,
            candidates,
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

    if let Err(error) = authenticate_sender(&mut connection, pairing).await {
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
    )
    .await?;
    let events: Arc<dyn EventSink> = Arc::from(events);
    events.on_event(TransferEvent::Diagnostic {
        message: format!("dial start {}", endpoint_addr_shape(&peer_addr)),
    });
    events.on_event(TransferEvent::Connecting);
    let mut connection = match dial_peer_addr(local_endpoint.clone(), peer_addr).await {
        Ok(connection) => connection,
        Err(error) => {
            events.on_event(TransferEvent::Diagnostic {
                message: format!("dial failed: {error}"),
            });
            local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);
    if let Err(error) = authenticate_sender(&mut connection, pairing).await {
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
    )
    .await?;
    let mdns = MdnsAddressLookup::builder()
        .advertise(false)
        .build(local_endpoint.id())
        .map_err(|error| CoreError::Discovery(error.to_string()))?;
    local_endpoint
        .address_lookup()
        .map_err(|error| CoreError::Discovery(error.to_string()))?
        .add(mdns.clone());

    let mut discoveries = mdns.subscribe().await;
    let deadline = tokio::time::Instant::now() + MDNS_DISCOVERY_TIMEOUT;
    let mut last_error = None;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }

        let event = tokio::select! {
            result = tokio::time::timeout_at(deadline, discoveries.next()) => {
                result.map_err(|_| {
                    CoreError::Discovery(format!(
                        "no iroh mDNS peers discovered within {} seconds",
                        MDNS_DISCOVERY_TIMEOUT.as_secs()
                    ))
                })?
            }
            () = cancel.cancelled() => {
                local_endpoint.close().await;
                return Err(interrupted_error(&cancel));
            }
        };

        let Some(event) = event else {
            break;
        };

        let DiscoveryEvent::Discovered {
            endpoint_info: discovered_peer,
            ..
        } = event
        else {
            continue;
        };
        if discovered_peer.endpoint_id == local_endpoint.id() {
            continue;
        }
        let peer_addr = discovered_peer.to_endpoint_addr();

        match send_file_to_peer_addr(
            local_endpoint.clone(),
            peer_addr,
            file_path.clone(),
            resume,
            config.clone(),
            pairing,
            events.clone(),
            &cancel,
        )
        .await
        {
            Ok(summary) => {
                local_endpoint.close().await;
                return Ok(summary);
            }
            Err(error) => {
                events.on_event(TransferEvent::Failed {
                    direction: TransferDirection::Send,
                    reason: error.to_string(),
                });
                if cancel.is_cancelled() {
                    local_endpoint.close().await;
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
    }

    local_endpoint.close().await;
    Err(last_error.unwrap_or_else(|| {
        CoreError::Discovery(format!(
            "no iroh mDNS peers discovered within {} seconds",
            MDNS_DISCOVERY_TIMEOUT.as_secs()
        ))
    }))
}

/// Receives one file, reporting the concrete bound peer descriptor and relay
/// URLs before accepting; stops while waiting or transferring if cancelled.
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

/// Receives one file over an mDNS-advertised endpoint: binds an mDNS endpoint,
/// reports the bound peer descriptor through `on_bound_peer`, then accepts the
/// first dialer that authenticates. Stops on cancellation.
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
    let bound_endpoint =
        bind_iroh_endpoint_enable_mdns(listen_addrs, &config.identity, &config.candidates).await?;
    let peer = bound_endpoint.peer_descriptor()?;
    on_bound_peer(peer, Vec::new());
    receive_with_auth_retries(bound_endpoint, output_dir, config, pairing, events, cancel).await
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
    let mut connection = match accept_or_cancel(&bound_endpoint, &cancel, events.as_ref()).await {
        Ok(connection) => connection,
        Err(error) => {
            bound_endpoint.local_endpoint.close().await;
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);

    if let Err(error) = authenticate_receiver(&mut connection, pairing).await {
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
        match accept_authenticated_with_retries(&bound_endpoint, pairing, &cancel, events.as_ref())
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

/// Close the data connection after a receive. On success the receiver sent the
/// last frame (`CompleteAck`), so it waits for the sender to close - closing
/// first would race that close against the sender reading the ack. On failure
/// it closes actively, since there is no ack in flight to protect.
async fn close_after_receive(
    connection: &mut IrohFrameConnection,
    result: &Result<TransferSummary, SessionError>,
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
    events: &dyn EventSink,
) -> Result<IrohFrameConnection, SessionError> {
    let mut failures = 0_u32;
    loop {
        let mut connection = accept_or_cancel(bound_endpoint, cancel, events).await?;
        match authenticate_receiver(&mut connection, pairing).await {
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
    let connection = local_endpoint
        .connect(peer_addr, ALPN)
        .await
        .map_err(|error| CoreError::Transport(error.to_string()))?;
    let (send, recv) = connection
        .open_bi()
        .await
        .map_err(|error| CoreError::Transport(error.to_string()))?;
    Ok(IrohFrameConnection::new(
        local_endpoint,
        connection,
        send,
        recv,
    ))
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
) -> Result<TransferSummary, SessionError> {
    events.on_event(TransferEvent::Diagnostic {
        message: format!("dial start {}", endpoint_addr_shape(&peer_addr)),
    });
    events.on_event(TransferEvent::Connecting);
    let mut connection = match tokio::time::timeout(
        MDNS_CONNECT_TIMEOUT,
        dial_peer_addr(local_endpoint, peer_addr),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            events.on_event(TransferEvent::Diagnostic {
                message: format!("dial failed: {error}"),
            });
            return Err(error);
        }
        Err(_) => {
            let error = CoreError::Transport(format!(
                "connect to peer timed out after {}s",
                MDNS_CONNECT_TIMEOUT.as_secs()
            ));
            events.on_event(TransferEvent::Diagnostic {
                message: format!("dial failed: {error}"),
            });
            return Err(error);
        }
    };
    connection.watch_path(events.clone());
    let engine = TransferEngine::new(config.chunk_size);
    if let Err(error) = authenticate_sender(&mut connection, pairing).await {
        let _ = connection.close().await;
        return Err(error);
    }
    let result = engine
        .send_file_with_cancel(&mut connection, file_path, resume, events.as_ref(), cancel)
        .await;
    let _ = connection.close().await;
    result
}

async fn accept_or_cancel(
    bound_endpoint: &BoundEndpoint,
    cancel: &TransferCancelToken,
    events: &dyn EventSink,
) -> Result<IrohFrameConnection, SessionError> {
    tokio::select! {
        result = bound_endpoint.accept_with_events(events) => result,
        () = cancel.cancelled() => Err(interrupted_error(cancel)),
    }
}

fn endpoint_addr_shape(addr: &EndpointAddr) -> String {
    format!(
        "endpoint={} direct={} relay={}",
        short_endpoint_id(&addr.id.to_string()),
        addr.ip_addrs().count(),
        addr.relay_urls().count()
    )
}

fn short_endpoint_id(id: &str) -> &str {
    let end = id.len().min(12);
    &id[..end]
}

fn interrupted_error(cancel: &TransferCancelToken) -> SessionError {
    let message = if cancel.is_pause() {
        USER_PAUSE_MESSAGE
    } else {
        USER_INTERRUPT_MESSAGE
    };
    CoreError::Transfer(message.into())
}
