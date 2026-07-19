//! End-to-end: a file transfers from sender to receiver after they pair in a
//! room over a loopback rendezvous broker, using only a short code.

use std::sync::Arc;
use std::time::Duration;

use envoix_protocol::{
    ManifestEntryKind, ManifestEntryV1, ManifestHashAlgorithm, ManifestId, ManifestV1,
    TransferProtocol,
};
use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use envoix_session::{
    DEFAULT_CHUNK_SIZE, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig, ManifestSendRequest,
    NoopEventSink, NoopSessionEventSink, SessionConfig, SessionTransferSummary,
    TransferCancelToken, receive_file_via_room, receive_transfer_via_room, send_file_via_room,
    send_manifest_via_room,
};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use tempfile::tempdir;

static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..100 {
        if ep.addr().ip_addrs().next().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    endpoint_addr(ep)
}

async fn start_broker(registry: Arc<RoomRegistry>) -> EndpointAddr {
    let server = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .unwrap();
    let broker = ready_addr(&server).await;
    tokio::spawn(serve_endpoint(server, registry, None));
    broker
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
    let _guard = IROH_TEST_LOCK.lock().await;
    // Rendezvous broker.
    let broker = start_broker(Arc::new(RoomRegistry::new())).await;

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
async fn room_reconfirms_an_existing_single_file() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let broker = start_broker(Arc::new(RoomRegistry::new())).await;
    let dir = tempdir().unwrap();
    let src = dir.path().join("repeat.txt");
    let contents = b"repeat through room without retransmitting";
    std::fs::write(&src, contents).unwrap();
    let out = dir.path().join("received");
    std::fs::create_dir(&out).unwrap();
    let listen: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    for (code, resume) in [
        ("2468-room-repeat-fresh", false),
        ("2468-room-repeat-resume", true),
    ] {
        let receiver_broker = broker.clone();
        let receiver_output = out.clone();
        let recv = tokio::spawn(async move {
            receive_transfer_via_room(
                receiver_broker,
                code,
                listen,
                receiver_output,
                config(),
                Box::new(NoopSessionEventSink),
                TransferCancelToken::new(),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let sent = send_file_via_room(
            broker.clone(),
            code,
            src.clone(),
            resume,
            config(),
            Box::new(NoopEventSink),
            TransferCancelToken::new(),
        );

        tokio::time::timeout(Duration::from_secs(30), sent)
            .await
            .expect("room sender timed out")
            .expect("room sender failed");
        let received = tokio::time::timeout(Duration::from_secs(30), recv)
            .await
            .expect("room receiver timed out")
            .expect("room receiver task panicked")
            .expect("room receiver failed");
        assert!(matches!(received, SessionTransferSummary::SingleFile(_)));
    }

    assert_eq!(std::fs::read(out.join("repeat.txt")).unwrap(), contents);
    assert!(!out.join("repeat (1).txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifest_transfers_through_the_existing_room_rendezvous() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let broker = start_broker(Arc::new(RoomRegistry::new())).await;

    let dir = tempdir().unwrap();
    let source = dir.path().join("room-manifest.txt");
    let contents = b"manifest payload through existing room pairing";
    std::fs::write(&source, contents).unwrap();
    let out = dir.path().join("received");
    std::fs::create_dir(&out).unwrap();
    let manifest = ManifestV1 {
        manifest_id: ManifestId::new("room-manifest-routing"),
        entries: vec![
            ManifestEntryV1 {
                entry_id: 0,
                relative_path: "room".to_string(),
                kind: ManifestEntryKind::Directory,
                size: 0,
                hash: None,
                modified_at_unix_ms: None,
            },
            ManifestEntryV1 {
                entry_id: 1,
                relative_path: "room/room-manifest.txt".to_string(),
                kind: ManifestEntryKind::RegularFile,
                size: contents.len() as u64,
                hash: Some(*blake3::hash(contents).as_bytes()),
                modified_at_unix_ms: None,
            },
        ],
        file_count: 1,
        directory_count: 1,
        root_count: 1,
        total_bytes: contents.len() as u64,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    };
    let request = ManifestSendRequest::new(manifest, [(1, source)]).unwrap();
    let code = "5678-manifest-room";
    let listen: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    let receiver_broker = broker.clone();
    let recv = tokio::spawn(async move {
        receive_transfer_via_room(
            receiver_broker,
            code,
            listen,
            out,
            config(),
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let send = tokio::spawn(async move {
        send_manifest_via_room(
            broker,
            code,
            request,
            true,
            config(),
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        )
        .await
    });

    let join = Duration::from_secs(30);
    let sent = tokio::time::timeout(join, send)
        .await
        .expect("Manifest room send timed out")
        .expect("Manifest room send task panicked")
        .expect("Manifest room send failed");
    let received = tokio::time::timeout(join, recv)
        .await
        .expect("Manifest room receive timed out")
        .expect("Manifest room receive task panicked")
        .expect("Manifest room receive failed");

    assert_eq!(sent.file_count, 1);
    assert_eq!(received.protocol(), TransferProtocol::ManifestV1);
    assert!(matches!(received, SessionTransferSummary::Manifest(_)));
    assert_eq!(
        std::fs::read(dir.path().join("received/room/room-manifest.txt")).unwrap(),
        contents
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_expiry_reports_no_peer_joined() {
    let _guard = IROH_TEST_LOCK.lock().await;
    use std::time::Duration;

    // Broker with a short room TTL so the wait window elapses quickly.
    let broker = start_broker(Arc::new(RoomRegistry::with_ttl(Duration::from_secs(2)))).await;

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
    let _guard = IROH_TEST_LOCK.lock().await;
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
    let _guard = IROH_TEST_LOCK.lock().await;
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
