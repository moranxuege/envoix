//! iroh transport for the room rendezvous: an iroh endpoint accepts pairing
//! connections, wraps each as a [`PeerConn`], and serves it through the
//! [`RoomRegistry`]. Clients reach the broker by its (hard-coded) endpoint id.
//!
//! The broker crate (`envoix-rendezvous`) stays transport-agnostic; this is the
//! only place that knows about iroh.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::dns::DnsResolver;
use iroh::endpoint::{Connection, Incoming, RecvStream, RelayMode, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayUrl, SecretKey, TransportAddr};

use tracing::Instrument;

pub use envoix_invite::{
    BootstrapKind, Commitment, InvitationControlContext, InvitationSide, TransferRole,
};
use envoix_rendezvous::{
    CloseWaiter, Join, PeerConn, Reply, Role, RoomRegistry, read_framed, write_framed,
};
use serde::{Deserialize, Serialize};

mod code;
pub use code::{generate_code, split_code};

/// AEAD associated data binding a sealed descriptor to the sender's role, so a
/// relay cannot reflect one peer's ciphertext back as the other's.
const JOINER_CONTEXT_AAD: &[u8] = b"envoix-invite control context joiner v2";
const CREATOR_CONTEXT_AAD: &[u8] = b"envoix-invite control context creator v2";
const JOINER_DESCRIPTOR_AAD: &[u8] = b"envoix-invite endpoint descriptor joiner v2";
const CREATOR_DESCRIPTOR_AAD: &[u8] = b"envoix-invite endpoint descriptor creator v2";

/// Cap on the post-exchange graceful close, so a misbehaving peer or broker
/// cannot hang the pairing after the descriptors are already exchanged.
const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Outcome of a successful room pairing.
pub struct RoomPairing<T> {
    /// The peer's payload (for Envoix, its iroh `PeerDescriptor` to dial).
    pub peer: T,
    /// Sealed public context sent by the invitation creator.
    pub peer_public_context: Option<Vec<u8>>,
    pub selected_bootstrap_method: BootstrapKind,
    control_key: Vec<u8>,
    /// Hash of the authenticated PAKE and both sealed control bundles.
    pub control_transcript_hash: Commitment,
}

