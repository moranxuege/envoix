use super::*;
use envoix_protocol::{
    ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryResultV1, ManifestEntryV1,
    ManifestHashAlgorithm, ManifestId,
};
use envoix_session::{
    EventSink as _, ManifestEventSink as _, ManifestTransferSummary, TransferDirection,
};
use envoix_types::TransferId;

#[tokio::test]
async fn detach_aborts_the_task_and_says_nothing() {
    let (_events_tx, events) = mpsc::unbounded_channel();
    let cancel = TransferCancelToken::new();
    // The task holds `alive_tx`; an abort drops it without sending.
    let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _keep = alive_tx;
        std::future::pending::<Result<TransferSummary, PublicError>>().await
    });
    let transfer = Transfer::new(
        events,
        cancel.clone(),
        PhaseCell::new(),
        StatsHandle::new(),
        task,
    );

    transfer.detach();

    alive_rx.await.unwrap_err(); // aborted, not completed
    assert!(
        !cancel.is_cancelled(),
        "detach is not a user intent: the interrupt token must stay untouched \
         (a triggered token sends an interrupt frame the peer reads as cancel)"
    );
}

#[tokio::test]
async fn cancel_and_join_fires_the_token_and_waits_for_the_task() {
    let (_events_tx, events) = mpsc::unbounded_channel();
    let cancel = TransferCancelToken::new();
    // A cooperative engine: ends as soon as the token fires.
    let token = cancel.clone();
    let task = tokio::spawn(async move {
        token.cancelled().await;
        Err(PublicError::Transfer("cancelled".into()))
    });
    let transfer = Transfer::new(
        events,
        cancel.clone(),
        PhaseCell::new(),
        StatsHandle::new(),
        task,
    );

    transfer.cancel_and_join().await;

    assert!(cancel.is_cancelled(), "discard is an explicit user intent");
}

#[tokio::test(start_paused = true)]
async fn cancel_and_join_aborts_a_wedged_task() {
    let (_events_tx, events) = mpsc::unbounded_channel();
    let cancel = TransferCancelToken::new();
    // A wedged engine: ignores the token (e.g. an unbounded await).
    let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _keep = alive_tx;
        std::future::pending::<Result<TransferSummary, PublicError>>().await
    });
    let transfer = Transfer::new(
        events,
        cancel.clone(),
        PhaseCell::new(),
        StatsHandle::new(),
        task,
    );

    transfer.cancel_and_join().await; // paused clock: grace elapses instantly

    alive_rx.await.unwrap_err(); // the wedged task was aborted, not leaked
}

#[tokio::test]
async fn plain_drop_still_cancels_a_live_attempt() {
    let (_events_tx, events) = mpsc::unbounded_channel();
    let cancel = TransferCancelToken::new();
    let task = tokio::spawn(async {
        std::future::pending::<Result<TransferSummary, PublicError>>().await
    });
    let transfer = Transfer::new(
        events,
        cancel.clone(),
        PhaseCell::new(),
        StatsHandle::new(),
        task,
    );

    drop(transfer);

    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn session_adapter_maps_legacy_events() {
    let (sender, mut receiver) = EventSender::channel();
    let adapter = SessionEventAdapter(sender);

    adapter.on_event(SessionEvent::HashStarted {
        transfer_id: TransferId::new("t1"),
        direction: TransferDirection::Receive,
        file_name: "a.bin".into(),
        bytes_to_hash: 42,
    });
    adapter.on_event(SessionEvent::Completed {
        transfer_id: TransferId::new("t1"),
        file_name: "a.bin".into(),
        bytes_transferred: 42,
    });

    let first = receiver.recv().await.unwrap();
    assert!(first.ts_ms > 0);
    assert_eq!(
        first.event,
        TransferEvent::Verifying {
            transfer_id: TransferId::new("t1"),
            direction: TransferDirection::Receive,
            file_name: "a.bin".into(),
            bytes_to_hash: 42,
        }
    );
    assert_eq!(
        receiver.recv().await.unwrap().event,
        TransferEvent::Completed {
            transfer_id: TransferId::new("t1"),
            file_name: "a.bin".into(),
            bytes_transferred: 42,
        }
    );
}

#[tokio::test]
async fn session_adapter_maps_every_manifest_event() {
    let (sender, mut receiver) = EventSender::channel();
    let adapter = SessionEventAdapter(sender);
    let manifest_id = ManifestId::new("manifest-client-events");
    let transfer_id = TransferId::new("manifest-client-events:1");
    let manifest = ManifestV1 {
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
                size: 120,
                hash: Some([7; 32]),
                modified_at_unix_ms: None,
            },
        ],
        file_count: 1,
        directory_count: 1,
        root_count: 1,
        total_bytes: 120,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    };
    let result = ManifestEntryResultV1 {
        entry_id: 1,
        status: ManifestEntryResultStatus::Completed,
        offered_relative_path: "album/photo.jpg".into(),
        final_relative_path: Some("album/photo.jpg".into()),
        failure_code: None,
    };

    adapter.on_manifest_plan(TransferDirection::Receive, &manifest);

    for event in [
        SessionManifestEvent::PreparingEntry {
            manifest_id: manifest_id.clone(),
            entry_id: 1,
            relative_path: "album/photo.jpg".into(),
            size: 120,
        },
        SessionManifestEvent::Started {
            manifest_id: manifest_id.clone(),
            direction: TransferDirection::Receive,
            file_count: 1,
            directory_count: 1,
            total_bytes: 120,
        },
        SessionManifestEvent::EntryStarted {
            manifest_id: manifest_id.clone(),
            entry_id: 1,
            transfer_id: transfer_id.clone(),
            relative_path: "album/photo.jpg".into(),
            total_bytes: 120,
            bytes_resumed: 20,
        },
        SessionManifestEvent::Progress {
            manifest_id: manifest_id.clone(),
            entry_id: 1,
            entry_bytes: 70,
            entry_total_bytes: 120,
            completed_bytes: 70,
            total_bytes: 120,
        },
        SessionManifestEvent::EntryCompleted {
            manifest_id: manifest_id.clone(),
            result: result.clone(),
        },
        SessionManifestEvent::Completed {
            summary: ManifestTransferSummary {
                manifest_id: manifest_id.clone(),
                file_count: 1,
                directory_count: 1,
                total_bytes: 120,
                entries: vec![result.clone()],
            },
        },
    ] {
        adapter.on_manifest_event(event);
    }

    let mut actual = Vec::new();
    for _ in 0..7 {
        actual.push(receiver.recv().await.unwrap().event);
    }
    assert_eq!(
        actual,
        vec![
            TransferEvent::ManifestPlanned {
                direction: TransferDirection::Receive,
                manifest: manifest.clone(),
            },
            TransferEvent::ManifestPreparingEntry {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                relative_path: "album/photo.jpg".into(),
                size: 120,
            },
            TransferEvent::ManifestStarted {
                manifest_id: manifest_id.clone(),
                direction: TransferDirection::Receive,
                file_count: 1,
                directory_count: 1,
                total_bytes: 120,
            },
            TransferEvent::ManifestEntryStarted {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id,
                relative_path: "album/photo.jpg".into(),
                total_bytes: 120,
                bytes_resumed: 20,
            },
            TransferEvent::ManifestProgress {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                entry_bytes: 70,
                entry_total_bytes: 120,
                completed_bytes: 70,
                total_bytes: 120,
            },
            TransferEvent::ManifestEntryCompleted {
                manifest_id: manifest_id.clone(),
                result: result.clone(),
            },
            TransferEvent::ManifestCompleted {
                manifest_id,
                file_count: 1,
                directory_count: 1,
                total_bytes: 120,
                entries: vec![result],
            },
        ]
    );
}

