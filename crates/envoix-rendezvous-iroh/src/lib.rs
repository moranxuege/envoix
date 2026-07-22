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

pub use envoix_rendezvous::JoinIntent;
use envoix_rendezvous::{
    CloseWaiter, Join, PeerConn, Reply, Role, RoomRegistry, read_framed, write_framed,
};

mod code;
pub use code::{generate_code, split_code};

/// BLAKE3 KDF context separating the data-plane token from any other use of K.
const DATA_TOKEN_CONTEXT: &str = "envoix rendezvous data-plane token v1";

/// AEAD associated data binding a sealed descriptor to the sender's role, so a
/// relay cannot reflect one peer's ciphertext back as the other's.
const INITIATOR_SEAL_AAD: &[u8] = b"envoix-pairing seal initiator v1";
const RESPONDER_SEAL_AAD: &[u8] = b"envoix-pairing seal responder v1";

/// Cap on the post-exchange graceful close, so a misbehaving peer or broker
/// cannot hang the pairing after the descriptors are already exchanged.
const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Outcome of a successful room pairing.
pub struct RoomPairing<T> {
    /// The peer's payload (for Envoix, its iroh `PeerDescriptor` to dial).
    pub peer: T,
    /// A strong shared token derived from the SPAKE2 key, so the existing
    /// data-plane pairing (`envoix-auth` SPAKE2 over the iroh connection) can
    /// run unchanged - both peers derive the same one.
    pub token: String,
    /// Six-digit SAS derived from the confirmed SPAKE2 key and handshake
    /// transcript. Both peers compute the same value independently; it is
    /// never sent over the wire. The UI displays it for user comparison.
    pub sas: Option<String>,
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

/// Reason string the broker signals (and `join_room` returns) when a room's
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
}

/// Connect to the broker, open the pairing stream, join `room_id`, and return
/// the streams + assigned role to drive the pairing over.
pub async fn join_room(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_id: &str,
) -> Result<BrokerSession> {
    join_room_with_intent(endpoint, broker, room_id, None).await
}

/// Intent-aware variant of [`join_room`]. New clients declare the transfer
/// direction so the broker cannot pair two senders or two receivers sharing a
/// room id. Passing `None` preserves legacy room-only matching.
pub async fn join_room_with_intent(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_id: &str,
    intent: Option<JoinIntent>,
) -> Result<BrokerSession> {
    let connection = endpoint.connect(broker, RENDEZVOUS_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    write_framed(
        &mut send,
        &Join {
            room_id: room_id.to_string(),
            intent,
        },
    )
    .await?;
    let reply: Reply = read_framed(&mut recv).await?;
    let role = match reply {
        Reply::Paired(paired) => paired.role,
        Reply::Expired => anyhow::bail!(ROOM_EXPIRED),
    };
    Ok(BrokerSession {
        connection,
        send,
        recv,
        role,
    })
}

/// Pair with a peer in `room_id` over the broker: run SPAKE2 with `password`,
/// then swap payloads sealed under the derived key. Returns the peer's payload
/// (for Envoix, each side passes its iroh `PeerDescriptor`, so the result is the
/// address to dial). The broker only relays ciphertext - it can neither read
/// nor forge the exchanged payload.
pub async fn pair_in_room<T>(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_id: &str,
    password: &str,
    mine: &T,
) -> Result<RoomPairing<T>>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let session = join_room(endpoint, broker, room_id).await?;
    drive_pairing(session, password, mine).await
}

/// Intent-aware variant of [`pair_in_room`].
pub async fn pair_in_room_with_intent<T>(
    endpoint: &Endpoint,
    broker: EndpointAddr,
    room_id: &str,
    password: &str,
    mine: &T,
    intent: JoinIntent,
) -> Result<RoomPairing<T>>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let session = join_room_with_intent(endpoint, broker, room_id, Some(intent)).await?;
    drive_pairing(session, password, mine).await
}

/// Drive the end-to-end pairing over an already-joined [`BrokerSession`]: run
/// SPAKE2 with `password`, then swap payloads sealed under the derived key.
/// Split from [`pair_in_room`] so a caller can time-box just this phase - with a
/// live partner it completes in milliseconds, so a stall means the broker
/// matched us with a stale/dead peer and the caller should re-join.
pub async fn drive_pairing<T>(
    session: BrokerSession,
    password: &str,
    mine: &T,
) -> Result<RoomPairing<T>>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    use envoix_pairing::{
        Confirm, PakeResponse, PakeStart, initiator_start, open_json, pairing_sas,
        responder_respond, seal_json,
    };

    let BrokerSession {
        connection,
        mut send,
        mut recv,
        role,
    } = session;

    let (key, start, response) = match role {
        Role::Initiator => {
            let (pending, start) = initiator_start(password)?;
            write_framed(&mut send, &start).await?;
            let response: PakeResponse = read_framed(&mut recv).await?;
            let (confirming, confirm) = pending.finish(&response)?;
            write_framed(&mut send, &confirm).await?;
            let responder_confirm: Confirm = read_framed(&mut recv).await?;
            let key = confirming.verify(&responder_confirm)?;
            (key, start, response)
        }
        Role::Responder => {
            let start: PakeStart = read_framed(&mut recv).await?;
            let (confirming, response) = responder_respond(password, &start)?;
            write_framed(&mut send, &response).await?;
            let initiator_confirm: Confirm = read_framed(&mut recv).await?;
            let (key, confirm) = confirming.verify(&initiator_confirm)?;
            write_framed(&mut send, &confirm).await?;
            (key, start, response)
        }
    };

    // Six-digit SAS derived from the confirmed key + transcript. Both peers
    // compute the same value independently; neither value is sent over the wire.
    let sas = pairing_sas(key.key(), &start, &response);

    // Bind each sealed descriptor to the sender's role (AEAD aad); we seal with
    // our role and open with the peer's, so a reflected ciphertext fails to open.
    let (my_aad, peer_aad): (&[u8], &[u8]) = match role {
        Role::Initiator => (INITIATOR_SEAL_AAD, RESPONDER_SEAL_AAD),
        Role::Responder => (RESPONDER_SEAL_AAD, INITIATOR_SEAL_AAD),
    };
    write_framed(&mut send, &seal_json(key.key(), my_aad, mine)?).await?;
    let sealed: Vec<u8> = read_framed(&mut recv).await?;
    let peer: T = open_json(key.key(), peer_aad, &sealed)?;

    // Derive a strong data-plane token from K (both peers get the same one).
    let token = hex(&blake3::derive_key(DATA_TOKEN_CONTEXT, key.key()));

    // Graceful close: finish + wait for the broker to ack our FIN (so it is
    // delivered through the relay), then drain our recv to EOF before dropping.
    // Bounded by CLOSE_TIMEOUT so a stalled peer cannot hang a done pairing.
    let _ = send.finish();
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        let _ = send.stopped().await;
        let _ = recv.read_to_end(1024).await;
    })
    .await;
    drop(connection);

    Ok(RoomPairing {
        peer,
        token,
        sas: Some(sas),
    })
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}
