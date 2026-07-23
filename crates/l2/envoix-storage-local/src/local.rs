use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use envoix_storage_api::{
    ArtifactManifestEntry, CardManifest, CommitReceipt, Durability, EnvelopeDecode, EnvelopeKey,
    LeaseAcquisition, LoadOutcome, OperationEnvelope, QuarantineReason, QuarantinedEnvelope,
    Storage, StorageTransaction, WriterLease,
};
use envoix_types::{ArtifactId, OfferedName, RecordId};

const CARDS_DIR: &str = "cards";
const REVISIONS_DIR: &str = "revisions";
const STAGING_DIR: &str = "staging";
const QUARANTINE_DIR: &str = "quarantine";
const ARTIFACTS_DIR: &str = "artifacts";
const CURRENT_FILE: &str = "current";
const MANIFEST_FILE: &str = "manifest.json";
const OPERATION_FILE: &str = "operation.env";

static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

pub struct LocalStorage {
    root: PathBuf,
    storage_id: u64,
    next_lease_generation: u64,
    next_revision_id: u64,
    active_writers: HashMap<RecordId, u64>,
    #[cfg(test)]
    fault: Option<CommitStage>,
}

impl LocalStorage {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LocalStorageError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(CARDS_DIR))?;
        let mut storage = Self {
            root,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            next_lease_generation: 1,
            next_revision_id: 1,
            active_writers: HashMap::new(),
            #[cfg(test)]
            fault: None,
        };
        let maximum_revision = storage.recover()?;
        storage.next_revision_id = maximum_revision
            .checked_add(1)
            .ok_or(LocalStorageError::RevisionIdExhausted)?;
        Ok(storage)
    }

    fn recover(&self) -> Result<u64, LocalStorageError> {
        let mut maximum_revision = 0;
        for entry in fs::read_dir(self.cards_root())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(value) = name.parse::<u64>() else {
                continue;
            };
            if value.to_string() != name {
                continue;
            }
            maximum_revision =
                maximum_revision.max(self.recover_card(RecordId::new(value), &entry.path())?);
        }
        Ok(maximum_revision)
    }

    fn recover_card(&self, record_id: RecordId, card_dir: &Path) -> Result<u64, LocalStorageError> {
        self.cleanup_dir(&card_dir.join(STAGING_DIR), None)?;
        self.cleanup_pointer_temps(card_dir)?;
        self.cleanup_quarantine_temps(&card_dir.join(QUARANTINE_DIR))?;

        let current = self.read_current_revision(card_dir)?;
        let revisions_dir = card_dir.join(REVISIONS_DIR);
        let Some(current_id) = current else {
            self.cleanup_dir(&revisions_dir, None)?;
            return Ok(0);
        };

        let revision_dir = revisions_dir.join(current_id.to_string());
        if !revision_dir.is_dir() {
            return Err(LocalStorageError::MissingCurrentRevision {
                record_id,
                revision: current_id,
            });
        }
        let manifest = self.read_card_manifest(record_id, &revision_dir)?;
        self.validate_manifest_files(record_id, &revision_dir, &manifest)?;
        self.cleanup_dir(&revisions_dir, Some(&current_id.to_string()))?;
        Ok(current_id)
    }

    fn validate_manifest_files(
        &self,
        record_id: RecordId,
        revision_dir: &Path,
        manifest: &CardManifest,
    ) -> Result<(), LocalStorageError> {
        let artifacts_dir = revision_dir.join(ARTIFACTS_DIR);
        if artifacts_dir.is_dir() {
            for entry in fs::read_dir(&artifacts_dir)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let indexed = file_name.to_str().is_some_and(|file_name| {
                    manifest
                        .artifacts()
                        .keys()
                        .any(|artifact_id| artifact_file_name(*artifact_id) == file_name)
                });
                if !indexed {
                    remove_entry(&entry.path())?;
                }
            }
            sync_directory(&artifacts_dir)?;
        }

        for artifact_id in manifest.artifacts().keys().copied() {
            let path = artifacts_dir.join(artifact_file_name(artifact_id));
            if path.try_exists()? {
                continue;
            }
            let key = EnvelopeKey::Artifact {
                record_id,
                artifact_id,
            };
            if self.read_quarantine(key)?.is_empty() {
                return Err(LocalStorageError::MissingCommittedEnvelope(key));
            }
        }
        Ok(())
    }

    fn cleanup_pointer_temps(&self, card_dir: &Path) -> Result<(), LocalStorageError> {
        for entry in fs::read_dir(card_dir)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("current.next-"))
            {
                remove_entry(&entry.path())?;
            }
        }
        Ok(())
    }

    fn cleanup_quarantine_temps(&self, quarantine: &Path) -> Result<(), LocalStorageError> {
        if !quarantine.try_exists()? {
            return Ok(());
        }
        for directory in fs::read_dir(quarantine)? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(directory.path())? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".tmp"))
                {
                    remove_entry(&entry.path())?;
                }
            }
        }
        Ok(())
    }

    fn cleanup_dir(
        &self,
        directory: &Path,
        keep_name: Option<&str>,
    ) -> Result<(), LocalStorageError> {
        if !directory.try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if keep_name.is_some_and(|keep| entry.file_name() == keep) {
                continue;
            }
            remove_entry(&entry.path())?;
        }
        sync_directory(directory)?;
        Ok(())
    }

    fn validate_lease(&self, lease: &LocalWriterLease) -> Result<(), LocalStorageError> {
        if lease.storage_id != self.storage_id
            || self.active_writers.get(&lease.record_id) != Some(&lease.generation)
        {
            return Err(LocalStorageError::InvalidLease);
        }
        Ok(())
    }

    fn commit_revision(
        &mut self,
        record_id: RecordId,
        operation: Option<OperationEnvelope>,
        artifacts: BTreeMap<ArtifactId, StagedArtifact>,
        durability: Durability,
    ) -> Result<CommitReceipt, LocalStorageError> {
        let revision_id = self.allocate_revision_id()?;
        let card_dir = self.card_dir(record_id);
        let staging_root = card_dir.join(STAGING_DIR);
        let revisions_root = card_dir.join(REVISIONS_DIR);
        fs::create_dir_all(&staging_root)?;
        fs::create_dir_all(&revisions_root)?;

        let staged_revision = staging_root.join(revision_id.to_string());
        fs::create_dir(&staged_revision)?;
        self.checkpoint(CommitStage::StagingCreated)?;

        let prior_revision = self.read_current_revision(&card_dir)?;
        let mut manifest = if let Some(prior_id) = prior_revision {
            let prior_dir = revisions_root.join(prior_id.to_string());
            let prior_manifest = self.read_card_manifest(record_id, &prior_dir)?;
            self.copy_prior_revision(record_id, &prior_dir, &staged_revision, &prior_manifest)?;
            prior_manifest
        } else {
            CardManifest::new(record_id, durability)
        };

        if let Some(operation) = operation {
            write_file(&staged_revision.join(OPERATION_FILE), &operation.encode())?;
        }
        if !artifacts.is_empty() {
            fs::create_dir_all(staged_revision.join(ARTIFACTS_DIR))?;
        }
        for (artifact_id, artifact) in artifacts {
            write_file(
                &staged_revision
                    .join(ARTIFACTS_DIR)
                    .join(artifact_file_name(artifact_id)),
                &artifact.envelope.encode(),
            )?;
            manifest.insert(
                artifact_id,
                ArtifactManifestEntry::new(artifact.name, durability),
            );
        }
        manifest.set_committed_at(durability);
        let manifest_bytes =
            serde_json::to_vec(&manifest).map_err(|_| LocalStorageError::ManifestEncodingFailed)?;
        write_file(&staged_revision.join(MANIFEST_FILE), &manifest_bytes)?;
        self.checkpoint(CommitStage::RevisionWritten)?;

        if durability >= Durability::Flushed {
            sync_revision_files(&staged_revision)?;
        }
        if durability == Durability::Durable {
            sync_revision_directories(&staged_revision)?;
        }
        self.checkpoint(CommitStage::RevisionSynced)?;

        let published_revision = revisions_root.join(revision_id.to_string());
        fs::rename(&staged_revision, &published_revision)?;
        if durability == Durability::Durable {
            sync_directory(&staging_root)?;
            sync_directory(&revisions_root)?;
        }
        self.checkpoint(CommitStage::RevisionPublished)?;

        let decision_temp = card_dir.join(format!("current.next-{revision_id}"));
        write_file(&decision_temp, format!("{revision_id}\n").as_bytes())?;
        self.checkpoint(CommitStage::DecisionWritten)?;
        if durability >= Durability::Flushed {
            File::open(&decision_temp)?.sync_all()?;
        }
        self.checkpoint(CommitStage::DecisionSynced)?;

        fs::rename(&decision_temp, card_dir.join(CURRENT_FILE))?;
        self.checkpoint(CommitStage::Linearized)?;
        if durability == Durability::Durable {
            sync_directory(&card_dir)?;
            sync_directory(&self.cards_root())?;
            sync_directory(&self.root)?;
        }
        self.checkpoint(CommitStage::DecisionDirectorySynced)?;

        if let Some(prior_id) = prior_revision
            && prior_id != revision_id
        {
            remove_entry(&revisions_root.join(prior_id.to_string()))?;
        }
        self.checkpoint(CommitStage::OldRevisionCleaned)?;

        if durability == Durability::Durable {
            sync_directory(&card_dir)?;
            sync_directory(&revisions_root)?;
        }
        self.checkpoint(CommitStage::CommitDirectorySynced)?;

        CommitReceipt::observed(durability, durability)
            .map_err(|_| LocalStorageError::DurabilityContractFailed)
    }

    fn copy_prior_revision(
        &self,
        record_id: RecordId,
        prior: &Path,
        staged: &Path,
        manifest: &CardManifest,
    ) -> Result<(), LocalStorageError> {
        let prior_operation = prior.join(OPERATION_FILE);
        if prior_operation.try_exists()? {
            fs::copy(&prior_operation, staged.join(OPERATION_FILE))?;
        }

        for artifact_id in manifest.artifacts().keys().copied() {
            let source = prior
                .join(ARTIFACTS_DIR)
                .join(artifact_file_name(artifact_id));
            if source.try_exists()? {
                let target_dir = staged.join(ARTIFACTS_DIR);
                fs::create_dir_all(&target_dir)?;
                fs::copy(source, target_dir.join(artifact_file_name(artifact_id)))?;
                continue;
            }
            let key = EnvelopeKey::Artifact {
                record_id,
                artifact_id,
            };
            if self.read_quarantine(key)?.is_empty() {
                return Err(LocalStorageError::MissingCommittedEnvelope(key));
            }
        }
        Ok(())
    }

    fn allocate_revision_id(&mut self) -> Result<u64, LocalStorageError> {
        let revision = self.next_revision_id;
        self.next_revision_id = self
            .next_revision_id
            .checked_add(1)
            .ok_or(LocalStorageError::RevisionIdExhausted)?;
        Ok(revision)
    }

    fn live_path(&self, key: EnvelopeKey) -> Result<Option<PathBuf>, LocalStorageError> {
        let record_id = key.record_id();
        let card_dir = self.card_dir(record_id);
        let Some(revision_id) = self.read_current_revision(&card_dir)? else {
            return Ok(None);
        };
        let revision_dir = card_dir.join(REVISIONS_DIR).join(revision_id.to_string());
        let path = match key {
            EnvelopeKey::Operation(_) => revision_dir.join(OPERATION_FILE),
            EnvelopeKey::Artifact { artifact_id, .. } => {
                let manifest = self.read_card_manifest(record_id, &revision_dir)?;
                if !manifest.artifacts().contains_key(&artifact_id) {
                    return Ok(None);
                }
                revision_dir
                    .join(ARTIFACTS_DIR)
                    .join(artifact_file_name(artifact_id))
            }
        };
        if path.try_exists()? {
            return Ok(Some(path));
        }
        if matches!(key, EnvelopeKey::Artifact { .. }) && self.read_quarantine(key)?.is_empty() {
            return Err(LocalStorageError::MissingCommittedEnvelope(key));
        }
        Ok(None)
    }

    fn historical_outcome(&self, key: EnvelopeKey) -> Result<LoadOutcome, LocalStorageError> {
        Ok(self
            .read_quarantine(key)?
            .last()
            .map_or(LoadOutcome::Absent, |entry| LoadOutcome::Quarantined {
                reason: entry.reason(),
            }))
    }

    fn preserve_quarantine(
        &self,
        key: EnvelopeKey,
        reason: QuarantineReason,
        bytes: &[u8],
    ) -> Result<(), LocalStorageError> {
        let existing = self.read_quarantine(key)?;
        if existing
            .iter()
            .any(|entry| entry.reason() == reason && entry.bytes() == bytes)
        {
            return Ok(());
        }

        let directory = self.quarantine_key_dir(key);
        fs::create_dir_all(&directory)?;
        let next = existing
            .len()
            .checked_add(1)
            .ok_or(LocalStorageError::QuarantineIndexExhausted)?;
        let reason_name = match reason {
            QuarantineReason::Corrupt => "corrupt",
            QuarantineReason::UnsupportedFuture => "future",
        };
        let final_path = directory.join(format!("{next:020}-{reason_name}.env"));
        let temp_path = directory.join(format!("{next:020}-{reason_name}.tmp"));
        write_file(&temp_path, bytes)?;
        File::open(&temp_path)?.sync_all()?;
        fs::rename(temp_path, final_path)?;
        sync_directory(&directory)?;
        if let Some(quarantine_root) = directory.parent() {
            sync_directory(quarantine_root)?;
        }
        sync_directory(&self.card_dir(key.record_id()))?;
        Ok(())
    }

    fn read_quarantine(
        &self,
        key: EnvelopeKey,
    ) -> Result<Vec<QuarantinedEnvelope>, LocalStorageError> {
        let directory = self.quarantine_key_dir(key);
        if !directory.try_exists()? {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or(LocalStorageError::InvalidQuarantineEntry)?;
            if name.ends_with(".tmp") {
                continue;
            }
            let (index, reason) = parse_quarantine_name(name)?;
            entries.push((index, reason, fs::read(entry.path())?));
        }
        entries.sort_by_key(|(index, _, _)| *index);
        Ok(entries
            .into_iter()
            .map(|(_, reason, bytes)| QuarantinedEnvelope::new(reason, bytes))
            .collect())
    }

    fn read_current_revision(&self, card_dir: &Path) -> Result<Option<u64>, LocalStorageError> {
        let path = card_dir.join(CURRENT_FILE);
        if !path.try_exists()? {
            return Ok(None);
        }
        let encoded = fs::read_to_string(&path)?;
        let canonical = encoded
            .strip_suffix('\n')
            .ok_or_else(|| LocalStorageError::InvalidCurrentPointer(path.clone()))?;
        let revision = canonical
            .parse::<u64>()
            .map_err(|_| LocalStorageError::InvalidCurrentPointer(path.clone()))?;
        if revision == 0 || revision.to_string() != canonical {
            return Err(LocalStorageError::InvalidCurrentPointer(path));
        }
        Ok(Some(revision))
    }

    fn read_manifest(&self, revision_dir: &Path) -> Result<CardManifest, LocalStorageError> {
        let path = revision_dir.join(MANIFEST_FILE);
        let bytes = fs::read(&path)?;
        serde_json::from_slice(&bytes).map_err(|_| LocalStorageError::CorruptManifest(path))
    }

    fn read_card_manifest(
        &self,
        record_id: RecordId,
        revision_dir: &Path,
    ) -> Result<CardManifest, LocalStorageError> {
        let manifest = self.read_manifest(revision_dir)?;
        if manifest.record_id() != record_id {
            return Err(LocalStorageError::ManifestRecordMismatch {
                expected: record_id,
                actual: manifest.record_id(),
            });
        }
        Ok(manifest)
    }

    fn cards_root(&self) -> PathBuf {
        self.root.join(CARDS_DIR)
    }

    fn card_dir(&self, record_id: RecordId) -> PathBuf {
        self.cards_root().join(record_id.get().to_string())
    }

    fn quarantine_key_dir(&self, key: EnvelopeKey) -> PathBuf {
        let base = self.card_dir(key.record_id()).join(QUARANTINE_DIR);
        match key {
            EnvelopeKey::Operation(_) => base.join("operation"),
            EnvelopeKey::Artifact { artifact_id, .. } => {
                base.join(format!("artifact-{}", artifact_hex(artifact_id)))
            }
        }
    }

    fn checkpoint(&mut self, stage: CommitStage) -> Result<(), LocalStorageError> {
        #[cfg(test)]
        if self.fault == Some(stage) {
            self.fault = None;
            return Err(LocalStorageError::InjectedFault);
        }
        let _ = stage;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_fault(&mut self, stage: CommitStage) {
        self.fault = Some(stage);
    }

    #[cfg(test)]
    pub(crate) fn overwrite_live(
        &self,
        key: EnvelopeKey,
        bytes: &[u8],
    ) -> Result<(), LocalStorageError> {
        let path = self
            .live_path(key)?
            .ok_or(LocalStorageError::MissingCommittedEnvelope(key))?;
        write_file(&path, bytes)
    }
}

