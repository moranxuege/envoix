//! Local file and transfer-state storage.

use std::path::{Component, Path, PathBuf};

use envoix_error::CoreError;
use envoix_types::TransferId;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;

/// Error type returned by local storage operations.
pub type StorageError = CoreError;

/// Filesystem-backed storage used by the current transfer engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFileStorage;

/// Durable proof that a transfer completed: written beside the final file on
/// finalize and kept after the file itself is moved or published away (e.g.
/// Android publishes to MediaStore and deletes the output copy). A later
/// re-offer of the same file matches against it, so the receiver can re-confirm
/// completion — re-deliver a lost CompleteAck — without the file on hand and
/// without any bytes being re-sent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransferReceipt {
    /// Plain destination file name, without path components.
    pub file_name: String,
    /// Final file length in bytes.
    pub file_size: u64,
    /// BLAKE3 hash of the completed file (as verified at finalize).
    pub file_hash: String,
}

/// Durable receiver-side state used to resume an interrupted transfer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransferResumeState {
    /// Transfer identifier for the current receiver-side temp and state files.
    pub transfer_id: TransferId,
    /// Plain destination file name, without path components.
    pub file_name: String,
    /// Expected final file length in bytes.
    pub file_size: u64,
    /// Chunk size declared by the sender for this transfer.
    pub chunk_size: u64,
    /// Number of plaintext bytes already persisted in the temp file.
    pub bytes_received: u64,
    /// Next sequential chunk index expected from the sender.
    pub next_chunk_index: u64,
    /// Number of temp-file bytes included in `hash_checkpoint`.
    pub hash_bytes: u64,
    /// Informational BLAKE3 checkpoint for debugging; never trusted for resume.
    pub hash_checkpoint: Option<String>,
    /// Local file name this transfer is landing under, when it differs from
    /// `file_name`: a fresh (`resume_requested = false`) re-receive lands
    /// beside an existing same-name final instead of being answered by it.
    /// `None` means the transfer lands under `file_name` itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_file_name: Option<String>,
}

impl LocalFileStorage {
    /// Opens a source file for reading.
    pub async fn open_source(path: &Path) -> Result<File, StorageError> {
        File::open(path).await.map_err(CoreError::from)
    }

    /// Creates a non-resumable temp destination for a new file.
    pub async fn create_temp_destination(
        output_dir: &Path,
        file_name: &str,
    ) -> Result<(PathBuf, File), StorageError> {
        if !is_plain_file_name(file_name) {
            return Err(CoreError::Storage(format!(
                "invalid output file name: {file_name}"
            )));
        }

        fs::create_dir_all(output_dir).await?;

        let temp_path = output_dir.join(format!(".{file_name}.part"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await?;

        Ok((temp_path, file))
    }

    /// Opens the deterministic resumable temp file in append mode.
    pub async fn open_resumable_destination(
        output_dir: &Path,
        state: &TransferResumeState,
    ) -> Result<(PathBuf, File), StorageError> {
        validate_resume_state_name(state)?;
        fs::create_dir_all(output_dir).await?;

        let temp_path = resumable_temp_path(output_dir, &state.file_name, &state.transfer_id);
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&temp_path)
            .await?;

        Ok((temp_path, file))
    }

    /// Renames a verified temp file to its final destination.
    pub async fn finalize_temp_file(
        temp_path: &Path,
        final_path: &Path,
    ) -> Result<(), StorageError> {
        if fs::try_exists(final_path).await? {
            return Err(CoreError::Storage(format!(
                "destination already exists: {}",
                final_path.display()
            )));
        }

        fs::rename(temp_path, final_path).await?;
        Ok(())
    }

    /// Reads the JSON sidecar state for a resumable transfer, if present.
    pub async fn read_resume_state(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<Option<TransferResumeState>, StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        let state_path = resumable_state_path(output_dir, file_name, transfer_id);

        if !fs::try_exists(&state_path).await? {
            return Ok(None);
        }

        let bytes = fs::read(&state_path).await?;
        let state = serde_json::from_slice(&bytes)
            .map_err(|error| CoreError::Storage(format!("invalid resume state: {error}")))?;
        Ok(Some(state))
    }

    /// Finds one compatible resume state by file metadata.
    pub async fn find_resume_state(
        output_dir: &Path,
        file_name: &str,
        file_size: u64,
        chunk_size: u64,
    ) -> Result<Option<TransferResumeState>, StorageError> {
        if !is_plain_file_name(file_name) {
            return Err(CoreError::Storage(format!(
                "invalid output file name: {file_name}"
            )));
        }
        if !fs::try_exists(output_dir).await? {
            return Ok(None);
        }

        let mut best_state = None;
        let mut entries = fs::read_dir(output_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(candidate_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_resume_state_sidecar_for_file(candidate_name, file_name) {
                continue;
            }

            let bytes = match fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!("failed to read resume state {}: {error}", path.display());
                    continue;
                }
            };
            let state = match serde_json::from_slice::<TransferResumeState>(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!("invalid resume state {}: {error}", path.display());
                    continue;
                }
            };
            if validate_resume_state_name(&state).is_err() {
                continue;
            }
            if state.file_name == file_name
                && state.file_size == file_size
                && state.chunk_size == chunk_size
            {
                let should_replace =
                    best_state
                        .as_ref()
                        .is_none_or(|best: &TransferResumeState| {
                            state.bytes_received > best.bytes_received
                        });
                if should_replace {
                    best_state = Some(state);
                }
            }
        }

