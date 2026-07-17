//! Real iroh coverage for additive ALPN routing. The negotiated receiver keeps
//! the existing single-file path working while accepting Manifest v1 on the
//! same endpoint.

use std::path::{Path, PathBuf};
use std::time::Duration;

use envoix_protocol::{
    ManifestEntryKind, ManifestEntryV1, ManifestHashAlgorithm, ManifestId, ManifestV1,
    TransferProtocol,
};
use envoix_session::{
    BindAddrs, DEFAULT_CHUNK_SIZE, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig,
    MANIFEST_UNSUPPORTED_PEER_CODE, ManifestSendRequest, NoopEventSink, NoopSessionEventSink,
    PairingConfig, SessionConfig, SessionTransferSummary, TransferCancelToken,
    receive_file_with_bound_peer, receive_transfer_enable_mdns, receive_transfer_with_bound_peer,
    send_file_enable_mdns, send_file_manual, send_manifest_enable_mdns, send_manifest_manual,
};
use tempfile::tempdir;
use tokio::sync::oneshot;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHARED_TOKEN: &str = "manifest-routing-shared-token";
const WRONG_SHARED_TOKEN: &str = "manifest-routing-wrong-token";
static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn pairing() -> PairingConfig {
    pairing_with_token(SHARED_TOKEN)
}

fn pairing_with_token(token: &str) -> PairingConfig {
    PairingConfig::spake2_shared_token(token).unwrap()
}

fn regular_file(entry_id: u32, relative_path: &str, bytes: &[u8]) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.to_string(),
        kind: ManifestEntryKind::RegularFile,
        size: bytes.len() as u64,
        hash: Some(*blake3::hash(bytes).as_bytes()),
        modified_at_unix_ms: None,
    }
}

fn directory(entry_id: u32, relative_path: &str) -> ManifestEntryV1 {
    ManifestEntryV1 {
        entry_id,
        relative_path: relative_path.to_string(),
        kind: ManifestEntryKind::Directory,
        size: 0,
        hash: None,
        modified_at_unix_ms: None,
    }
}

fn manifest_request(source_root: &Path) -> ManifestSendRequest {
    let first = b"first file over manifest session routing";
    let second = b"second file over manifest session routing";
    let first_path = source_root.join("first.txt");
    let second_path = source_root.join("second.txt");
    std::fs::write(&first_path, first).unwrap();
    std::fs::write(&second_path, second).unwrap();

    let manifest = ManifestV1 {
        manifest_id: ManifestId::new("session-routing-manifest"),
        entries: vec![
            directory(0, "album"),
            regular_file(1, "album/first.txt", first),
            regular_file(2, "second.txt", second),
        ],
        file_count: 2,
        directory_count: 1,
        root_count: 2,
        total_bytes: (first.len() + second.len()) as u64,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    };
    ManifestSendRequest::new(manifest, [(1, first_path), (2, second_path)]).unwrap()
}

async fn bound_peer_receiver(
    output_dir: PathBuf,
) -> (
    envoix_protocol::PeerDescriptor,
    tokio::task::JoinHandle<Result<SessionTransferSummary, envoix_session::SessionError>>,
) {
    let (peer_tx, peer_rx) = oneshot::channel();
    let mut receive = tokio::spawn(async move {
        let pairing = pairing();
        receive_transfer_with_bound_peer(
            "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
            output_dir,
            config(),
            &pairing,
            Box::new(NoopSessionEventSink),
            move |peer, _relay_urls| {
                let _ = peer_tx.send(peer);
            },
            TransferCancelToken::new(),
        )
        .await
    });
    let peer = match tokio::time::timeout(TEST_TIMEOUT, peer_rx).await {
        Ok(Ok(peer)) => peer,
        callback => {
            let task = tokio::time::timeout(Duration::from_secs(1), &mut receive).await;
            panic!("receiver failed before bound callback: callback={callback:?}, task={task:?}");
        }
    };
    (peer, receive)
}