impl Storage for LocalStorage {
    type Error = LocalStorageError;
    type Lease = LocalWriterLease;
    type Transaction<'a> = LocalTransaction<'a>;

    fn maximum_durability(&self) -> Durability {
        Durability::Durable
    }

    fn get(&mut self, key: EnvelopeKey) -> Result<LoadOutcome, Self::Error> {
        let Some(path) = self.live_path(key)? else {
            return self.historical_outcome(key);
        };
        let bytes = fs::read(&path)?;
        match OperationEnvelope::decode(&bytes) {
            EnvelopeDecode::Loaded(envelope) => Ok(LoadOutcome::Loaded(envelope)),
            EnvelopeDecode::Quarantined { reason } => {
                self.preserve_quarantine(key, reason, &bytes)?;
                fs::remove_file(&path)?;
                if let Some(parent) = path.parent() {
                    sync_directory(parent)?;
                }
                Ok(LoadOutcome::Quarantined { reason })
            }
        }
    }

    fn quarantined(&self, key: EnvelopeKey) -> Result<Vec<QuarantinedEnvelope>, Self::Error> {
        self.read_quarantine(key)
    }

    fn acquire_writer(
        &mut self,
        record_id: RecordId,
    ) -> Result<LeaseAcquisition<Self::Lease>, Self::Error> {
        if self.active_writers.contains_key(&record_id) {
            return Ok(LeaseAcquisition::Busy);
        }
        let generation = self.next_lease_generation;
        self.next_lease_generation = self
            .next_lease_generation
            .checked_add(1)
            .ok_or(LocalStorageError::LeaseGenerationExhausted)?;
        self.active_writers.insert(record_id, generation);
        Ok(LeaseAcquisition::Acquired(LocalWriterLease {
            storage_id: self.storage_id,
            record_id,
            generation,
        }))
    }

