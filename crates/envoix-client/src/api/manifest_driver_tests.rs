use super::*;
use std::path::Path;

use super::super::driver::ClientContext;
use envoix_protocol::{ManifestEntryV1, ManifestHashAlgorithm, ManifestId, ManifestV1};
use envoix_session::ManifestSendRequest;
use envoix_types::TransferId;
use tempfile::tempdir;

fn send_context(source: &Path) -> ManifestSessionContext {
    let manifest = manifest();
    ManifestSessionContext {
        client: ClientContext::default(),
        params: super::super::manifest_activity::ManifestSessionParams {
            operation: ManifestOperation::Send {
                request: ManifestSendRequest::new(manifest, [(0, source.to_path_buf())])
                    .unwrap(),
            },
            sources: vec![PeerSource::ShowManual {
                token: Some("stable-test-token".into()),
            }],
            options: super::super::TransferOptions::default(),
            publication_required: false,
        },
    }
}

fn manifest() -> ManifestV1 {
    ManifestV1 {
        manifest_id: ManifestId::new("driver-manifest"),
        entries: vec![ManifestEntryV1 {
            entry_id: 0,
            relative_path: "photo.jpg".into(),
            kind: ManifestEntryKind::RegularFile,
            size: 3,
            hash: Some([1; 32]),
            modified_at_unix_ms: None,
        }],
        file_count: 1,
        directory_count: 0,
        root_count: 1,
        total_bytes: 3,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    }
}

fn receive_context(output_dir: &Path) -> ManifestSessionContext {
    ManifestSessionContext {
        client: ClientContext::default(),
        params: super::super::manifest_activity::ManifestSessionParams {
            operation: ManifestOperation::Receive {
                output_dir: output_dir.to_path_buf(),
            },
            sources: vec![PeerSource::ShowManual {
                token: Some("stable-test-token".into()),
            }],
            options: super::super::TransferOptions::default(),
            publication_required: true,
        },
    }
}

async fn next_snapshot(
    notices: &mut mpsc::UnboundedReceiver<ManifestSessionNotice>,
) -> ManifestSessionSnapshot {
    loop {
        match notices.recv().await.unwrap() {
            ManifestSessionNotice::Snapshot(snapshot) => return snapshot,
            ManifestSessionNotice::Event(_) => {}
        }
    }
}

#[tokio::test]
async fn synchronous_launch_failure_is_persisted() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("photo.jpg");
    tokio::fs::write(&source, b"jpg").await.unwrap();
    let store = ManifestRecordStore::new(temp.path().join("records"));
    let (_session, mut notices) =
        ManifestTransferSession::start(send_context(&source), Some((store.clone(), 9)), None)
            .unwrap();

    let first = next_snapshot(&mut notices).await;
    assert_eq!(first.activity.session.state, State::Connecting);
    let failed = next_snapshot(&mut notices).await;
    assert_eq!(failed.activity.session.state, State::Failed);
    assert!(failed.activity.session.failure.is_some());
    let persisted = store.load(9).await.unwrap();
    assert_eq!(persisted.activity.session.state, State::Failed);
}

#[tokio::test]
async fn restore_parks_an_active_attempt_without_launching() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("photo.jpg");
    tokio::fs::write(&source, b"jpg").await.unwrap();
    let context = send_context(&source);
    let activity = ManifestActivity::new(&context).unwrap();
    let record = new_manifest_record(4, context, activity, None);
    let store = ManifestRecordStore::new(temp.path().join("records"));

    let (_session, mut notices) =
        ManifestTransferSession::restore(record, Some((store.clone(), 4))).unwrap();
    let snapshot = next_snapshot(&mut notices).await;

    assert_eq!(
        snapshot.activity.session.state,
        State::Paused(PauseOrigin::Lost)
    );
    assert_eq!(
        store.load(4).await.unwrap().activity.session.state,
        State::Paused(PauseOrigin::Lost)
    );
}