impl<T> RoomPairing<T> {
    /// Control key for immediate Room-path data-password derivation.
    pub fn control_key(&self) -> &[u8] {
        &self.control_key
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContextBundle {
    public_context: Option<Vec<u8>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DescriptorBundle<T> {
    peer: Option<T>,
}

/// Lets the broker wait for an iroh peer to close before dropping the relay.
struct IrohClose(Connection);

impl CloseWaiter for IrohClose {
    fn wait_closed(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            self.0.closed().await;
        })
    }
}

/// ALPN for the rendezvous protocol (distinct from the data-plane `envoix/1`).
pub const RENDEZVOUS_ALPN: &[u8] = b"envoix-rendezvous/1";

/// Reason string the broker signals (and [`join_invitation`] returns) when a room's
/// wait window elapses with no partner - distinct from a network failure.
pub const ROOM_EXPIRED: &str = "no peer joined the room within the wait window";

/// Optionally annotates a peer IP with a human location/ISP string for logs
/// (e.g. a GeoIP lookup). Supplied by the operator binary so this crate needs
/// no geo-database dependency.
pub type PeerLocator = Arc<dyn Fn(IpAddr) -> Option<String> + Send + Sync>;

/// Bind an iroh endpoint that speaks the rendezvous ALPN. Pass
/// [`RelayMode::Disabled`] for LAN/direct, or a custom relay mode (see
/// [`relay_mode_from_url`]) for WAN reachability through a relay.
pub async fn build_endpoint(
    bind: SocketAddr,
    secret_key: SecretKey,
    relay: RelayMode,
) -> Result<Endpoint> {
    build_endpoint_with_dns(bind, secret_key, relay, None).await
}

/// Same as [`build_endpoint`], with an optional resolver for relay hostnames.
/// Native clients can supply their platform DNS resolver without changing the
/// rendezvous transport's direct-only behavior.
pub async fn build_endpoint_with_dns(
    bind: SocketAddr,
    secret_key: SecretKey,
    relay: RelayMode,
    dns_resolver: Option<DnsResolver>,
) -> Result<Endpoint> {
    let builder = Endpoint::builder(presets::N0);
    // Defined by build.rs only when the NAT harness supplies its generated CA.
    #[cfg(envoix_nat_test_local_ca)]
    let builder = builder.ca_tls_config(
        iroh::tls::CaTlsConfig::embedded().with_extra_roots([include_bytes!(env!(
            "ENVOIX_NAT_TEST_CA_DER_PATH"
        ))
        .to_vec()
        .into()]),
    );
    let mut builder = builder
        .secret_key(secret_key)
        .relay_mode(relay)
        .clear_address_lookup()
        .alpns(vec![RENDEZVOUS_ALPN.to_vec()]);
    if let Some(dns_resolver) = dns_resolver {
        builder = builder.dns_resolver(dns_resolver);
    }
    builder
        .clear_ip_transports()
        .bind_addr(bind)
        .context("invalid bind address")?
        .bind()
        .await
        .context("failed to bind iroh endpoint")
}

/// Build a [`RelayMode`] from an optional relay URL: `None` disables relays
/// (LAN/direct only); `Some(url)` routes through that single custom relay so
/// peers behind NAT can reach the broker and each other.
pub fn relay_mode_from_url(url: Option<&str>) -> Result<RelayMode> {
    match url {
        None => Ok(RelayMode::Disabled),
        Some(url) => {
            let url: RelayUrl = url.parse().context("invalid relay url")?;
            Ok(RelayMode::Custom(RelayMap::from(url)))
        }
    }
}

/// The endpoint's connectable address (id + direct socket addresses).
pub fn endpoint_addr(endpoint: &Endpoint) -> EndpointAddr {
    EndpointAddr::from_parts(
        endpoint.id(),
        endpoint.addr().ip_addrs().copied().map(TransportAddr::Ip),
    )
}

/// Cap on connections served at once, so a flood cannot exhaust the broker.
const MAX_CONCURRENT_CONNECTIONS: usize = 256;

/// Cap on a fresh connection's handshake + pairing-stream open before Join, so a
/// half-open idle connection cannot hold a connection slot indefinitely.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Accept pairing connections forever, serving each through `registry`, up to
/// MAX_CONCURRENT_CONNECTIONS at a time (excess incoming connections are dropped).
pub async fn serve_endpoint(
    endpoint: Endpoint,
    registry: Arc<RoomRegistry>,
    locate: Option<PeerLocator>,
) -> Result<()> {
    let limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    while let Some(incoming) = endpoint.accept().await {
        let Ok(permit) = limit.clone().try_acquire_owned() else {
            tracing::warn!("rendezvous connection limit reached; dropping incoming");
            continue;
        };
        let registry = registry.clone();
        let locate = locate.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = serve_incoming(incoming, &registry, locate.as_ref()).await {
                tracing::debug!(%error, "rendezvous connection ended");
            }
        });
    }
    Ok(())
}

async fn serve_incoming(
    incoming: Incoming,
    registry: &RoomRegistry,
    locate: Option<&PeerLocator>,
) -> Result<()> {
    // Bound the pre-Join setup so a connection that half-opens and then goes idle
    // cannot pin a connection slot; the registry separately bounds the first
    // control-frame read.
    let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming)
        .await
        .context("rendezvous connection handshake timed out")??;
    let (send, recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .context("rendezvous pairing stream not opened in time")??;
    // Correlation span for this connection. `room` is filled in by the broker
    // once it reads the Join; `peer`/`geo` are filled in by the task below once
    // the direct path settles (a NATed peer reaches even a public broker over
    // the relay first, and only punches direct ~seconds later - so the reflexive
    // address is not known at accept time).
    let span = tracing::info_span!(
        "conn",
        room = tracing::field::Empty,
        peer = tracing::field::Empty,
        geo = tracing::field::Empty,
    );
    spawn_peer_locator(connection.clone(), locate.cloned(), span.clone());
    // The Connection is the close-waiter: the broker keeps it open until the
    // peer closes, then drops it.
    let conn = PeerConn::new(send, recv, IrohClose(connection));
    registry.serve(conn).instrument(span).await?;
    Ok(())
}

/// Interval and cap for waiting on the peer's direct path to settle.
const PEER_LOCATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
const PEER_LOCATE_ATTEMPTS: usize = 40;

/// Watch `connection` until its direct (reflexive) address appears, then record
/// it - annotated via `locate` when set - onto `span` and emit one `peer
/// located` line. Gives up quietly if the connection closes first or never
/// punches direct (a purely relay-reached peer).
fn spawn_peer_locator(connection: Connection, locate: Option<PeerLocator>, span: tracing::Span) {
    tokio::spawn(async move {
        for _ in 0..PEER_LOCATE_ATTEMPTS {
            if let Some(addr) = observed_addr(&connection) {
                let geo = locate.as_ref().and_then(|locate| locate(addr.ip()));
                span.record("peer", tracing::field::display(&addr));
                if let Some(geo) = &geo {
                    span.record("geo", tracing::field::display(geo));
                }
                span.in_scope(|| {
                    tracing::info!(
                        peer = %addr,
                        geo = geo.as_deref().unwrap_or(""),
                        "peer located"
                    );
                });
                return;
            }
            tokio::time::sleep(PEER_LOCATE_INTERVAL).await;
        }
    });
}