        Ok(best_state)
    }

    /// Writes or replaces the JSON sidecar state for a resumable transfer.
    pub async fn write_resume_state(
        output_dir: &Path,
        state: &TransferResumeState,
    ) -> Result<(), StorageError> {
        validate_resume_state_name(state)?;
        fs::create_dir_all(output_dir).await?;

        let state_path = resumable_state_path(output_dir, &state.file_name, &state.transfer_id);
        let temp_state_path =
            resumable_temp_state_path(output_dir, &state.file_name, &state.transfer_id);
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        let mut file = File::create(&temp_state_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(temp_state_path, state_path).await?;
        Ok(())
    }

    /// Deletes the JSON sidecar state after a transfer is finalized.
    pub async fn delete_resume_state(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<(), StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        let state_path = resumable_state_path(output_dir, file_name, transfer_id);
        if fs::try_exists(&state_path).await? {
            fs::remove_file(state_path).await?;
        }
        Ok(())
    }

    /// Deletes a resumable temp file if present.
    pub async fn delete_resume_temp(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<(), StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        let temp_path = resumable_temp_path(output_dir, file_name, transfer_id);
        if fs::try_exists(&temp_path).await? {
            fs::remove_file(temp_path).await?;
        }
        Ok(())
    }

    /// Renames a resumable temp file to a new transfer identifier.
    pub async fn rebind_resume_temp(
        output_dir: &Path,
        file_name: &str,
        old_transfer_id: &TransferId,
        new_transfer_id: &TransferId,
    ) -> Result<(), StorageError> {
        validate_resume_path_parts(file_name, old_transfer_id)?;
        validate_resume_path_parts(file_name, new_transfer_id)?;
        let old_path = resumable_temp_path(output_dir, file_name, old_transfer_id);
        let new_path = resumable_temp_path(output_dir, file_name, new_transfer_id);
        if old_path != new_path && fs::try_exists(&old_path).await? {
            if fs::try_exists(&new_path).await? {
                fs::remove_file(&new_path).await?;
            }
            fs::rename(old_path, new_path).await?;
        }
        Ok(())
    }

    /// Returns the deterministic temp path for a resumable transfer.
    pub fn resumable_temp_path(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<PathBuf, StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        Ok(resumable_temp_path(output_dir, file_name, transfer_id))
    }

    /// Writes the completion receipt for a finalized transfer (atomic sidecar;
    /// one per file name — a re-completion overwrites it).
    pub async fn write_receipt(
        output_dir: &Path,
        receipt: &TransferReceipt,
    ) -> Result<(), StorageError> {
        if !is_plain_file_name(&receipt.file_name) {
            return Err(CoreError::Storage(format!(
                "invalid output file name: {}",
                receipt.file_name
            )));
        }
        fs::create_dir_all(output_dir).await?;

        let path = receipt_path(output_dir, &receipt.file_name);
        let temp_path = output_dir.join(format!(".envoix-receipt.{}.json.tmp", receipt.file_name));
        let bytes = serde_json::to_vec_pretty(receipt)
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        let mut file = File::create(&temp_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(temp_path, path).await?;
        Ok(())
    }

    /// Deletes the completion receipt for `file_name`, if present (Remove is
    /// the one true abandon: a later re-offer of this file re-transfers).
    pub async fn delete_receipt(output_dir: &Path, file_name: &str) -> Result<(), StorageError> {
        if !is_plain_file_name(file_name) {
            return Err(CoreError::Storage(format!(
                "invalid output file name: {file_name}"
            )));
        }
        let path = receipt_path(output_dir, file_name);
        if fs::try_exists(&path).await? {
            fs::remove_file(path).await?;
        }
        Ok(())
    }

    /// Reads the completion receipt for `file_name`, if present. A receipt that
    /// fails to parse reads as `None` (self-healing: the next completion
    /// overwrites it) — a corrupt optimization sidecar must never block a
    /// receive.
    pub async fn read_receipt(
        output_dir: &Path,
        file_name: &str,
    ) -> Result<Option<TransferReceipt>, StorageError> {
        if !is_plain_file_name(file_name) {
            return Err(CoreError::Storage(format!(
                "invalid output file name: {file_name}"
            )));
        }
        let path = receipt_path(output_dir, file_name);
        if !fs::try_exists(&path).await? {
            return Ok(None);
        }
        let bytes = fs::read(&path).await?;
        Ok(serde_json::from_slice(&bytes).ok())
    }
}

fn is_plain_file_name(file_name: &str) -> bool {
    let mut components = Path::new(file_name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_resume_state_name(state: &TransferResumeState) -> Result<(), StorageError> {
    validate_resume_path_parts(&state.file_name, &state.transfer_id)?;
    if let Some(target) = &state.target_file_name
        && !is_plain_file_name(target)
    {
        return Err(CoreError::Storage(format!(
            "invalid target file name: {target}"
        )));
    }
    Ok(())
}

fn validate_resume_path_parts(
    file_name: &str,
    transfer_id: &TransferId,
) -> Result<(), StorageError> {
    if !is_plain_file_name(file_name) {
        return Err(CoreError::Storage(format!(
            "invalid output file name: {file_name}"
        )));
    }
    if !is_plain_file_name(&transfer_id.0) {
        return Err(CoreError::Storage(format!(
            "invalid transfer id: {transfer_id}"
        )));
    }
    Ok(())
}

fn resumable_temp_path(output_dir: &Path, file_name: &str, transfer_id: &TransferId) -> PathBuf {
    output_dir.join(format!(".envoix.{file_name}.{transfer_id}.part"))
}

fn resumable_state_path(output_dir: &Path, file_name: &str, transfer_id: &TransferId) -> PathBuf {
    output_dir.join(format!(".envoix.{file_name}.{transfer_id}.json"))
}

fn resumable_temp_state_path(
    output_dir: &Path,
    file_name: &str,
    transfer_id: &TransferId,
) -> PathBuf {
    output_dir.join(format!(".envoix.{file_name}.{transfer_id}.json.tmp"))
}

fn is_resume_state_sidecar_for_file(candidate_name: &str, file_name: &str) -> bool {
    let prefix = format!(".envoix.{file_name}.");
    candidate_name.starts_with(&prefix) && candidate_name.ends_with(".json")
}

/// Receipt sidecar path. The `.envoix-receipt.` prefix is deliberately outside
/// the `.envoix.{file}.` namespace [`is_resume_state_sidecar_for_file`] scans,
/// so a receipt can never be mistaken for resume state.
fn receipt_path(output_dir: &Path, file_name: &str) -> PathBuf {
    output_dir.join(format!(".envoix-receipt.{file_name}.json"))
}

#[cfg(test)]
mod tests {
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
}
