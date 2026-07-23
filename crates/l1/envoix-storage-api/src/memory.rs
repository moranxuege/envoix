use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use envoix_types::{ArtifactId, OfferedName, RecordId};

use crate::{
    ArtifactManifestEntry, CardManifest, CommitReceipt, Durability, EnvelopeDecode, EnvelopeKey,
    LeaseAcquisition, LoadOutcome, OperationEnvelope, QuarantinedEnvelope, Storage,
    StorageTransaction, WriterLease,
};

static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

pub struct InMemoryStorage {
    storage_id: u64,
    next_lease_generation: u64,
    active_writers: HashMap<RecordId, u64>,
    live: HashMap<EnvelopeKey, Vec<u8>>,
    quarantine: HashMap<EnvelopeKey, Vec<QuarantinedEnvelope>>,
    manifests: HashMap<RecordId, CardManifest>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            next_lease_generation: 1,
            active_writers: HashMap::new(),
            live: HashMap::new(),
            quarantine: HashMap::new(),
            manifests: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn seed_raw(&mut self, key: EnvelopeKey, bytes: Vec<u8>) {
        self.live.insert(key, bytes);
    }

    fn validate_lease(&self, lease: &InMemoryWriterLease) -> Result<(), MemoryStorageError> {
        if lease.storage_id != self.storage_id
            || self.active_writers.get(&lease.record_id) != Some(&lease.generation)
        {
            return Err(MemoryStorageError::InvalidLease);
        }
        Ok(())
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage for InMemoryStorage {
    type Error = MemoryStorageError;
    type Lease = InMemoryWriterLease;
    type Transaction<'a> = InMemoryTransaction<'a>;

    fn maximum_durability(&self) -> Durability {
        Durability::Buffered
    }

    fn get(&mut self, key: EnvelopeKey) -> Result<LoadOutcome, Self::Error> {
        let Some(bytes) = self.live.get(&key) else {
            return Ok(self
                .quarantine
                .get(&key)
                .and_then(|entries| entries.last())
                .map_or(LoadOutcome::Absent, |entry| LoadOutcome::Quarantined {
                    reason: entry.reason(),
                }));
        };
        match OperationEnvelope::decode(bytes) {
            EnvelopeDecode::Loaded(envelope) => Ok(LoadOutcome::Loaded(envelope)),
            EnvelopeDecode::Quarantined { reason } => {
                let bytes = self.live.remove(&key).ok_or(MemoryStorageError::Internal)?;
                self.quarantine
                    .entry(key)
                    .or_default()
                    .push(QuarantinedEnvelope::new(reason, bytes));
                Ok(LoadOutcome::Quarantined { reason })
            }
        }
    }

    fn quarantined(&self, key: EnvelopeKey) -> Result<Vec<QuarantinedEnvelope>, Self::Error> {
        Ok(self.quarantine.get(&key).cloned().unwrap_or_default())
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
            .ok_or(MemoryStorageError::LeaseGenerationExhausted)?;
        self.active_writers.insert(record_id, generation);
        Ok(LeaseAcquisition::Acquired(InMemoryWriterLease {
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
        Ok(InMemoryTransaction {
            storage: self,
            record_id: lease.record_id,
            operation: None,
            artifacts: BTreeMap::new(),
        })
    }

    fn manifest(&self, record_id: RecordId) -> Result<Option<CardManifest>, Self::Error> {
        Ok(self.manifests.get(&record_id).cloned())
    }
}

#[derive(Eq, PartialEq)]
pub struct InMemoryWriterLease {
    storage_id: u64,
    record_id: RecordId,
    generation: u64,
}

impl WriterLease for InMemoryWriterLease {
    fn record_id(&self) -> RecordId {
        self.record_id
    }
}

impl fmt::Debug for InMemoryWriterLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryWriterLease")
            .field("record_id", &self.record_id)
            .finish_non_exhaustive()
    }
}

struct StagedArtifact {
    name: OfferedName,
    envelope: OperationEnvelope,
}

pub struct InMemoryTransaction<'a> {
    storage: &'a mut InMemoryStorage,
    record_id: RecordId,
    operation: Option<OperationEnvelope>,
    artifacts: BTreeMap<ArtifactId, StagedArtifact>,
}

impl StorageTransaction for InMemoryTransaction<'_> {
    type Error = MemoryStorageError;

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
        let achieved = self.storage.maximum_durability();
        if durability > achieved {
            return Err(MemoryStorageError::UnsupportedDurability {
                requested: durability,
                maximum: achieved,
            });
        }

        let operation = self.operation.map(|envelope| envelope.encode());
        let artifacts = self
            .artifacts
            .into_iter()
            .map(|(artifact_id, artifact)| (artifact_id, artifact.name, artifact.envelope.encode()))
            .collect::<Vec<_>>();

        if let Some(bytes) = operation {
            self.storage
                .live
                .insert(EnvelopeKey::Operation(self.record_id), bytes);
        }
        let manifest = self
            .storage
            .manifests
            .entry(self.record_id)
            .or_insert_with(|| CardManifest::new(self.record_id, achieved));
        manifest.set_committed_at(achieved);
        for (artifact_id, name, bytes) in artifacts {
            self.storage.live.insert(
                EnvelopeKey::Artifact {
                    record_id: self.record_id,
                    artifact_id,
                },
                bytes,
            );
            manifest.insert(artifact_id, ArtifactManifestEntry::new(name, achieved));
        }
        CommitReceipt::observed(durability, achieved).map_err(|_| MemoryStorageError::Internal)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryStorageError {
    InvalidLease,
    LeaseGenerationExhausted,
    UnsupportedDurability {
        requested: Durability,
        maximum: Durability,
    },
    Internal,
}

impl fmt::Display for MemoryStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLease => formatter.write_str("writer lease is invalid or inactive"),
            Self::LeaseGenerationExhausted => {
                formatter.write_str("writer lease generation exhausted")
            }
            Self::UnsupportedDurability { requested, maximum } => write!(
                formatter,
                "durability {requested:?} exceeds backend maximum {maximum:?}"
            ),
            Self::Internal => formatter.write_str("in-memory storage invariant failed"),
        }
    }
}

impl std::error::Error for MemoryStorageError {}