    fn release_writer(&mut self, lease: Self::Lease) -> Result<(), Self::Error> {
        self.validate_lease(&lease)?;
        self.active_writers.remove(&lease.record_id);
        Ok(())
    }

    fn begin<'a>(&'a mut self, lease: &Self::Lease) -> Result<Self::Transaction<'a>, Self::Error> {
        self.validate_lease(lease)?;
        Ok(LocalTransaction {
            storage: self,
            record_id: lease.record_id,
            operation: None,
            artifacts: BTreeMap::new(),
        })
    }

    fn manifest(&self, record_id: RecordId) -> Result<Option<CardManifest>, Self::Error> {
        let card_dir = self.card_dir(record_id);
        let Some(revision_id) = self.read_current_revision(&card_dir)? else {
            return Ok(None);
        };
        let revision_dir = card_dir.join(REVISIONS_DIR).join(revision_id.to_string());
        self.read_card_manifest(record_id, &revision_dir).map(Some)
    }
}

#[derive(Eq, PartialEq)]
pub struct LocalWriterLease {
    storage_id: u64,
    record_id: RecordId,
    generation: u64,
}

impl WriterLease for LocalWriterLease {
    fn record_id(&self) -> RecordId {
        self.record_id
    }
}

impl fmt::Debug for LocalWriterLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalWriterLease")
            .field("record_id", &self.record_id)
            .finish_non_exhaustive()
    }
}

