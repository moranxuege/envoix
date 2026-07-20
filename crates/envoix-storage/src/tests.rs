use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn creates_and_finalizes_temp_destination() {
    let dir = unique_test_dir();
    let final_path = dir.join("hello.txt");

    let (temp_path, mut file) = LocalFileStorage::create_temp_destination(&dir, "hello.txt")
        .await
        .unwrap();
    let text = b"hello";
    file.write_all(text).await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    LocalFileStorage::finalize_temp_file(&temp_path, &final_path)
        .await
        .unwrap();

    assert_eq!(fs::read(&final_path).await.unwrap(), text);
}

#[test]
fn resume_state_without_target_field_still_parses() {
    // Sidecars written before target_file_name existed must keep loading.
    let legacy = r#"{
        "transfer_id": "transfer-1",
        "file_name": "hello.txt",
        "file_size": 11,
        "chunk_size": 4,
        "bytes_received": 4,
        "next_chunk_index": 1,
        "hash_bytes": 4,
        "hash_checkpoint": null
    }"#;
    let state: TransferResumeState = serde_json::from_str(legacy).unwrap();
    assert_eq!(state.target_file_name, None);

    // And a same-name target round-trips without serializing the field.
    let json = serde_json::to_string(&state).unwrap();
    assert!(!json.contains("target_file_name"));
}

#[test]
fn resume_state_with_traversal_target_is_rejected() {
    let state = TransferResumeState {
        transfer_id: TransferId::new("transfer-1"),
        file_name: "hello.txt".into(),
        file_size: 11,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: None,
        target_file_name: Some("../escape.txt".into()),
    };
    assert!(validate_resume_state_name(&state).is_err());
}

#[tokio::test]
async fn rejects_nested_destination_file_name() {
    let dir = unique_test_dir();

    let error = LocalFileStorage::create_temp_destination(&dir, "../hello.txt")
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Storage(_)));
}

