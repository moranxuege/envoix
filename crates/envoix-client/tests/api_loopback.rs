//! End-to-end loopback transfer through the new unified API: a ShowManual
//! listener and a Manual dialer over real iroh endpoints on this host.

use envoix_client::api::{
    Client, ManifestEntryKind, ManifestEntryV1, ManifestHashAlgorithm, ManifestId,
    ManifestSendRequest, ManifestTransferRequest, ManifestV1, PeerSource, SessionTransferSummary,
    TransferEvent, TransferOptions, TransferRequest,
};
use envoix_client::{PeerDescriptor, TransferDirection};
use std::io::ErrorKind;
use std::net::UdpSocket;

static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn wait_for_transfer_set_advertisement(
    transfer: &mut envoix_client::api::TransferSet,
    token: &str,
) -> PeerDescriptor {
    loop {
        let stamped = transfer
            .next_event()
            .await
            .expect("receiver advertises before ending");
        match stamped.event {
            TransferEvent::Advertised {
                peer,
                token: advertised_token,
                invite,
            } => {
                assert_eq!(advertised_token.as_deref(), Some(token));
                assert_eq!(invite, None);
                return peer;
            }
            TransferEvent::Binding { .. } => {}
            other => panic!("unexpected receiver event before Advertised: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_loopback_roundtrip() {
    if !loopback_transport_available() {
        return;
    }
    let _guard = IROH_TEST_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "envoix-api-loopback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let out_dir = root.join("out");
    tokio::fs::create_dir_all(&out_dir).await.unwrap();
    let source_path = root.join("payload.bin");
    let payload = vec![7_u8; 300 * 1024];
    tokio::fs::write(&source_path, &payload).await.unwrap();

    let client = Client::new();
    let token = "loopback-api-token-1".to_string();

    let mut receive = client
        .receive(
            out_dir.clone(),
            PeerSource::ShowManual {
                token: Some(token.clone()),
            },
            TransferOptions::default(),
        )
        .unwrap();

    // The listener reports itself: Binding, then Advertised with our token.
    let peer = loop {
        let Some(stamped) = receive.next_event().await else {
            panic!("receiver event stream ended: {:?}", receive.wait().await);
        };
        match stamped.event {
            TransferEvent::Advertised {
                peer,
                token: advertised_token,
                invite,
            } => {
                assert_eq!(advertised_token.as_deref(), Some(token.as_str()));
                assert_eq!(invite, None);
                break peer;
            }
            TransferEvent::Binding { .. } => {}
            other => panic!("unexpected receiver event before Advertised: {other:?}"),
        }
    };

    let send = client
        .send(
            source_path.clone(),
            PeerSource::Manual {
                peer,
                token: token.clone(),
            },
            TransferOptions::default(),
        )
        .unwrap();

    let send_summary = send.wait().await.expect("send completes");
    assert_eq!(send_summary.bytes_transferred, payload.len() as u64);

    // Drain the receiver's stream: it must tell the whole story and end.
    let mut saw_started = false;
    let mut saw_completed = false;
    while let Some(event) = receive.next_event().await {
        match event.event {
            TransferEvent::Started { .. } => saw_started = true,
            TransferEvent::Completed {
                bytes_transferred, ..
            } => {
                assert_eq!(bytes_transferred, payload.len() as u64);
                saw_completed = true;
            }
            _ => {}
        }
    }
    assert!(saw_started, "receiver stream missing Started");
    assert!(saw_completed, "receiver stream missing Completed");

    let receive_summary = receive.wait().await.expect("receive completes");
    assert_eq!(receive_summary.bytes_transferred, payload.len() as u64);
    assert_eq!(
        tokio::fs::read(out_dir.join("payload.bin")).await.unwrap(),
        payload
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_manifest_loopback_roundtrip() {
    if !loopback_transport_available() {
        return;
    }
    let _guard = IROH_TEST_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "envoix-api-manifest-loopback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source_dir = root.join("source");
    let out_dir = root.join("out");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    tokio::fs::create_dir_all(&out_dir).await.unwrap();
    let photo_path = source_dir.join("photo.jpg");
    let notes_path = source_dir.join("notes.txt");
    let photo = vec![0x5a_u8; 180 * 1024];
    let notes = b"two files plus explicit and empty directories".to_vec();
    tokio::fs::write(&photo_path, &photo).await.unwrap();
    tokio::fs::write(&notes_path, &notes).await.unwrap();
    let total_bytes = (photo.len() + notes.len()) as u64;
    let manifest_id = ManifestId::new("client-manual-manifest-loopback");
    let request = ManifestSendRequest::new(
        ManifestV1 {
            manifest_id: manifest_id.clone(),
            entries: vec![
                ManifestEntryV1 {
                    entry_id: 0,
                    relative_path: "album".into(),
                    kind: ManifestEntryKind::Directory,
                    size: 0,
                    hash: None,
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 1,
                    relative_path: "album/photo.jpg".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: photo.len() as u64,
                    hash: Some(*blake3::hash(&photo).as_bytes()),
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 2,
                    relative_path: "notes.txt".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: notes.len() as u64,
                    hash: Some(*blake3::hash(&notes).as_bytes()),
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 3,
                    relative_path: "empty".into(),
                    kind: ManifestEntryKind::Directory,
                    size: 0,
                    hash: None,
                    modified_at_unix_ms: None,
                },
            ],
            file_count: 2,
            directory_count: 2,
            root_count: 3,
            total_bytes,
            hash_algorithm: ManifestHashAlgorithm::Blake3_256,
        },
        [(1, photo_path), (2, notes_path)],
    )
    .unwrap();

    let client = Client::new();
    let token = "loopback-api-manifest-token-1".to_string();
    let mut receive = client
        .run_receive_transfer(TransferRequest {
            direction: TransferDirection::Receive,
            path: out_dir.clone(),
            sources: vec![PeerSource::ShowManual {
                token: Some(token.clone()),
            }],
            options: TransferOptions::default(),
        })
        .unwrap();

    let peer = wait_for_transfer_set_advertisement(&mut receive, &token).await;

    let mut send = client
        .run_manifest(ManifestTransferRequest {
            request,
            sources: vec![PeerSource::Manual {
                peer,
                token: token.clone(),
            }],
            options: TransferOptions::default(),
        })
        .unwrap();
    let mut sender_prepared = 0;
    let mut sender_completed = false;
    while let Some(event) = send.next_event().await {
        match event.event {
            TransferEvent::ManifestPreparingEntry { .. } => sender_prepared += 1,
            TransferEvent::ManifestCompleted {
                manifest_id: completed_id,
                total_bytes: completed_bytes,
                ..
            } => {
                assert_eq!(completed_id, manifest_id);
                assert_eq!(completed_bytes, total_bytes);
                sender_completed = true;
            }
            _ => {}
        }
    }
    assert_eq!(sender_prepared, 2);
    assert!(sender_completed, "sender stream missing ManifestCompleted");
    let send_summary = send.wait().await.expect("Manifest send completes");
    let SessionTransferSummary::Manifest(send_summary) = send_summary else {
        panic!("Manifest sender returned a single-file summary");
    };
    assert_eq!(send_summary.manifest_id, manifest_id);
    assert_eq!(send_summary.total_bytes, total_bytes);

    let mut receiver_entries = 0;
    let mut receiver_completed = false;
    while let Some(event) = receive.next_event().await {
        match event.event {
            TransferEvent::ManifestEntryCompleted { .. } => receiver_entries += 1,
            TransferEvent::ManifestCompleted {
                manifest_id: completed_id,
                total_bytes: completed_bytes,
                ..
            } => {
                assert_eq!(completed_id, manifest_id);
                assert_eq!(completed_bytes, total_bytes);
                receiver_completed = true;
            }
            _ => {}
        }
    }
    assert_eq!(receiver_entries, 4);
    assert!(
        receiver_completed,
        "receiver stream missing ManifestCompleted"
    );
    let receive_summary = receive.wait().await.expect("Manifest receive completes");
    let SessionTransferSummary::Manifest(receive_summary) = receive_summary else {
        panic!("Manifest receiver returned a single-file summary");
    };
    assert_eq!(receive_summary.manifest_id, manifest_id);
    assert_eq!(receive_summary.file_count, 2);
    assert_eq!(receive_summary.directory_count, 2);
    assert_eq!(
        tokio::fs::read(out_dir.join("album/photo.jpg"))
            .await
            .unwrap(),
        photo
    );
    assert_eq!(
        tokio::fs::read(out_dir.join("notes.txt")).await.unwrap(),
        notes
    );
    assert!(
        tokio::fs::metadata(out_dir.join("empty"))
            .await
            .unwrap()
            .is_dir()
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn negotiated_receiver_keeps_legacy_single_file_compatible() {
    if !loopback_transport_available() {
        return;
    }
    let _guard = IROH_TEST_LOCK.lock().await;
    let root = std::env::temp_dir().join(format!(
        "envoix-api-negotiated-single-loopback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let out_dir = root.join("out");
    tokio::fs::create_dir_all(&out_dir).await.unwrap();
    let source_path = root.join("legacy.bin");
    let payload = vec![0x2a_u8; 96 * 1024];
    tokio::fs::write(&source_path, &payload).await.unwrap();

    let client = Client::new();
    let token = "loopback-negotiated-single-token-1".to_string();
    let mut receive = client
        .receive_transfer(
            out_dir.clone(),
            PeerSource::ShowManual {
                token: Some(token.clone()),
            },
            TransferOptions::default(),
        )
        .unwrap();
    let peer = wait_for_transfer_set_advertisement(&mut receive, &token).await;

    let send = client
        .send(
            source_path,
            PeerSource::Manual { peer, token },
            TransferOptions::default(),
        )
        .unwrap();
    assert_eq!(
        send.wait()
            .await
            .expect("legacy sender completes")
            .bytes_transferred,
        payload.len() as u64
    );

    let mut saw_legacy_completed = false;
    while let Some(event) = receive.next_event().await {
        if let TransferEvent::Completed {
            bytes_transferred, ..
        } = event.event
        {
            assert_eq!(bytes_transferred, payload.len() as u64);
            saw_legacy_completed = true;
        }
    }
    assert!(saw_legacy_completed);
    let summary = receive.wait().await.expect("negotiated receiver completes");
    let SessionTransferSummary::SingleFile(summary) = summary else {
        panic!("legacy sender unexpectedly produced a Manifest summary");
    };
    assert_eq!(summary.bytes_transferred, payload.len() as u64);
    assert_eq!(
        tokio::fs::read(out_dir.join("legacy.bin")).await.unwrap(),
        payload
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}

fn loopback_transport_available() -> bool {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            println!("skipping API loopback tests: UDP bind permission denied ({error})");
            false
        }
        Err(error) => panic!("API loopback transport pre-check failed: {error}"),
    }
}