struct StagedArtifact {
    name: OfferedName,
    envelope: OperationEnvelope,
}

pub struct LocalTransaction<'a> {
    storage: &'a mut LocalStorage,
    record_id: RecordId,
    operation: Option<OperationEnvelope>,
    artifacts: BTreeMap<ArtifactId, StagedArtifact>,
}

impl StorageTransaction for LocalTransaction<'_> {
    type Error = LocalStorageError;

    fn record_id(&self) -> RecordId {
        self.record_id
    }

    fn put_operation(&mut self, envelope: OperationEnvelope) {
        self.operation = Some(envelope);
    }

    fn put_artifact(
        &mut self,
        artifact_id: ArtifactId,
        name: OfferedName,
        envelope: OperationEnvelope,
    ) {
        self.artifacts
            .insert(artifact_id, StagedArtifact { name, envelope });
    }

    fn commit(self, durability: Durability) -> Result<CommitReceipt, Self::Error> {
        self.storage
            .commit_revision(self.record_id, self.operation, self.artifacts, durability)
    }
}

#[derive(Debug)]
pub enum LocalStorageError {
    Io(io::Error),
    InvalidLease,
    LeaseGenerationExhausted,
    RevisionIdExhausted,
    QuarantineIndexExhausted,
    InvalidCurrentPointer(PathBuf),
    MissingCurrentRevision {
        record_id: RecordId,
        revision: u64,
    },
    ManifestRecordMismatch {
        expected: RecordId,
        actual: RecordId,
    },
    MissingCommittedEnvelope(EnvelopeKey),
    CorruptManifest(PathBuf),
    ManifestEncodingFailed,
    InvalidQuarantineEntry,
    DurabilityContractFailed,
    InjectedFault,
}