#[tokio::test]
async fn legacy_receipt_without_transfer_id_is_ignored() {
    let dir = unique_test_dir();
    fs::create_dir_all(&dir).await.unwrap();
    fs::write(
        receipt_path(&dir, "video.mp4"),
        br#"{"file_name":"video.mp4","file_size":42,"file_hash":"old"}"#,
    )
    .await
    .unwrap();

    assert_eq!(
        LocalFileStorage::read_receipt(&dir, "video.mp4")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn deletes_receipt_only_for_the_exact_transfer() {
    let dir = unique_test_dir();
    let receipt = TransferReceipt {
        transfer_id: TransferId::new("transfer-new"),
        file_name: "video.mp4".into(),
        file_size: 42,
        file_hash: "hash-new".into(),
    };
    LocalFileStorage::write_receipt(&dir, &receipt)
        .await
        .unwrap();

    assert!(
        !LocalFileStorage::delete_receipt_for_transfer(
            &dir,
            &receipt.file_name,
            &TransferId::new("transfer-old"),
        )
        .await
        .unwrap()
    );
    assert_eq!(
        LocalFileStorage::read_receipt(&dir, &receipt.file_name)
            .await
            .unwrap(),
        Some(receipt.clone())
    );

    assert!(
        LocalFileStorage::delete_receipt_for_transfer(
            &dir,
            &receipt.file_name,
            &receipt.transfer_id,
        )
        .await
        .unwrap()
    );
    assert_eq!(
        LocalFileStorage::read_receipt(&dir, &receipt.file_name)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn writes_reads_updates_and_deletes_resume_state() {
    let dir = unique_test_dir();
    let state = TransferResumeState {
        transfer_id: TransferId::new("transfer-1"),
        file_name: "hello.txt".into(),
        file_size: 11,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: Some("abc123".into()),
        target_file_name: None,
    };

    LocalFileStorage::write_resume_state(&dir, &state)
        .await
        .unwrap();
    assert_eq!(
        LocalFileStorage::read_resume_state(&dir, "hello.txt", &state.transfer_id)
            .await
            .unwrap(),
        Some(state.clone())
    );

    let mut updated = state.clone();
    updated.bytes_received = 8;
    updated.next_chunk_index = 2;
    LocalFileStorage::write_resume_state(&dir, &updated)
        .await
        .unwrap();
    assert_eq!(
        LocalFileStorage::read_resume_state(&dir, "hello.txt", &state.transfer_id)
            .await
            .unwrap(),
        Some(updated.clone())
    );

    LocalFileStorage::delete_resume_state(&dir, "hello.txt", &state.transfer_id)
        .await
        .unwrap();
    assert_eq!(
        LocalFileStorage::read_resume_state(&dir, "hello.txt", &state.transfer_id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn opens_deterministic_resume_temp_for_append() {
    let dir = unique_test_dir();
    let state = TransferResumeState {
        transfer_id: TransferId::new("transfer-1"),
        file_name: "hello.txt".into(),
        file_size: 11,
        chunk_size: 4,
        bytes_received: 0,
        next_chunk_index: 0,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    };

    let (temp_path, mut file) = LocalFileStorage::open_resumable_destination(&dir, &state)
        .await
        .unwrap();
    file.write_all(b"hello").await.unwrap();
    drop(file);

    let (second_temp_path, mut file) =
        LocalFileStorage::open_resumable_destination(&dir, &state)
            .await
            .unwrap();
    file.write_all(b" world").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    assert_eq!(second_temp_path, temp_path);
    assert_eq!(fs::read(temp_path).await.unwrap(), b"hello world");
}

#[tokio::test]
async fn finds_only_envoix_resume_sidecars_for_file() {
    let dir = unique_test_dir();
    fs::create_dir_all(&dir).await.unwrap();
    fs::write(dir.join("notes.json"), b"{not json")
        .await
        .unwrap();
    fs::write(
        dir.join(".other.txt.transfer-1.json"),
        br#"{"file_name":"hello.txt"}"#,
    )
    .await
    .unwrap();
    let state = TransferResumeState {
        transfer_id: TransferId::new("transfer-1"),
        file_name: "hello.txt".into(),
        file_size: 11,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: Some("abc123".into()),
        target_file_name: None,
    };
    LocalFileStorage::write_resume_state(&dir, &state)
        .await
        .unwrap();

    assert_eq!(
        LocalFileStorage::find_resume_state(&dir, "hello.txt", 11, 4)
            .await
            .unwrap(),
        Some(state)
    );
}

#[tokio::test]
async fn find_resume_state_prefers_most_advanced_sidecar() {
    let dir = unique_test_dir();
    let stale = TransferResumeState {
        transfer_id: TransferId::new("transfer-stale"),
        file_name: "movie.mkv".into(),
        file_size: 1024,
        chunk_size: 64,
        bytes_received: 0,
        next_chunk_index: 0,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    };
    let advanced = TransferResumeState {
        transfer_id: TransferId::new("transfer-advanced"),
        file_name: "movie.mkv".into(),
        file_size: 1024,
        chunk_size: 64,
        bytes_received: 512,
        next_chunk_index: 8,
        hash_bytes: 512,
        hash_checkpoint: Some("abc123".into()),
        target_file_name: None,
    };

    LocalFileStorage::write_resume_state(&dir, &stale)
        .await
        .unwrap();
    LocalFileStorage::write_resume_state(&dir, &advanced)
        .await
        .unwrap();

    assert_eq!(
        LocalFileStorage::find_resume_state(&dir, "movie.mkv", 1024, 64)
            .await
            .unwrap(),
        Some(advanced)
    );
}

#[test]
fn resume_lease_is_exclusive_rebindable_and_released_on_drop() {
    let dir = unique_test_dir();
    let first_id = TransferId::new("transfer-first");
    let second_id = TransferId::new("transfer-second");
    let mut lease = LocalFileStorage::try_acquire_resume_lease(&dir, "movie.mkv", &first_id)
        .unwrap()
        .expect("first owner should acquire the lease");

    assert!(
        LocalFileStorage::try_acquire_resume_lease(&dir, "movie.mkv", &first_id)
            .unwrap()
            .is_none()
    );

    lease.rebind(&dir, "movie.mkv", &second_id).unwrap();
    assert!(
        LocalFileStorage::try_acquire_resume_lease(&dir, "movie.mkv", &first_id)
            .unwrap()
            .is_some()
    );
    assert!(
        LocalFileStorage::try_acquire_resume_lease(&dir, "movie.mkv", &second_id)
            .unwrap()
            .is_none()
    );

    drop(lease);
    assert!(
        LocalFileStorage::try_acquire_resume_lease(&dir, "movie.mkv", &second_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn stale_cleanup_preserves_active_partial_and_receipt() {
    let dir = unique_test_dir();
    let stale = TransferResumeState {
        transfer_id: TransferId::new("stale-transfer"),
        file_name: "stale.bin".into(),
        file_size: 4,
        chunk_size: 4,
        bytes_received: 4,
        next_chunk_index: 1,
        hash_bytes: 4,
        hash_checkpoint: None,
        target_file_name: None,
    };
    let active = TransferResumeState {
        transfer_id: TransferId::new("active-transfer"),
        file_name: "active.bin".into(),
        ..stale.clone()
    };
    LocalFileStorage::write_resume_state(&dir, &stale)
        .await
        .unwrap();
    LocalFileStorage::write_resume_state(&dir, &active)
        .await
        .unwrap();
    let stale_temp =
        LocalFileStorage::resumable_temp_path(&dir, &stale.file_name, &stale.transfer_id)
            .unwrap();
    let active_temp =
        LocalFileStorage::resumable_temp_path(&dir, &active.file_name, &active.transfer_id)
            .unwrap();
    fs::write(&stale_temp, b"old!").await.unwrap();
    fs::write(&active_temp, b"live").await.unwrap();
    let receipt = TransferReceipt {
        transfer_id: TransferId::new("completed-transfer"),
        file_name: "done.bin".into(),
        file_size: 4,
        file_hash: "hash".into(),
    };
    LocalFileStorage::write_receipt(&dir, &receipt)
        .await
        .unwrap();
    let _lease = LocalFileStorage::try_acquire_resume_lease(
        &dir,
        &active.file_name,
        &active.transfer_id,
    )
    .unwrap()
    .unwrap();

    let report = LocalFileStorage::cleanup_stale_resume_artifacts(&dir, Duration::ZERO)
        .await
        .unwrap();

    assert_eq!(report.files_deleted, 2);
    assert!(!stale_temp.exists());
    assert!(active_temp.exists());
    assert!(
        LocalFileStorage::read_resume_state(&dir, &active.file_name, &active.transfer_id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        LocalFileStorage::read_receipt(&dir, &receipt.file_name)
            .await
            .unwrap(),
        Some(receipt)
    );
}

struct TestDir(tempfile::TempDir);

impl std::ops::Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

fn unique_test_dir() -> TestDir {
    TestDir(
        tempfile::Builder::new()
            .prefix("envoix-storage-test-")
            .tempdir()
            .unwrap(),
    )
}
