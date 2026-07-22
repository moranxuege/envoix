//! End-to-end: a file transfers from sender to receiver after they pair in a
//! room over a loopback rendezvous broker, using only a short code.

use std::sync::Arc;
use std::time::Duration;

use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use envoix_session::{
    DEFAULT_CHUNK_SIZE, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig, NoopEventSink, SessionConfig,
    TransferCancelToken, receive_file_via_room, send_file_via_room,
};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use tempfile::tempdir;

async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..100 {
        if ep.addr().ip_addrs().next().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    endpoint_addr(ep)
}

/// A room-mode config: no pairing here - the room flow derives the token from
/// the SPAKE2 exchange during pairing.
fn config() -> SessionConfig {
    SessionConfig {
        chunk_size: DEFAULT_CHUNK_SIZE,
        identity: IdentityConfig::Ephemeral,
        relay: None,
        relay_only: false,
        direct_only: false,
        candidates: Default::default(),
        data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_transfers_through_the_rendezvous() {
    // Rendezvous broker.
    let server = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .unwrap();
    let broker = ready_addr(&server).await;
    tokio::spawn(serve_endpoint(server, Arc::new(RoomRegistry::new()), None));

    // A source file and an output directory.
    let dir = tempdir().unwrap();
    let src = dir.path().join("greeting.txt");
    let contents = b"hello through the room rendezvous";
    std::fs::write(&src, contents).unwrap();
    let out = dir.path().join("received");
    std::fs::create_dir(&out).unwrap();

    let code = "1234-amber-comet";
    let listen: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    let (broker_r, broker_s) = (broker.clone(), broker.clone());
    let out_dir = out.clone();
    let recv = tokio::spawn(async move {
        receive_file_via_room(
            broker_r,
            code,
            listen,
            out_dir,
            config(),
            Box::new(NoopEventSink),
            TransferCancelToken::new(),
        )
        .await
    });
    // Let the receiver bind + start pairing first.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let send = tokio::spawn(async move {
        send_file_via_room(
            broker_s,
            code,
            src,
            false,
            config(),
            Box::new(NoopEventSink),
            TransferCancelToken::new(),
        )
        .await
    });

    let join = Duration::from_secs(30);
    let sent = tokio::time::timeout(join, send)
        .await
        .expect("send timed out")
        .unwrap();
    let received = tokio::time::timeout(join, recv)
        .await
        .expect("recv timed out")
        .unwrap();
    sent.expect("sender ok");
    received.expect("receiver ok");

    // The file arrived intact under its original name.
    let got = std::fs::read(out.join("greeting.txt")).expect("received file");
    assert_eq!(got, contents);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_expiry_reports_no_peer_joined() {
    use std::time::Duration;

    // Broker with a short room TTL so the wait window elapses quickly.
    let server = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .unwrap();
    let broker = ready_addr(&server).await;
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(2)));
    tokio::spawn(serve_endpoint(server, registry, None));

    let dir = tempdir().unwrap();
    let out = dir.path().join("received");
    std::fs::create_dir(&out).unwrap();
    let listen: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    // Receive in a room no sender ever joins: it must fail with the friendly
    // "no peer joined" message, not a bare connection-lost error.
    let error = receive_file_via_room(
        broker,
        "9999-lonely-room",
        listen,
        out,
        config(),
        Box::new(NoopEventSink),
        TransferCancelToken::new(),
    )
    .await
    .unwrap_err();

    assert!(
        error.to_string().contains("no peer joined the room"),
        "expected the friendly expiry message, got: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn candidate_filter_scopes_the_advertised_descriptor() {
    use envoix_session::{CandidateFilter, bind_iroh_endpoint_enable_mdns};

    let listen = envoix_session::BindAddrs::dual_stack(0);
    let bound = bind_iroh_endpoint_enable_mdns(
        listen,
        &IdentityConfig::Ephemeral,
        &CandidateFilter::default(),
        DEFAULT_DATA_STREAM_WINDOW,
    )
    .await
    .unwrap();

    // Unfiltered: the endpoint has at least one direct address.
    let all = bound.direct_addrs();
    assert!(!all.is_empty(), "endpoint should have direct addrs");

    // Deny one of its addresses: the advertised set drops exactly that one,
    // proving the filter is applied where descriptors are built.
    let denied = all[0].ip();
    let filtered = bound
        .with_candidate_filter(CandidateFilter::from_lists(&[], &[denied.to_string()]).unwrap());
    let kept = filtered.direct_addrs();
    assert!(
        !kept.iter().any(|a| a.ip() == denied),
        "denied address must not be advertised"
    );
    assert!(
        kept.len() < all.len(),
        "filter must reduce the advertised set"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn candidate_filter_that_drops_everything_gives_a_pointed_error() {
    use envoix_session::{CandidateFilter, bind_iroh_endpoint_enable_mdns};

    let listen = envoix_session::BindAddrs::dual_stack(0);
    let bound = bind_iroh_endpoint_enable_mdns(
        listen,
        &IdentityConfig::Ephemeral,
        &CandidateFilter::default(),
        DEFAULT_DATA_STREAM_WINDOW,
    )
    .await
    .unwrap();
    let all = bound.direct_addrs();
    let deny: Vec<String> = all.iter().map(|a| a.ip().to_string()).collect();
    let filtered = bound.with_candidate_filter(CandidateFilter::from_lists(&[], &deny).unwrap());

    let error = filtered.peer_descriptor().unwrap_err().to_string();
    assert!(
        error.contains("candidate filter removed every advertisable address"),
        "expected the pointed filter error, got: {error}"
    );
}