/// The peer's observed direct socket address (its post-NAT reflexive address),
/// or `None` if only a relay path is known so far. We take the first direct
/// (`Ip`) path rather than only the *selected* one, because path selection has
/// not necessarily settled at accept time - a peer dialing the broker's direct
/// address is already reachable there even before iroh promotes it to selected.
/// We never log a relay's address as if it were the peer's.
fn observed_addr(connection: &Connection) -> Option<SocketAddr> {
    connection
        .paths()
        .iter()
        .find_map(|path| match path.remote_addr() {
            TransportAddr::Ip(addr) => Some(*addr),
            _ => None,
        })
}

/// A peer's live session with the broker after joining a room. The caller drives
/// the end-to-end pairing over `send`/`recv`; `connection` keeps the streams
/// alive and must be held for the duration.
pub struct BrokerSession {
    pub connection: Connection,
    pub send: SendStream,
    pub recv: RecvStream,
    pub role: Role,
    pub selected_bootstrap_method: BootstrapKind,
}

/// Join the strict directional rendezvous protocol.
pub async fn join_invitation(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    join: Join,
) -> Result<BrokerSession> {
    let connection = endpoint.connect(broker, RENDEZVOUS_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    write_framed(&mut send, &join).await?;
    let reply: Reply = read_framed(&mut recv).await?;
    let paired = match reply {
        Reply::Paired(paired) => paired,
        Reply::Expired => anyhow::bail!(ROOM_EXPIRED),
    };
    Ok(BrokerSession {
        connection,
        send,
        recv,
        role: paired.role,
        selected_bootstrap_method: paired.selected_bootstrap_method,
    })
}

/// Authenticated control channel after the PAKE and sealed public-context
/// delivery, but before either side creates or discloses a data endpoint.
pub struct AuthenticatedControl {
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    role: Role,
    pub selected_bootstrap_method: BootstrapKind,
    control_key: Vec<u8>,
    pake_transcript_hash: Commitment,
    pub peer_public_context: Option<Vec<u8>>,
    my_context_sealed: Vec<u8>,
    peer_context_sealed: Vec<u8>,
}

/// Authenticate the selected invitation bootstrap and deliver sealed public
/// context. Endpoint descriptors are exchanged separately only after callers
/// validate this result.
pub async fn authenticate_invitation(
    session: BrokerSession,
    password: &str,
    context: &InvitationControlContext,
    public_context: Option<&[u8]>,
) -> Result<AuthenticatedControl> {
    use envoix_pairing::{
        Confirm, PakeResponse, PakeStart, initiator_start, open_json, responder_respond, seal_json,
    };

    let BrokerSession {
        connection,
        mut send,
        mut recv,
        role,
        selected_bootstrap_method,
    } = session;
    if selected_bootstrap_method != context.selected_bootstrap_method {
        anyhow::bail!("broker selected a different invitation bootstrap method");
    }

    let key = match role {
        Role::Initiator => {
            let (pending, start) = initiator_start(password, context)?;
            write_framed(&mut send, &start).await?;
            let response: PakeResponse = read_framed(&mut recv).await?;
            let (confirming, confirm) = pending.finish(&response)?;
            write_framed(&mut send, &confirm).await?;
            let responder_confirm: Confirm = read_framed(&mut recv).await?;
            confirming.verify(&responder_confirm)?
        }
        Role::Responder => {
            let start: PakeStart = read_framed(&mut recv).await?;
            let (confirming, response) = responder_respond(password, context, &start)?;
            write_framed(&mut send, &response).await?;
            let initiator_confirm: Confirm = read_framed(&mut recv).await?;
            let (key, confirm) = confirming.verify(&initiator_confirm)?;
            write_framed(&mut send, &confirm).await?;
            key
        }
    };

    // Bind each sealed context to invitation side, so reflection fails.
    let (my_aad, peer_aad): (&[u8], &[u8]) = match role {
        Role::Initiator => (JOINER_CONTEXT_AAD, CREATOR_CONTEXT_AAD),
        Role::Responder => (CREATOR_CONTEXT_AAD, JOINER_CONTEXT_AAD),
    };
    let my_bundle = ContextBundle {
        public_context: public_context.map(<[u8]>::to_vec),
    };
    let my_sealed = seal_json(key.key(), my_aad, &my_bundle)?;
    write_framed(&mut send, &my_sealed).await?;
    let peer_sealed: Vec<u8> = read_framed(&mut recv).await?;
    let peer_bundle: ContextBundle = open_json(key.key(), peer_aad, &peer_sealed)?;

    Ok(AuthenticatedControl {
        connection,
        send,
        recv,
        role,
        selected_bootstrap_method,
        control_key: key.key().to_vec(),
        pake_transcript_hash: key.transcript_hash(),
        peer_public_context: peer_bundle.public_context,
        my_context_sealed: my_sealed,
        peer_context_sealed: peer_sealed,
    })
}

impl AuthenticatedControl {
    /// Exchange optional endpoint descriptors after the caller has validated
    /// the sealed invitation context.
    pub async fn exchange_descriptor<T>(
        mut self,
        mine: Option<&T>,
    ) -> Result<RoomPairing<Option<T>>>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        use envoix_pairing::{open_json, seal_json};

        let (my_aad, peer_aad): (&[u8], &[u8]) = match self.role {
            Role::Initiator => (JOINER_DESCRIPTOR_AAD, CREATOR_DESCRIPTOR_AAD),
            Role::Responder => (CREATOR_DESCRIPTOR_AAD, JOINER_DESCRIPTOR_AAD),
        };
        let my_sealed = seal_json(&self.control_key, my_aad, &DescriptorBundle { peer: mine })?;
        write_framed(&mut self.send, &my_sealed).await?;
        let peer_sealed: Vec<u8> = read_framed(&mut self.recv).await?;
        let peer_bundle: DescriptorBundle<T> =
            open_json(&self.control_key, peer_aad, &peer_sealed)?;
        let control_transcript_hash = complete_control_transcript_hash(
            self.pake_transcript_hash,
            self.role,
            &self.my_context_sealed,
            &self.peer_context_sealed,
            &my_sealed,
            &peer_sealed,
        );

        let _ = self.send.finish();
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
            let _ = self.send.stopped().await;
            let _ = self.recv.read_to_end(1024).await;
        })
        .await;
        drop(self.connection);

        Ok(RoomPairing {
            peer: peer_bundle.peer,
            peer_public_context: self.peer_public_context,
            selected_bootstrap_method: self.selected_bootstrap_method,
            control_key: self.control_key,
            control_transcript_hash,
        })
    }
}

