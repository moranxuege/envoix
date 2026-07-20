use super::*;

fn manifest_send_request() -> ManifestSendRequest {
    ManifestSendRequest::new(
        ManifestV1 {
            manifest_id: ManifestId::new("client-run-manifest"),
            entries: vec![ManifestEntryV1 {
                entry_id: 0,
                relative_path: "file.bin".into(),
                kind: ManifestEntryKind::RegularFile,
                size: 1,
                hash: Some([7; 32]),
                modified_at_unix_ms: None,
            }],
            file_count: 1,
            directory_count: 0,
            root_count: 1,
            total_bytes: 1,
            hash_algorithm: ManifestHashAlgorithm::Blake3_256,
        },
        [(0, PathBuf::from("file.bin"))],
    )
    .unwrap()
}

fn client() -> Client {
    Client::new()
}

#[test]
fn send_rejects_producer_sources() {
    for source in [
        PeerSource::ShowManual { token: None },
        PeerSource::ShowInvite {
            ttl_secs: 300,
            token: None,
        },
    ] {
        let error = client()
            .send("f.txt".into(), source, TransferOptions::default())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }
}

#[test]
fn send_rejects_invalid_chunk_size() {
    let mut client = Client::new();
    client.chunk_size = 0;
    let error = client
        .send(
            "f.txt".into(),
            PeerSource::Mdns {
                token: Some("abcdefghijkl".into()),
            },
            TransferOptions::default(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn send_over_mdns_requires_token() {
    let error = client()
        .send(
            "f.txt".into(),
            PeerSource::Mdns { token: None },
            TransferOptions::default(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn relay_only_requires_relay() {
    let options = TransferOptions {
        path: PathPolicy::RelayOnly,
        ..Default::default()
    };
    let error = client()
        .send(
            "f.txt".into(),
            PeerSource::Room {
                code: "123456-a-b".into(),
                broker: "unused".into(),
            },
            options,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn receive_rejects_consumer_sources() {
    let peer = PeerDescriptor::new("peer", vec!["[::1]:9000".parse().unwrap()]).unwrap();
    for source in [
        PeerSource::Manual {
            peer,
            token: "abcdefghijkl".into(),
        },
        PeerSource::Invite {
            invite: "envoix:whatever".into(),
        },
    ] {
        let error = client()
            .receive("out".into(), source, TransferOptions::default())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }
}

#[test]
fn runtime_sources_read_candidate_cidrs_from_config_file() {
    let path = std::env::temp_dir().join(format!(
        "envoix-api-config-{}-candidates.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "chunk_size = \"1M\"\n[candidates]\ndeny = [\"10.0.0.0/8\", \"fe80::/10\"]\n",
    )
    .unwrap();

    let client = Client::from_runtime_sources(Some(&path)).unwrap();

    assert_eq!(client.chunk_size, 1024 * 1024);
    // The deny list scopes addresses: a LAN address is dropped, a public one kept.
    let kept = client
        .candidates
        .apply(["10.0.0.5:1".parse().unwrap(), "1.2.3.4:2".parse().unwrap()]);
    assert_eq!(kept, vec!["1.2.3.4:2".parse().unwrap()]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn runtime_sources_reject_invalid_candidate_cidr() {
    let path = std::env::temp_dir().join(format!(
        "envoix-api-config-{}-badcidr.toml",
        std::process::id()
    ));
    std::fs::write(&path, "[candidates]\ndeny = [\"not-a-cidr\"]\n").unwrap();
    assert!(Client::from_runtime_sources(Some(&path)).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn runtime_sources_read_chunk_size_from_config_file() {
    let path = std::env::temp_dir().join(format!(
        "envoix-api-config-{}-chunk.toml",
        std::process::id()
    ));
    std::fs::write(&path, "chunk_size = \"1M\"\n").unwrap();

    let client = Client::from_runtime_sources(Some(&path)).unwrap();

    assert_eq!(client.chunk_size, 1024 * 1024);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn config_fields_apply_chunk_size_and_candidate_cidrs() {
    // The FFI path passes discrete fields (no file) and must assemble the
    // same client as the equivalent config.toml above.
    let deny = vec!["10.0.0.0/8".to_string(), "fe80::/10".to_string()];
    let client = Client::from_config_fields(Some("1M"), &[], &deny, None).unwrap();

    assert_eq!(client.chunk_size, 1024 * 1024);
    let kept = client
        .candidates
        .apply(["10.0.0.5:1".parse().unwrap(), "1.2.3.4:2".parse().unwrap()]);
    assert_eq!(kept, vec!["1.2.3.4:2".parse().unwrap()]);
}

#[test]
fn config_fields_reject_invalid_candidate_cidr() {
    assert!(Client::from_config_fields(None, &[], &["not-a-cidr".to_string()], None).is_err());
}

#[test]
fn send_rejects_garbage_invite() {
    let error = client()
        .send(
            "f.txt".into(),
            PeerSource::Invite {
                invite: "not-an-invite".into(),
            },
            TransferOptions::default(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn run_rejects_empty_sources() {
    let error = client()
        .run(TransferRequest {
            direction: TransferDirection::Send,
            path: "f.txt".into(),
            sources: vec![],
            options: TransferOptions::default(),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn run_manifest_rejects_empty_sources() {
    let error = client()
        .run_manifest(ManifestTransferRequest {
            request: manifest_send_request(),
            sources: vec![],
            options: TransferOptions::default(),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[test]
fn negotiated_receive_rejects_send_direction() {
    let error = client()
        .run_receive_transfer(TransferRequest {
            direction: TransferDirection::Send,
            path: "file.bin".into(),
            sources: vec![PeerSource::Mdns { token: None }],
            options: TransferOptions::default(),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[tokio::test]
async fn run_manifest_advances_past_a_source_that_fails_to_build() {
    let mut transfer = client()
        .run_manifest(ManifestTransferRequest {
            request: manifest_send_request(),
            sources: vec![
                PeerSource::Invite {
                    invite: "not-an-invite".into(),
                },
                PeerSource::Mdns { token: None },
            ],
            options: TransferOptions::default(),
        })
        .unwrap();

    let event = transfer.next_event().await.expect("terminal failure event");
    assert!(matches!(
        event.event,
        TransferEvent::Failed {
            direction: TransferDirection::Send,
            ..
        }
    ));
    let error = transfer.wait().await.unwrap_err();
    assert!(
        error.message.contains("mDNS requires a token"),
        "the final error must come from the second source: {error:?}"
    );
}

#[test]
fn only_room_senders_with_a_fallback_get_a_preconnect_deadline() {
    assert_eq!(
        preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Room, true),
        Some(ROOM_SEND_PRECONNECT_TIMEOUT),
    );
    assert_eq!(
        preconnect_timeout_for_source(TransferDirection::Receive, TransferMode::Room, true),
        None,
    );
    assert_eq!(
        preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Room, false),
        None,
    );
    assert_eq!(
        preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Mdns, true),
        None,
    );
}

#[tokio::test]
async fn preconnect_deadline_ends_a_stuck_attempt() {
    let pending: TransferFuture = Box::pin(std::future::pending());
    let (events, _receiver) = EventSender::channel();
    let error = with_preconnection_timeout(
        pending,
        events.stats_handle(),
        Some(Duration::from_millis(5)),
        TransferDirection::Send,
        TransferMode::Room,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("timed out before connecting"));
}

#[test]
fn run_validates_chunk_size_up_front() {
    let mut client = Client::new();
    client.chunk_size = 0;
    let error = client
        .run(TransferRequest {
            direction: TransferDirection::Send,
            path: "f.txt".into(),
            sources: vec![PeerSource::Mdns {
                token: Some("abcdefghijkl".into()),
            }],
            options: TransferOptions::default(),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[tokio::test]
async fn run_surfaces_a_source_failure_through_wait() {
    // A lone unbuildable source (garbage invite) has no fallback, so the
    // error surfaces on the returned handle rather than synchronously.
    let error = client()
        .run(TransferRequest {
            direction: TransferDirection::Send,
            path: "f.txt".into(),
            sources: vec![PeerSource::Invite {
                invite: "not-an-invite".into(),
            }],
            options: TransferOptions::default(),
        })
        .expect("run spawns the transfer")
        .wait()
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
}

#[tokio::test]
async fn run_emits_a_terminal_failed_event_with_reason_code() {
    // The event stream must tell the whole story on its own: a failed run
    // ends with a Failed event carrying the typed reason_code (frontends
    // branch on it; the operation's Result is a separate channel).
    let mut transfer = client()
        .run(TransferRequest {
            direction: TransferDirection::Send,
            path: "f.txt".into(),
            sources: vec![PeerSource::Invite {
                invite: "not-an-invite".into(),
            }],
            options: TransferOptions::default(),
        })
        .expect("run spawns the transfer");
    let mut terminal = None;
    while let Some(stamped) = transfer.next_event().await {
        if let TransferEvent::Failed { reason_code, .. } = stamped.event {
            terminal = Some(reason_code);
        }
    }
    assert_eq!(terminal, Some(SessionFailureCode::Other));
}
