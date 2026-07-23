use std::fmt;

use envoix_types::{ArtifactId, OfferedName, RecordId};
use serde::{Deserialize, Serialize};

use crate::{CardManifest, OperationEnvelope};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// Visible through this store instance; no process-crash guarantee.
    Buffered,
    /// Flushed through process/runtime buffers to the backing store.
    Flushed,
    /// Stable across a process or system crash under the backend's contract.
    Durable,
}

/// Proof of the durability level observed before a commit returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitReceipt {
    requested: Durability,
    achieved: Durability,
}

impl CommitReceipt {
    pub fn observed(
        requested: Durability,
        achieved: Durability,
    ) -> Result<Self, DurabilityContractError> {
        if achieved < requested {
            return Err(DurabilityContractError::AchievedBelowRequested {
                requested,
                achieved,
            });
        }
        Ok(Self {
            requested,
            achieved,
        })
    }

    pub const fn requested(self) -> Durability {
        self.requested
    }

    pub const fn achieved(self) -> Durability {
        self.achieved
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurabilityContractError {
    AchievedBelowRequested {
        requested: Durability,
        achieved: Durability,
    },
}

impl fmt::Display for DurabilityContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AchievedBelowRequested {
                requested,
                achieved,
            } => write!(
                formatter,
                "commit achieved {achieved:?}, below requested {requested:?}"
            ),
        }
    }
}

impl std::error::Error for DurabilityContractError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EnvelopeKey {
    Operation(RecordId),
    Artifact {
        record_id: RecordId,
        artifact_id: ArtifactId,
    },
}

impl EnvelopeKey {
    pub const fn record_id(self) -> RecordId {
        match self {
            Self::Operation(record_id) | Self::Artifact { record_id, .. } => record_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineReason {
    Corrupt,
    UnsupportedFuture,
}

/// Original bytes retained outside the live namespace after quarantine.
#[derive(Clone, Eq, PartialEq)]
pub struct QuarantinedEnvelope {
    reason: QuarantineReason,
    bytes: Vec<u8>,
}

impl QuarantinedEnvelope {
    pub fn new(reason: QuarantineReason, bytes: Vec<u8>) -> Self {
        Self { reason, bytes }
    }

    pub const fn reason(&self) -> QuarantineReason {
        self.reason
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for QuarantinedEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuarantinedEnvelope")
            .field("reason", &self.reason)
            .field("byte_length", &self.bytes.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadOutcome {
    Loaded(OperationEnvelope),
    Quarantined { reason: QuarantineReason },
    Absent,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LeaseAcquisition<L> {
    Acquired(L),
    Busy,
}

pub trait WriterLease {
    fn record_id(&self) -> RecordId;
}

/// Card-scoped writes that become visible together only after a successful commit.
pub trait StorageTransaction {
    type Error;

    fn record_id(&self) -> RecordId;

    fn put_operation(&mut self, envelope: OperationEnvelope);

    fn put_artifact(
        &mut self,
        artifact_id: ArtifactId,
        name: OfferedName,
        envelope: OperationEnvelope,
    );

    /// Makes every staged write visible atomically at no less than `durability`.
    /// An error must leave all staged writes invisible.
    fn commit(self, durability: Durability) -> Result<CommitReceipt, Self::Error>;
}

/// Substitutable record/artifact storage; no method assumes a filesystem.
pub trait Storage {
    type Error;
    type Lease: WriterLease;
    type Transaction<'a>: StorageTransaction<Error = Self::Error>
    where
        Self: 'a;

    /// Highest durability level this backend can honestly guarantee.
    fn maximum_durability(&self) -> Durability;

    /// Loads one envelope. Corrupt/future bytes must leave the live namespace,
    /// remain retrievable through `quarantined`, and never report as absent.
    fn get(&mut self, key: EnvelopeKey) -> Result<LoadOutcome, Self::Error>;

    /// Returns preserved quarantine history without interpreting its bytes.
    fn quarantined(&self, key: EnvelopeKey) -> Result<Vec<QuarantinedEnvelope>, Self::Error>;

    /// Acquires backend-wide exclusive write ownership of one card namespace.
    fn acquire_writer(
        &mut self,
        record_id: RecordId,
    ) -> Result<LeaseAcquisition<Self::Lease>, Self::Error>;

    /// Releases ownership; a following acquire for the same card may succeed.
    fn release_writer(&mut self, lease: Self::Lease) -> Result<(), Self::Error>;

    /// Starts a card-scoped transaction after validating the active lease.
    fn begin<'a>(&'a mut self, lease: &Self::Lease) -> Result<Self::Transaction<'a>, Self::Error>;

    /// Returns the identity-keyed artifact manifest; names are metadata only.
    fn manifest(&self, record_id: RecordId) -> Result<Option<CardManifest>, Self::Error>;
}