/// Convenience wrapper for callers which already have a descriptor before
/// authenticating. Transfer sessions use the split API so validation precedes
/// data-endpoint creation.
pub async fn drive_pairing<T>(
    session: BrokerSession,
    password: &str,
    context: &InvitationControlContext,
    mine: &T,
    public_context: Option<&[u8]>,
) -> Result<RoomPairing<T>>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let pairing = authenticate_invitation(session, password, context, public_context)
        .await?
        .exchange_descriptor(Some(mine))
        .await?;
    Ok(RoomPairing {
        peer: pairing
            .peer
            .ok_or_else(|| anyhow::anyhow!("peer omitted its endpoint descriptor"))?,
        peer_public_context: pairing.peer_public_context,
        selected_bootstrap_method: pairing.selected_bootstrap_method,
        control_key: pairing.control_key,
        control_transcript_hash: pairing.control_transcript_hash,
    })
}

fn complete_control_transcript_hash(
    pake_transcript_hash: Commitment,
    role: Role,
    my_context_sealed: &[u8],
    peer_context_sealed: &[u8],
    my_descriptor_sealed: &[u8],
    peer_descriptor_sealed: &[u8],
) -> Commitment {
    let (creator_context, joiner_context, creator_descriptor, joiner_descriptor) = match role {
        Role::Initiator => (
            peer_context_sealed,
            my_context_sealed,
            peer_descriptor_sealed,
            my_descriptor_sealed,
        ),
        Role::Responder => (
            my_context_sealed,
            peer_context_sealed,
            my_descriptor_sealed,
            peer_descriptor_sealed,
        ),
    };
    let mut transcript = Vec::new();
    for value in [
        pake_transcript_hash.as_bytes().as_slice(),
        creator_context,
        joiner_context,
        creator_descriptor,
        joiner_descriptor,
    ] {
        transcript.extend_from_slice(&(value.len() as u64).to_be_bytes());
        transcript.extend_from_slice(value);
    }
    Commitment::sha256(&transcript)
}