async fn mdns_receiver(
    output_dir: PathBuf,
) -> tokio::task::JoinHandle<Result<SessionTransferSummary, envoix_session::SessionError>> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let mut receive = tokio::spawn(async move {
        let pairing = pairing();
        receive_transfer_enable_mdns(
            BindAddrs::single("127.0.0.1:0".parse().unwrap()),
            output_dir,
            config(),
            &pairing,
            Box::new(NoopSessionEventSink),
            move |_peer, _relay_urls| {
                let _ = ready_tx.send(());
            },
            TransferCancelToken::new(),
        )
        .await
    });
    if let callback @ (Err(_) | Ok(Err(_))) = tokio::time::timeout(TEST_TIMEOUT, ready_rx).await {
        let task = tokio::time::timeout(Duration::from_secs(1), &mut receive).await;
        panic!("mDNS receiver failed before advertising: callback={callback:?}, task={task:?}");
    }
    receive
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dual_alpn_receiver_routes_manifest_after_authentication() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source_root = temp.path().join("source");
    let output_dir = temp.path().join("received");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let request = manifest_request(&source_root);
    let (peer, receive) = bound_peer_receiver(output_dir.clone()).await;

    let pairing = pairing();
    let sent = tokio::time::timeout(
        TEST_TIMEOUT,
        send_manifest_manual(
            peer,
            request,
            true,
            config(),
            &pairing,
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("manifest sender timed out")
    .expect("manifest sender failed");
    let received = tokio::time::timeout(TEST_TIMEOUT, receive)
        .await
        .expect("manifest receiver timed out")
        .expect("manifest receiver task panicked")
        .expect("manifest receiver failed");

    assert_eq!(sent.file_count, 2);
    assert_eq!(received.protocol(), TransferProtocol::ManifestV1);
    assert!(matches!(received, SessionTransferSummary::Manifest(_)));
    assert_eq!(
        std::fs::read(output_dir.join("album/first.txt")).unwrap(),
        b"first file over manifest session routing"
    );
    assert_eq!(
        std::fs::read(output_dir.join("second.txt")).unwrap(),
        b"second file over manifest session routing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dual_alpn_receiver_preserves_single_file_compatibility() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source = temp.path().join("legacy.txt");
    let output_dir = temp.path().join("received");
    std::fs::write(&source, b"existing single-file protocol").unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let (peer, receive) = bound_peer_receiver(output_dir.clone()).await;

    let pairing = pairing();
    tokio::time::timeout(
        TEST_TIMEOUT,
        send_file_manual(
            peer,
            source,
            false,
            config(),
            &pairing,
            Box::new(NoopEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("single-file sender timed out")
    .expect("single-file sender failed");
    let received = tokio::time::timeout(TEST_TIMEOUT, receive)
        .await
        .expect("single-file receiver timed out")
        .expect("single-file receiver task panicked")
        .expect("single-file receiver failed");

    assert_eq!(received.protocol(), TransferProtocol::SingleFileV1);
    assert!(matches!(received, SessionTransferSummary::SingleFile(_)));
    assert_eq!(
        std::fs::read(output_dir.join("legacy.txt")).unwrap(),
        b"existing single-file protocol"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_single_file_receiver_rejects_manifest_before_payload() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source_root = temp.path().join("source");
    let output_dir = temp.path().join("received");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let request = manifest_request(&source_root);
    let (peer_tx, peer_rx) = oneshot::channel();
    let receiver_output = output_dir.clone();
    let receiver_cancel = TransferCancelToken::new();
    let receive_cancel = receiver_cancel.clone();
    let mut receive = tokio::spawn(async move {
        let pairing = pairing();
        receive_file_with_bound_peer(
            "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
            receiver_output,
            config(),
            &pairing,
            Box::new(NoopEventSink),
            move |peer, _relay_urls| {
                let _ = peer_tx.send(peer);
            },
            receive_cancel,
        )
        .await
    });
    let peer = match tokio::time::timeout(TEST_TIMEOUT, peer_rx).await {
        Ok(Ok(peer)) => peer,
        callback => {
            let task = tokio::time::timeout(Duration::from_secs(1), &mut receive).await;
            panic!(
                "legacy receiver failed before bound callback: callback={callback:?}, task={task:?}"
            );
        }
    };

    let pairing = pairing();
    let error = tokio::time::timeout(
        TEST_TIMEOUT,
        send_manifest_manual(
            peer,
            request,
            true,
            config(),
            &pairing,
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("manifest ALPN rejection timed out")
    .expect_err("legacy endpoint unexpectedly accepted Manifest v1");
    assert!(
        error.to_string().contains(MANIFEST_UNSUPPORTED_PEER_CODE),
        "unexpected Manifest ALPN rejection: {error}"
    );

    assert!(
        std::fs::read_dir(&output_dir).unwrap().next().is_none(),
        "ALPN rejection must happen before receiver payload writes"
    );
    receiver_cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), receive)
        .await
        .expect("legacy receiver did not stop after cancellation")
        .expect("legacy receiver task panicked")
        .expect_err("legacy receiver unexpectedly completed a file transfer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifest_routing_never_starts_before_authentication_succeeds() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source_root = temp.path().join("source");
    let output_dir = temp.path().join("received");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let request = manifest_request(&source_root);
    let (peer, receive) = bound_peer_receiver(output_dir.clone()).await;

    let wrong_pairing = pairing_with_token(WRONG_SHARED_TOKEN);
    let send_error = tokio::time::timeout(
        TEST_TIMEOUT,
        send_manifest_manual(
            peer,
            request,
            true,
            config(),
            &wrong_pairing,
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("mismatched authentication timed out")
    .expect_err("sender unexpectedly authenticated with the wrong token");
    let receive_error = tokio::time::timeout(TEST_TIMEOUT, receive)
        .await
        .expect("receiver authentication timed out")
        .expect("receiver task panicked")
        .expect_err("receiver unexpectedly authenticated the wrong token");

    assert!(!send_error.to_string().is_empty());
    assert!(!receive_error.to_string().is_empty());
    assert!(
        std::fs::read_dir(&output_dir).unwrap().next().is_none(),
        "authentication failure must happen before Manifest creates state or output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mdns_dual_receiver_routes_manifest() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source_root = temp.path().join("source");
    let output_dir = temp.path().join("received");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let request = manifest_request(&source_root);
    let receive = mdns_receiver(output_dir.clone()).await;

    let pairing = pairing();
    let sent = tokio::time::timeout(
        TEST_TIMEOUT,
        send_manifest_enable_mdns(
            request,
            true,
            config(),
            &pairing,
            Box::new(NoopSessionEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("mDNS Manifest sender timed out")
    .expect("mDNS Manifest sender failed");
    let received = tokio::time::timeout(TEST_TIMEOUT, receive)
        .await
        .expect("mDNS Manifest receiver timed out")
        .expect("mDNS Manifest receiver task panicked")
        .expect("mDNS Manifest receiver failed");

    assert_eq!(sent.file_count, 2);
    assert_eq!(received.protocol(), TransferProtocol::ManifestV1);
    assert_eq!(
        std::fs::read(output_dir.join("album/first.txt")).unwrap(),
        b"first file over manifest session routing"
    );
    assert_eq!(
        std::fs::read(output_dir.join("second.txt")).unwrap(),
        b"second file over manifest session routing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mdns_dual_receiver_preserves_single_file_sender() {
    let _guard = IROH_TEST_LOCK.lock().await;
    let temp = tempdir().unwrap();
    let source = temp.path().join("legacy-mdns.txt");
    let output_dir = temp.path().join("received");
    std::fs::write(&source, b"existing mDNS single-file protocol").unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let receive = mdns_receiver(output_dir.clone()).await;

    let pairing = pairing();
    tokio::time::timeout(
        TEST_TIMEOUT,
        send_file_enable_mdns(
            source,
            false,
            config(),
            &pairing,
            Box::new(NoopEventSink),
            TransferCancelToken::new(),
        ),
    )
    .await
    .expect("mDNS single-file sender timed out")
    .expect("mDNS single-file sender failed");
    let received = tokio::time::timeout(TEST_TIMEOUT, receive)
        .await
        .expect("mDNS single-file receiver timed out")
        .expect("mDNS single-file receiver task panicked")
        .expect("mDNS single-file receiver failed");

    assert_eq!(received.protocol(), TransferProtocol::SingleFileV1);
    assert_eq!(
        std::fs::read(output_dir.join("legacy-mdns.txt")).unwrap(),
        b"existing mDNS single-file protocol"
    );
}