#[tokio::test]
async fn negotiated_single_file_becomes_one_entry_manifest_activity() {
    let temp = tempdir().unwrap();
    let bytes = b"legacy";
    let final_path = temp.path().join("legacy.txt");
    tokio::fs::write(&final_path, bytes).await.unwrap();
    let context = receive_context(temp.path());
    let activity = ManifestActivity::new(&context).unwrap();
    let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (notice_tx, _notice_rx) = mpsc::unbounded_channel();
    let mut actor = Actor {
        client: context.client.client().unwrap(),
        context,
        activity,
        cmds: cmd_rx,
        notices: notice_tx,
        current: None,
        pending_run_end: None,
        seq: 0,
        rate: RateTracker::default(),
        last_progress_snapshot: None,
        created_ms: 1,
        record: None,
        platform_extras: None,
        staged: Vec::new(),
        commit_failures: 0,
        commit_retry_at: None,
        launch: false,
    };

    let (effects, progress_only) = actor
        .process_manifest_event(TransferEvent::Started {
            transfer_id: TransferId::new("legacy-transfer"),
            direction: TransferDirection::Receive,
            file_name: "legacy.txt".into(),
            total_bytes: bytes.len() as u64,
            bytes_resumed: 0,
        })
        .unwrap();
    assert!(effects.is_empty());
    assert!(!progress_only);
    assert_eq!(actor.activity.session.state, State::Transferring);
    assert_eq!(
        actor.activity.session.file_name.as_deref(),
        Some("legacy.txt")
    );

    let (_, progress_only) = actor
        .process_manifest_event(TransferEvent::Progress {
            transfer_id: TransferId::new("legacy-transfer"),
            bytes_transferred: bytes.len() as u64,
            total_bytes: bytes.len() as u64,
        })
        .unwrap();
    assert!(progress_only);
    assert_eq!(actor.activity.session.bytes, bytes.len() as u64);

    // The transfer engine already verified these bytes. Projection must
    // consume that proof instead of reopening the completed file.
    tokio::fs::remove_file(final_path).await.unwrap();

    let effects = actor
        .adopt_compatible_single_file(TransferSummary {
            transfer_id: TransferId::new("legacy-transfer"),
            file_name: "legacy.txt".into(),
            bytes_transferred: bytes.len() as u64,
            file_hash: blake3::hash(bytes).to_hex().to_string(),
        })
        .await
        .unwrap();

    assert!(effects.contains(&Effect::PostReceipt));
    assert_eq!(actor.activity.session.state, State::AwaitingPublication);
    assert_eq!(
        actor.activity.session.completed_file_path,
        temp.path().to_str().map(ToOwned::to_owned)
    );
    let manifest = actor.activity.manifest.as_ref().unwrap();
    assert_eq!(manifest.manifest_id.to_string(), "legacy-transfer");
    assert_eq!(manifest.file_count, 1);
    assert_eq!(manifest.directory_count, 0);
    assert_eq!(manifest.total_bytes, 6);
    assert_eq!(manifest.entries[0].relative_path, "legacy.txt");
    assert_eq!(
        manifest.entries[0].hash,
        Some(*blake3::hash(bytes).as_bytes())
    );
    assert_eq!(actor.activity.completed_files, 1);
    assert_eq!(actor.activity.entry_results.len(), 1);
    assert_eq!(
        actor.activity.entry_results[0].status,
        ManifestEntryResultStatus::Completed
    );
    assert_eq!(actor.activity.session.bytes_resumed, 0);
}