impl fmt::Display for LocalStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidLease => formatter.write_str("writer lease is invalid or inactive"),
            Self::LeaseGenerationExhausted => {
                formatter.write_str("writer lease generation exhausted")
            }
            Self::RevisionIdExhausted => formatter.write_str("storage revision id exhausted"),
            Self::QuarantineIndexExhausted => {
                formatter.write_str("quarantine history index exhausted")
            }
            Self::InvalidCurrentPointer(path) => {
                write!(
                    formatter,
                    "invalid current revision pointer: {}",
                    path.display()
                )
            }
            Self::MissingCurrentRevision {
                record_id,
                revision,
            } => write!(
                formatter,
                "card {record_id} points to missing revision {revision}"
            ),
            Self::ManifestRecordMismatch { expected, actual } => write!(
                formatter,
                "manifest record {actual} does not match card {expected}"
            ),
            Self::MissingCommittedEnvelope(key) => {
                write!(formatter, "committed envelope is missing: {key:?}")
            }
            Self::CorruptManifest(path) => {
                write!(formatter, "manifest is corrupt: {}", path.display())
            }
            Self::ManifestEncodingFailed => formatter.write_str("manifest encoding failed"),
            Self::InvalidQuarantineEntry => {
                formatter.write_str("quarantine history entry is invalid")
            }
            Self::DurabilityContractFailed => {
                formatter.write_str("durability receipt violated the storage contract")
            }
            Self::InjectedFault => formatter.write_str("injected commit-stage fault"),
        }
    }
}

