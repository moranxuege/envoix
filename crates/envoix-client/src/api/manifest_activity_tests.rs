use super::*;
use envoix_protocol::{ManifestEntryV1, ManifestHashAlgorithm, ManifestId};
use tempfile::tempdir;

fn manifest() -> ManifestV1 {
    ManifestV1 {
        manifest_id: ManifestId::new("durable-manifest"),
        entries: vec![
            ManifestEntryV1 {
                entry_id: 0,
                relative_path: "Album".into(),
                kind: ManifestEntryKind::Directory,
                size: 0,
                hash: None,
                modified_at_unix_ms: None,
            },
            ManifestEntryV1 {
                entry_id: 1,
                relative_path: "Album/a.jpg".into(),
                kind: ManifestEntryKind::RegularFile,
                size: 10,
                hash: Some([1; 32]),
                modified_at_unix_ms: None,
            },
            ManifestEntryV1 {
                entry_id: 2,
                relative_path: "b.txt".into(),
                kind: ManifestEntryKind::RegularFile,
                size: 5,
                hash: Some([2; 32]),
                modified_at_unix_ms: None,
            },
        ],
        file_count: 2,
        directory_count: 1,
        root_count: 2,
        total_bytes: 15,
        hash_algorithm: ManifestHashAlgorithm::Blake3_256,
    }
}

fn receive_context(publication_required: bool) -> ManifestSessionContext {
    ManifestSessionContext {
        client: ClientContext::default(),
        params: ManifestSessionParams {
            operation: ManifestOperation::Receive {
                output_dir: "/tmp/envoix-manifest-staging".into(),
            },
            sources: vec![PeerSource::ShowManual {
                token: Some("token".into()),
            }],
            options: TransferOptions::default(),
            publication_required,
        },
    }
}

fn result(entry_id: u32, path: &str) -> ManifestEntryResultV1 {
    ManifestEntryResultV1 {
        entry_id,
        status: ManifestEntryResultStatus::Completed,
        offered_relative_path: path.into(),
        final_relative_path: Some(path.into()),
        failure_code: None,
    }
}

#[test]
fn publication_gates_aggregate_completion() {
    let context = receive_context(true);
    let mut activity = ManifestActivity::new(&context).unwrap();
    let plan = manifest();
    activity
        .accept_plan(TransferDirection::Receive, plan.clone())
        .unwrap();
    activity.started().unwrap();
    activity
        .entry_started(1, "entry-1".into(), "Album/a.jpg".into(), 10, 2)
        .unwrap();
    activity.progress(1, 8, 8);
    activity.entry_completed(result(0, "Album")).unwrap();
    activity.entry_completed(result(1, "Album/a.jpg")).unwrap();
    activity.entry_completed(result(2, "b.txt")).unwrap();
    let summary = ManifestTransferSummary {
        manifest_id: plan.manifest_id,
        file_count: 2,
        directory_count: 1,
        total_bytes: 15,
        entries: activity.entry_results.clone(),
    };
    activity
        .completed(summary, Some("/tmp/envoix-manifest-staging".into()))
        .unwrap();

    assert_eq!(activity.session.state, State::AwaitingPublication);
    assert_eq!(activity.completed_files, 2);
    assert_eq!(activity.root_count(), 2);
    activity.session.reduce(Input::Published {
        path: "file:///Downloads/Envoix".into(),
    });
    assert_eq!(activity.session.state, State::Completed);
}

#[test]
fn cancel_preserves_committed_results_and_marks_only_unfinished_entries() {
    let context = receive_context(false);
    let mut activity = ManifestActivity::new(&context).unwrap();
    activity
        .accept_plan(TransferDirection::Receive, manifest())
        .unwrap();
    activity.started().unwrap();
    activity.entry_completed(result(0, "Album")).unwrap();
    activity.entry_completed(result(1, "Album/a.jpg")).unwrap();
    activity
        .entry_started(2, "entry-2".into(), "b.txt".into(), 5, 0)
        .unwrap();

    activity.cancel_unfinished();

    assert_eq!(activity.session.state, State::Cancelled);
    assert_eq!(activity.entry_results.len(), 3);
    assert_eq!(activity.completed_files, 1);
    assert_eq!(
        activity.entry_results[1].status,
        ManifestEntryResultStatus::Completed
    );
    assert_eq!(
        activity.entry_results[2].status,
        ManifestEntryResultStatus::Cancelled
    );
}

#[test]
fn partial_result_cannot_report_aggregate_completion() {
    let context = receive_context(false);
    let mut activity = ManifestActivity::new(&context).unwrap();
    let plan = manifest();
    activity
        .accept_plan(TransferDirection::Receive, plan.clone())
        .unwrap();
    activity.started().unwrap();
    let failed = ManifestEntryResultV1 {
        entry_id: 1,
        status: ManifestEntryResultStatus::Failed,
        offered_relative_path: "Album/a.jpg".into(),
        final_relative_path: None,
        failure_code: Some("manifest.receive_failed".into()),
    };
    let summary = ManifestTransferSummary {
        manifest_id: plan.manifest_id,
        file_count: 2,
        directory_count: 1,
        total_bytes: 15,
        entries: vec![result(0, "Album"), failed, result(2, "b.txt")],
    };

    assert!(activity.completed(summary, None).is_err());
    assert_eq!(activity.session.state, State::Transferring);
}

#[tokio::test]
async fn record_store_round_trips_manifest_facts() {
    let dir = tempdir().unwrap();
    let store = ManifestRecordStore::new(dir.path());
    let context = receive_context(true);
    let mut activity = ManifestActivity::new(&context).unwrap();
    activity
        .accept_plan(TransferDirection::Receive, manifest())
        .unwrap();
    activity
        .preparing_entry(1, "Album/a.jpg".into(), 10)
        .unwrap();
    let record = new_manifest_record(7, context, activity, Some(serde_json::json!({"ui": 1})));

    let mut invalid = record.clone();
    invalid.activity.completed_files = 9;
    assert!(store.save(&invalid).await.is_err());

    store.save(&record).await.unwrap();
    let loaded = store.load(7).await.unwrap();
    assert_eq!(loaded.activity.protocol, TransferProtocol::ManifestV1);
    assert_eq!(loaded.activity.root_count(), 2);
    assert_eq!(loaded.activity.current_entry.unwrap().entry_id, 1);
    assert_eq!(loaded.platform_extras, Some(serde_json::json!({"ui": 1})));
    assert_eq!(store.load_all().await.len(), 1);

    store.delete(7).await;
    assert!(store.load(7).await.is_none());
}

#[test]
fn deserialized_send_request_is_revalidated() {
    let request = ManifestSendRequest::new(
        manifest(),
        [
            (1, PathBuf::from("/private/a.jpg")),
            (2, PathBuf::from("/private/b.txt")),
        ],
    )
    .unwrap();
    let context = ManifestSessionContext {
        client: ClientContext::default(),
        params: ManifestSessionParams {
            operation: ManifestOperation::Send { request },
            sources: vec![PeerSource::Mdns {
                token: Some("token".into()),
            }],
            options: TransferOptions::default(),
            publication_required: false,
        },
    };
    let mut value = serde_json::to_value(context).unwrap();
    value["params"]["operation"]["request"]["source_paths"] = serde_json::json!({});
    let context: ManifestSessionContext = serde_json::from_value(value).unwrap();
    assert!(context.validate().is_err());
}