#[tokio::test]
async fn negotiated_existing_single_file_preserves_resume_accounting() {
    let temp = tempdir().unwrap();
    let bytes = b"already present";
    let context = receive_context(temp.path());
    let activity = ManifestActivity::new(&context).unwrap();
    let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (notice_tx, _notice_rx) = mpsc::unbounded_channel();
    let mut actor = Actor {
        client: context.client.client().unwrap(),
        context,
        activity,
        cmds: cmd_rx,
        notices: notice_tx,
        current: None,
        pending_run_end: None,
        seq: 0,
        rate: RateTracker::default(),
        last_progress_snapshot: None,
        created_ms: 1,
        record: None,
        platform_extras: None,
        staged: Vec::new(),
        commit_failures: 0,
        commit_retry_at: None,
        launch: false,
    };

    actor
        .process_manifest_event(TransferEvent::Verifying {
            transfer_id: TransferId::new("existing-transfer"),
            direction: TransferDirection::Receive,
            file_name: "existing.txt".into(),
            bytes_to_hash: bytes.len() as u64,
        })
        .unwrap();
    actor
        .process_manifest_event(TransferEvent::Verified {
            transfer_id: TransferId::new("existing-transfer"),
            direction: TransferDirection::Receive,
            file_name: "existing.txt".into(),
            bytes_hashed: bytes.len() as u64,
        })
        .unwrap();

    actor
        .adopt_compatible_single_file(TransferSummary {
            transfer_id: TransferId::new("existing-transfer"),
            file_name: "existing.txt".into(),
            bytes_transferred: bytes.len() as u64,
            file_hash: blake3::hash(bytes).to_hex().to_string(),
        })
        .await
        .unwrap();

    assert_eq!(actor.activity.session.bytes, bytes.len() as u64);
    assert_eq!(actor.activity.session.total, bytes.len() as u64);
    assert_eq!(actor.activity.session.bytes_resumed, bytes.len() as u64);
}

#[test]
fn manifest_events_reduce_to_one_publication_gated_activity() {
    let context = receive_context(Path::new("/tmp/manifest-driver-events"));
    let activity = ManifestActivity::new(&context).unwrap();
    let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (notice_tx, _notice_rx) = mpsc::unbounded_channel();
    let mut actor = Actor {
        client: context.client.client().unwrap(),
        context,
        activity,
        cmds: cmd_rx,
        notices: notice_tx,
        current: None,
        pending_run_end: None,
        seq: 0,
        rate: RateTracker::default(),
        last_progress_snapshot: None,
        created_ms: 1,
        record: None,
        platform_extras: None,
        staged: Vec::new(),
        commit_failures: 0,
        commit_retry_at: None,
        launch: false,
    };
    let plan = manifest();
    actor
        .process_manifest_event(TransferEvent::ManifestPlanned {
            direction: TransferDirection::Receive,
            manifest: plan.clone(),
        })
        .unwrap();
    actor
        .process_manifest_event(TransferEvent::ManifestStarted {
            manifest_id: plan.manifest_id.clone(),
            direction: TransferDirection::Receive,
            file_count: 1,
            directory_count: 0,
            total_bytes: 3,
        })
        .unwrap();
    actor
        .process_manifest_event(TransferEvent::ManifestEntryStarted {
            manifest_id: plan.manifest_id.clone(),
            entry_id: 0,
            transfer_id: TransferId::new("driver-entry"),
            relative_path: "photo.jpg".into(),
            total_bytes: 3,
            bytes_resumed: 0,
        })
        .unwrap();
    actor
        .process_manifest_event(TransferEvent::ManifestProgress {
            manifest_id: plan.manifest_id.clone(),
            entry_id: 0,
            entry_bytes: 3,
            entry_total_bytes: 3,
            completed_bytes: 3,
            total_bytes: 3,
        })
        .unwrap();
    let result = envoix_protocol::ManifestEntryResultV1 {
        entry_id: 0,
        status: ManifestEntryResultStatus::Completed,
        offered_relative_path: "photo.jpg".into(),
        final_relative_path: Some("photo.jpg".into()),
        failure_code: None,
    };
    actor
        .process_manifest_event(TransferEvent::ManifestEntryCompleted {
            manifest_id: plan.manifest_id.clone(),
            result: result.clone(),
        })
        .unwrap();
    actor
        .process_manifest_event(TransferEvent::ManifestCompleted {
            manifest_id: plan.manifest_id,
            file_count: 1,
            directory_count: 0,
            total_bytes: 3,
            entries: vec![result],
        })
        .unwrap();

    assert_eq!(actor.activity.session.state, State::AwaitingPublication);
    assert_eq!(actor.activity.session.bytes, 3);
    assert_eq!(actor.activity.completed_files, 1);
}
