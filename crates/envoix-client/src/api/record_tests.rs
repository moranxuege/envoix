use super::super::machine::State;
use super::*;
use envoix_session::TransferDirection;

fn record(id: u64) -> TransferRecord {
    TransferRecord {
        version: RECORD_VERSION,
        id,
        created_ms: 1,
        updated_ms: 1,
        platform_extras: None,
        context: SessionContext {
            client: Default::default(),
            params: SessionParams {
                direction: TransferDirection::Receive,
                path: "/tmp/x".into(),
                sources: vec![super::super::PeerSource::Room {
                    code: "123456-kelp-coral".into(),
                    broker: "id@1.2.3.4:5".into(),
                }],
                options: super::super::TransferOptions::default(),
                publication_required: false,
            },
        },
        session: Session::new(TransferDirection::Receive),
    }
}

#[tokio::test]
async fn save_load_delete_round_trip() {
    let dir = std::env::temp_dir().join(format!("envoix-records-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let store = RecordStore::new(&dir);

    let mut r = record(7);
    r.session.state = State::Unconfirmed;
    r.session.transfer_id = Some("transfer-x".into());
    store.save(&r).await.unwrap();
    store.save(&record(3)).await.unwrap();

    let loaded = store.load_all().await;
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, 3, "sorted by id");
    assert_eq!(loaded[1].session.state, State::Unconfirmed);
    assert_eq!(loaded[1].session.transfer_id.as_deref(), Some("transfer-x"),);
    assert_eq!(
        loaded[1].context.params.sources,
        record(7).context.params.sources,
        "relaunch context survives"
    );

    store.delete(3).await;
    assert_eq!(store.load_all().await.len(), 1);

    // A corrupt file is skipped, never fatal.
    tokio::fs::write(dir.join("record-9.json"), b"{nope")
        .await
        .unwrap();
    assert_eq!(store.load_all().await.len(), 1);
}

#[test]
fn restore_context_summarizes_the_typed_record() {
    let mut r = record(5); // a Room receive by default
    r.context.params.path = "/out/dir".into();
    let ctx = r.restore_context();
    assert_eq!(ctx.id, 5);
    assert_eq!(ctx.direction, "receive");
    assert_eq!(ctx.code, "123456-kelp-coral");
    assert_eq!(ctx.path, "/out/dir");
    assert!(ctx.use_room);
    assert!(!ctx.use_mdns);
}

#[test]
fn source_ready_migrates_from_state_for_legacy_records() {
    // A pre-v2 record lacks source_ready; the migration derives it from
    // state (a bare serde default of false would wrongly re-stage every
    // past-staging record). Serialize a legacy record with a deliberately
    // WRONG source_ready and confirm the migration overrides it.
    let migrated = |state: State, extras: Option<serde_json::Value>| -> bool {
        let mut r = record(1);
        r.version = 0; // legacy
        r.session = Session::new(TransferDirection::Send);
        r.session.state = state;
        r.session.facts.source_ready = true; // wrong on purpose
        r.platform_extras = extras;
        let json = serde_json::to_string(&r).unwrap();
        serde_json::from_str::<TransferRecord>(&json)
            .unwrap()
            .session
            .facts
            .source_ready
    };
    let staged = || Some(serde_json::json!({ "source_uri": "content://x" }));
    assert!(!migrated(State::Preparing, None), "Preparing -> not ready");
    assert!(migrated(State::Connecting, None), "past staging -> ready");
    assert!(migrated(State::Completed, None), "completed -> ready");
    assert!(
        !migrated(State::Cancelled, staged()),
        "cancelled staged -> re-stage",
    );
    assert!(
        migrated(State::Cancelled, None),
        "cancelled direct -> ready"
    );
}

#[test]
fn restore_context_needs_no_frontend_migration_for_legacy_records() {
    // A pre-context record (params at the top level) deserializes via the
    // typed migration, so restore_context reads it with no fallback - the
    // whole reason the frontend can drop its `context ?: params` dance.
    let mut value = serde_json::to_value(record(9)).unwrap();
    let object = value.as_object_mut().unwrap();
    let context = object.remove("context").unwrap();
    object.insert("params".into(), context["params"].clone());
    let loaded: TransferRecord = serde_json::from_value(value).unwrap();

    let ctx = loaded.restore_context();
    assert_eq!(ctx.id, 9);
    assert_eq!(ctx.code, "123456-kelp-coral");
    assert!(ctx.use_room);
}

#[tokio::test]
async fn discard_record_cleans_artifacts_without_a_live_session() {
    use envoix_storage::{LocalFileStorage, TransferReceipt, TransferResumeState};
    let dir = std::env::temp_dir().join(format!("envoix-discard-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let store = RecordStore::new(dir.join("records"));

    // A paused receive: record + partial + state + receipt on disk.
    let mut r = record(4);
    r.context.params.path = dir.clone();
    r.session.file_name = Some("f.bin".into());
    r.session.transfer_id = Some("t-1".into());
    store.save(&r).await.unwrap();
    let tid = envoix_types::TransferId::new("t-1");
    let temp = LocalFileStorage::resumable_temp_path(&dir, "f.bin", &tid).unwrap();
    tokio::fs::write(&temp, b"partial").await.unwrap();
    LocalFileStorage::write_resume_state(
        &dir,
        &TransferResumeState {
            transfer_id: tid.clone(),
            file_name: "f.bin".into(),
            file_size: 100,
            chunk_size: 10,
            bytes_received: 7,
            next_chunk_index: 1,
            hash_bytes: 7,
            hash_checkpoint: None,
            target_file_name: None,
        },
    )
    .await
    .unwrap();
    LocalFileStorage::write_receipt(
        &dir,
        &TransferReceipt {
            transfer_id: tid.clone(),
            file_name: "f.bin".into(),
            file_size: 100,
            file_hash: "h".into(),
        },
    )
    .await
    .unwrap();

    // No live session anywhere: Remove still cleans everything.
    discard_record(&store, 4).await;

    assert!(store.load(4).await.is_none(), "record deleted");
    assert!(
        !tokio::fs::try_exists(&temp).await.unwrap(),
        "partial deleted"
    );
    assert!(
        LocalFileStorage::read_receipt(&dir, "f.bin")
            .await
            .unwrap()
            .is_none(),
        "receipt deleted"
    );
    assert!(
        LocalFileStorage::find_resume_state(&dir, "f.bin", 100, 10)
            .await
            .unwrap()
            .is_none(),
        "state deleted"
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn platform_extras_survive_the_round_trip() {
    let dir = std::env::temp_dir().join(format!("envoix-extras-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let store = RecordStore::new(&dir);
    let mut r = record(11);
    r.platform_extras =
        Some(serde_json::json!({"qr": "envoix:abc", "saved_uri": "content://x"}));
    store.save(&r).await.unwrap();

    let loaded = store.load(11).await.unwrap();
    assert_eq!(loaded.version, RECORD_VERSION);
    assert_eq!(
        loaded.platform_extras.unwrap()["qr"],
        serde_json::json!("envoix:abc"),
        "the core persists the frontend's context verbatim"
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn legacy_string_id_migrates_without_duplicate_cards() {
    let dir = std::env::temp_dir().join(format!("envoix-string-id-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let store = RecordStore::new(&dir);
    let external_id = "activity-550e8400-e29b-41d4-a716-446655440000";
    let mut value = serde_json::to_value(record(12)).unwrap();
    value["id"] = external_id.into();
    tokio::fs::write(
        dir.join(format!("record-{external_id}.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .await
    .unwrap();

    let loaded = store.load_all().await;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, stable_record_id(external_id));
    assert_eq!(external_record_id(&loaded[0]), Some(external_id));

    store.save(&loaded[0]).await.unwrap();
    assert!(
        !dir.join(format!("record-{external_id}.json")).exists(),
        "saving the adapted record removes the legacy filename"
    );
    assert_eq!(store.load_all().await.len(), 1);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[test]
fn deserialize_legacy_params_record_as_default_context() {
    let mut value = serde_json::to_value(record(9)).unwrap();
    let object = value.as_object_mut().unwrap();
    let context = object.remove("context").unwrap();
    object.insert("params".into(), context["params"].clone());

    let loaded: TransferRecord = serde_json::from_value(value).unwrap();

    assert_eq!(loaded.id, 9);
    assert_eq!(loaded.context.client.chunk_size, None);
    assert_eq!(loaded.context.params.path, PathBuf::from("/tmp/x"));
}
