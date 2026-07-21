//! Local file and transfer-state storage.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

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

static ACTIVE_RESUME_LEASES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Process-local ownership of one resumable partial.
///
/// A sender creates a new protocol transfer id for each attempt, so a receiver
/// may rebind a compatible partial to that new id. The lease prevents a second
/// concurrent receive from selecting and renaming the same partial while the
/// first receive still has it open.
#[derive(Debug)]
pub struct ResumeLease {
    key: PathBuf,
}

/// Result of one stale-partial cleanup pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResumeCleanupReport {
    pub files_deleted: u64,
    pub bytes_deleted: u64,
}

impl ResumeLease {
    /// Moves the lease together with a resume sidecar that is rebound to a new
    /// protocol transfer id.
    pub fn rebind(
        &mut self,
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<(), StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        let new_key = resumable_state_path(output_dir, file_name, transfer_id);
        if new_key == self.key {
            return Ok(());
        }
        let mut leases = active_resume_leases()
            .lock()
            .map_err(|_| CoreError::Storage("resume lease registry is unavailable".to_string()))?;
        if leases.contains(&new_key) {
            return Err(CoreError::Storage(format!(
                "resume state is already in use: {}",
                new_key.display()
            )));
        }
        leases.remove(&self.key);
        leases.insert(new_key.clone());
        self.key = new_key;
        Ok(())
    }
}

impl Drop for ResumeLease {
    fn drop(&mut self) {
        if let Ok(mut leases) = active_resume_leases().lock() {
            leases.remove(&self.key);
        }
    }
}

/// Durable proof that a transfer completed: written beside the final file on
/// finalize and kept after the file itself is moved or published away (e.g.
/// Android publishes to MediaStore and deletes the output copy). A later
/// re-offer of the same transfer matches against it, so the receiver can
/// re-confirm completion — re-deliver a lost CompleteAck — without the file on
/// hand and without any bytes being re-sent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransferReceipt {
    /// Exact transfer identity. File name and size are not sufficient: a later
    /// transfer may reuse both after the previous final file was moved away.
    pub transfer_id: TransferId,
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
    /// Acquires exclusive in-process use of one resumable partial.
    pub fn try_acquire_resume_lease(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<Option<ResumeLease>, StorageError> {
        validate_resume_path_parts(file_name, transfer_id)?;
        let key = resumable_state_path(output_dir, file_name, transfer_id);
        let mut leases = active_resume_leases()
            .lock()
            .map_err(|_| CoreError::Storage("resume lease registry is unavailable".to_string()))?;
        if !leases.insert(key.clone()) {
            return Ok(None);
        }
        Ok(Some(ResumeLease { key }))
    }

    /// Deletes abandoned resume sidecars and partials older than `max_age`.
    /// Active in-process transfers are protected by their [`ResumeLease`].
    pub async fn cleanup_stale_resume_artifacts(
        output_dir: &Path,
        max_age: Duration,
    ) -> Result<ResumeCleanupReport, StorageError> {
        if !fs::try_exists(output_dir).await? {
            return Ok(ResumeCleanupReport::default());
        }
        let mut report = ResumeCleanupReport::default();
        let mut entries = fs::read_dir(output_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(".envoix.") || !name.ends_with(".json") {
                continue;
            }
            if is_resume_key_leased(&path) || !is_older_than(&path, max_age).await? {
                continue;
            }
            let state = fs::read(&path)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice::<TransferResumeState>(&bytes).ok())
                .filter(|state| validate_resume_state_name(state).is_ok());
            if let Some(state) = state {
                let temp_path =
                    resumable_temp_path(output_dir, &state.file_name, &state.transfer_id);
                delete_artifact(&temp_path, &mut report).await?;
            }
            delete_artifact(&path, &mut report).await?;
        }
        Ok(report)
    }

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

    /// Atomically claims `final_path` with the verified temp file's content.
    /// Returns `false` when the name is already taken - the caller picks
    /// another name. A name observed free is not a name owned: the old
    /// check-then-rename raced, and a concurrent finalizer's rename silently
    /// REPLACED a completed file (PR #48 review, P1). `hard_link` refuses an
    /// existing destination atomically.
    pub async fn finalize_temp_file(
        temp_path: &Path,
        final_path: &Path,
    ) -> Result<bool, StorageError> {
        match fs::hard_link(temp_path, final_path).await {
            Ok(()) => {
                fs::remove_file(temp_path).await?;
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(_) => {
                // Filesystem without hard links (e.g. FAT): degrade to the
                // checked rename. Racy, but better than failing receives;
                // real errors (permissions, IO) surface from the rename.
                if fs::try_exists(final_path).await? {
                    return Ok(false);
                }
                fs::rename(temp_path, final_path).await?;
                Ok(true)
            }
        }
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
        validate_resume_path_parts(&receipt.file_name, &receipt.transfer_id)?;
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

    /// Deletes a receipt only when it belongs to `transfer_id`. Cleanup from
    /// an older Activity must not remove a newer completion for the same name.
    pub async fn delete_receipt_for_transfer(
        output_dir: &Path,
        file_name: &str,
        transfer_id: &TransferId,
    ) -> Result<bool, StorageError> {
        let Some(receipt) = Self::read_receipt(output_dir, file_name).await? else {
            return Ok(false);
        };
        if &receipt.transfer_id != transfer_id {
            return Ok(false);
        }
        Self::delete_receipt(output_dir, file_name).await?;
        Ok(true)
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

fn active_resume_leases() -> &'static Mutex<HashSet<PathBuf>> {
    ACTIVE_RESUME_LEASES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_resume_key_leased(path: &Path) -> bool {
    active_resume_leases()
        .lock()
        .is_ok_and(|leases| leases.contains(path))
}

async fn is_older_than(path: &Path, max_age: Duration) -> Result<bool, StorageError> {
    let modified = fs::metadata(path)
        .await?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        >= max_age)
}

async fn delete_artifact(
    path: &Path,
    report: &mut ResumeCleanupReport,
) -> Result<(), StorageError> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    fs::remove_file(path).await?;
    report.files_deleted += 1;
    report.bytes_deleted = report.bytes_deleted.saturating_add(metadata.len());
    Ok(())
}

#[cfg(test)]
mod tests;