impl std::error::Error for LocalStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for LocalStorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitStage {
    StagingCreated,
    RevisionWritten,
    RevisionSynced,
    RevisionPublished,
    DecisionWritten,
    DecisionSynced,
    Linearized,
    DecisionDirectorySynced,
    OldRevisionCleaned,
    CommitDirectorySynced,
}

impl CommitStage {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 10] = [
        Self::StagingCreated,
        Self::RevisionWritten,
        Self::RevisionSynced,
        Self::RevisionPublished,
        Self::DecisionWritten,
        Self::DecisionSynced,
        Self::Linearized,
        Self::DecisionDirectorySynced,
        Self::OldRevisionCleaned,
        Self::CommitDirectorySynced,
    ];

    #[cfg(test)]
    pub(crate) const fn is_after_linearization(self) -> bool {
        matches!(
            self,
            Self::Linearized
                | Self::DecisionDirectorySynced
                | Self::OldRevisionCleaned
                | Self::CommitDirectorySynced
        )
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), LocalStorageError> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn sync_revision_files(revision: &Path) -> Result<(), LocalStorageError> {
    File::open(revision.join(MANIFEST_FILE))?.sync_all()?;
    let operation = revision.join(OPERATION_FILE);
    if operation.try_exists()? {
        File::open(operation)?.sync_all()?;
    }
    let artifacts = revision.join(ARTIFACTS_DIR);
    if artifacts.is_dir() {
        for entry in fs::read_dir(artifacts)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                File::open(entry.path())?.sync_all()?;
            }
        }
    }
    Ok(())
}