#[tokio::test]
async fn channel_closes_when_all_senders_drop() {
    let (sender, mut receiver) = EventSender::channel();
    sender.emit(TransferEvent::Pairing {
        step: envoix_types::PairingStep::Joining,
    });
    drop(sender);

    assert_eq!(
        receiver.recv().await.unwrap().event,
        TransferEvent::Pairing {
            step: envoix_types::PairingStep::Joining,
        }
    );
    assert!(receiver.recv().await.is_none());
}

#[test]
fn stats_accumulate_from_the_event_stream() {
    use super::super::TransferMode;
    let addr = "1.2.3.4:5".parse().unwrap();
    let h = StatsHandle::new();
    h.observe(
        1000,
        &TransferEvent::Binding {
            direction: TransferDirection::Send,
            mode: TransferMode::Room,
        },
    );
    h.observe(1100, &TransferEvent::Connecting);
    h.observe(
        1200,
        &TransferEvent::Connected {
            path: DataPath::Relay { url: "r".into() },
        },
    );
    h.observe(
        1300,
        &TransferEvent::Started {
            transfer_id: TransferId::new("t"),
            direction: TransferDirection::Send,
            file_name: "f".into(),
            total_bytes: 1000,
            bytes_resumed: 0,
        },
    );
    let progress = |bytes| TransferEvent::Progress {
        transfer_id: TransferId::new("t"),
        bytes_transferred: bytes,
        total_bytes: 1000,
    };
    h.observe(1300, &progress(0));
    h.observe(
        1400,
        &TransferEvent::PathChanged {
            path: DataPath::Direct { addr },
        },
    );
    h.observe(1400, &progress(1000)); // 1000 B in 100 ms -> 10_000 B/s peak
    h.observe(
        1800,
        &TransferEvent::Completed {
            transfer_id: TransferId::new("t"),
            file_name: "f".into(),
            bytes_transferred: 1000,
        },
    );

    let stats = h.snapshot();
    assert_eq!(stats.duration_ms, 500); // started 1300 -> completed 1800
    assert_eq!(stats.avg_bytes_per_sec, 2000); // 1000 * 1000 / 500
    assert_eq!(stats.peak_bytes_per_sec, 10_000);
    assert_eq!(stats.connect_latency_ms, Some(100)); // connecting 1100 -> connected 1200
    assert_eq!(
        stats.paths,
        vec![
            DataPath::Relay { url: "r".into() },
            DataPath::Direct { addr }
        ]
    );
}

#[test]
fn stats_use_aggregate_manifest_progress() {
    let manifest_id = ManifestId::new("manifest-stats");
    let h = StatsHandle::new();
    h.observe(
        1_000,
        &TransferEvent::ManifestStarted {
            manifest_id: manifest_id.clone(),
            direction: TransferDirection::Send,
            file_count: 2,
            directory_count: 1,
            total_bytes: 150,
        },
    );
    let progress = |completed_bytes| TransferEvent::ManifestProgress {
        manifest_id: manifest_id.clone(),
        entry_id: 1,
        entry_bytes: completed_bytes,
        entry_total_bytes: 150,
        completed_bytes,
        total_bytes: 150,
    };
    h.observe(1_100, &progress(50));
    h.observe(1_200, &progress(150));
    h.observe(
        1_400,
        &TransferEvent::ManifestCompleted {
            manifest_id,
            file_count: 2,
            directory_count: 1,
            total_bytes: 150,
            entries: Vec::new(),
        },
    );

    let stats = h.snapshot();
    assert_eq!(stats.duration_ms, 400);
    assert_eq!(stats.avg_bytes_per_sec, 375);
    assert_eq!(stats.peak_bytes_per_sec, 1_000);
}
