//! End-to-end loopback transfer through the new unified API: a ShowManual
//! listener and a Manual dialer over real iroh endpoints on this host.

use envoix_client::api::{Client, PeerSource, TransferEvent, TransferOptions};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_loopback_roundtrip() {
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
        match receive
            .next_event()
            .await
            .expect("receiver event stream")
            .event
        {
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