fn sync_revision_directories(revision: &Path) -> Result<(), LocalStorageError> {
    let artifacts = revision.join(ARTIFACTS_DIR);
    if artifacts.is_dir() {
        sync_directory(&artifacts)?;
    }
    sync_directory(revision)
}

fn sync_directory(path: &Path) -> Result<(), LocalStorageError> {
    if path.is_dir() {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<(), LocalStorageError> {
    let file_type = fs::symlink_metadata(path)?.file_type();
    if file_type.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn artifact_file_name(artifact_id: ArtifactId) -> String {
    format!("{}.env", artifact_hex(artifact_id))
}

fn artifact_hex(artifact_id: ArtifactId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(32);
    for byte in artifact_id.to_bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn parse_quarantine_name(name: &str) -> Result<(usize, QuarantineReason), LocalStorageError> {
    let stem = name
        .strip_suffix(".env")
        .ok_or(LocalStorageError::InvalidQuarantineEntry)?;
    let (index, reason) = stem
        .split_once('-')
        .ok_or(LocalStorageError::InvalidQuarantineEntry)?;
    let index = index
        .parse()
        .map_err(|_| LocalStorageError::InvalidQuarantineEntry)?;
    let reason = match reason {
        "corrupt" => QuarantineReason::Corrupt,
        "future" => QuarantineReason::UnsupportedFuture,
        _ => return Err(LocalStorageError::InvalidQuarantineEntry),
    };
    Ok((index, reason))
}
