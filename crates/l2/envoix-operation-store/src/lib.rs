//! Durable, versioned operation records and store-owned recovery facts.

#![forbid(unsafe_code)]

mod codec;
pub mod identifiers;

#[cfg(test)]
mod tests;

use std::fmt;

use envoix_capabilities::{
    Admission, Duty, DutyLedger, DutyResult, GenerationUpdate, Registration,
};
use envoix_storage_api::{
    CommitReceipt, Durability, EnvelopeError, EnvelopeKey, LeaseAcquisition, LoadOutcome,
    OperationEnvelope, QuarantineReason, Storage, StorageTransaction,
};
use envoix_types::{ArtifactId, AttemptGen, LandedName, OfferedName, RecordId, TransferId};
use serde::{Deserialize, Serialize};

use crate::identifiers::OPERATION_STORE_STATE_SCHEMA_ID;

/// Transfer and artifact identity used for possession and proof lookup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactKey {
    pub transfer: TransferId,
    pub artifact: ArtifactId,
}

/// Durable knowledge about one identity-keyed artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PossessionState {
    Partial,
    Complete { landed_name: Option<LandedName> },
}

/// A possession fact. Names are metadata and never participate in lookup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PossessionFact {
    key: ArtifactKey,
    offered_name: OfferedName,
    state: PossessionState,
    receipt_proven: bool,
}

impl PossessionFact {
    pub const fn key(&self) -> ArtifactKey {
        self.key
    }

    pub const fn offered_name(&self) -> &OfferedName {
        &self.offered_name
    }

    pub const fn state(&self) -> &PossessionState {
        &self.state
    }

    pub const fn completion_proven(&self) -> bool {
        matches!(self.state, PossessionState::Complete { .. })
    }

    pub const fn receipt_proven(&self) -> bool {
        self.receipt_proven
    }
}

/// Idempotency key and payload for one destructive world-facing operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructiveOperation {
    DiscardPartial { card: RecordId, key: ArtifactKey },
    CollectArtifact { card: RecordId, key: ArtifactKey },
    TombstoneCard { card: RecordId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxStatus {
    Recorded,
    AlreadyPending,
    Confirmed,
    AlreadyConfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordCommit {
    Committed {
        revision: u64,
        receipt: CommitReceipt,
    },
    AlreadyCommitted {
        revision: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactCommit {
    Staged { receipt: CommitReceipt },
    AlreadyStaged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RecordRevision {
    revision: u64,
    body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PersistedDuty {
    duty: Duty,
    result: Option<DutyResult>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OutboxEntry {
    operation: DestructiveOperation,
    confirmed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoreImage {
    schema: String,
    card: RecordId,
    next_record_revision: u64,
    records: Vec<RecordRevision>,
    tombstoned: bool,
    current_generation: Option<AttemptGen>,
    duties: Vec<PersistedDuty>,
    possessions: Vec<PossessionFact>,
    outbox: Vec<OutboxEntry>,
}

impl StoreImage {
    fn empty(card: RecordId) -> Self {
        Self {
            schema: OPERATION_STORE_STATE_SCHEMA_ID.to_owned(),
            card,
            next_record_revision: 1,
            records: Vec::new(),
            tombstoned: false,
            current_generation: None,
            duties: Vec::new(),
            possessions: Vec::new(),
            outbox: Vec::new(),
        }
    }
}

/// Card-scoped durable operation store over an injected C5 backend.
///
/// **Single-writer precondition.** Each mutation is a read-modify-write: it
/// refreshes the durable image, then commits the candidate under the C5 writer
/// lease. The lease serializes the *writes*, but the composition root MUST keep
/// **exactly one live `OperationStore` per card over a single backend handle**.
/// Two stores over two backend handles to the same storage root (e.g. two
/// `LocalStorage::open` of one directory) do not mutually exclude and can lose an
/// update. The runtime's card registry (P5/RT) owns this exclusivity.
pub struct OperationStore<S: Storage> {
    storage: S,
    card: RecordId,
    image: StoreImage,
    ledger: DutyLedger,
}

impl<S: Storage> OperationStore<S> {
    /// Opens one card and reconstructs its duty ledger, outbox, and possession facts.
    pub fn open(storage: S, card: RecordId) -> Result<Self, StoreError<S::Error>> {
        let mut store = Self {
            storage,
            card,
            image: StoreImage::empty(card),
            ledger: DutyLedger::new(),
        };
        store.refresh()?;
        Ok(store)
    }

    pub const fn record_id(&self) -> RecordId {
        self.card
    }

    pub fn into_storage(self) -> S {
        self.storage
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    pub fn latest_record(&self) -> Option<&[u8]> {
        self.image
            .records
            .last()
            .map(|revision| revision.body.as_slice())
    }

    pub fn record_revision(&self, revision: u64) -> Option<&[u8]> {
        self.image
            .records
            .iter()
            .find(|candidate| candidate.revision == revision)
            .map(|candidate| candidate.body.as_slice())
    }

    pub fn record_revision_count(&self) -> usize {
        self.image.records.len()
    }

    /// Atomically appends one opaque record body behind the backend's card lease.
    ///
    /// Retrying the same latest body does not create another revision.
    pub fn commit_record(
        &mut self,
        body: &[u8],
        durability: Durability,
    ) -> Result<RecordCommit, StoreError<S::Error>> {
        self.refresh()?;
        if let Some(latest) = self.image.records.last()
            && latest.body == body
        {
            return Ok(RecordCommit::AlreadyCommitted {
                revision: latest.revision,
            });
        }

        let mut candidate = self.image.clone();
        let revision = candidate.next_record_revision;
        candidate.next_record_revision = revision
            .checked_add(1)
            .ok_or(StoreError::RevisionExhausted)?;
        candidate.records.push(RecordRevision {
            revision,
            body: body.to_vec(),
        });
        // The whole per-card image lives in ONE C5 envelope capped at
        // `MAX_ENVELOPE_BODY_BYTES`. Retaining every historical revision would grow
        // it without bound and eventually wedge the card (it could commit nothing).
        // Keep the most recent revisions under a byte budget; the newest (which
        // backs the record store and the idempotence check) is always retained.
        prune_record_history(&mut candidate.records);
        let receipt = self.persist(candidate, None, durability)?;
        Ok(RecordCommit::Committed { revision, receipt })
    }

    /// Atomically writes a partial artifact and its identity-keyed possession fact.
    pub fn stage_artifact(
        &mut self,
        key: ArtifactKey,
        name: OfferedName,
        body: &[u8],
    ) -> Result<ArtifactCommit, StoreError<S::Error>> {
        self.refresh()?;
        self.validate_new_artifact_identity(key)?;

        if let Some(existing) = self.possession(key) {
            if existing.offered_name != name {
                return Err(StoreError::ArtifactMetadataConflict);
            }
            if !matches!(existing.state, PossessionState::Partial) {
                return Err(StoreError::CompletionRegression);
            }
            let envelope_key = EnvelopeKey::Artifact {
                record_id: self.card,
                artifact_id: key.artifact,
            };
            match self
                .storage
                .get(envelope_key)
                .map_err(StoreError::Backend)?
            {
                LoadOutcome::Loaded(envelope) if envelope.body().as_bytes() == body => {
                    return Ok(ArtifactCommit::AlreadyStaged);
                }
                LoadOutcome::Loaded(_) => return Err(StoreError::ArtifactBytesConflict),
                LoadOutcome::Quarantined { .. } | LoadOutcome::Absent => {}
            }
        }

        let artifact = OperationEnvelope::new(body.to_vec()).map_err(StoreError::Envelope)?;
        let mut candidate = self.image.clone();
        if candidate
            .possessions
            .iter()
            .all(|possession| possession.key != key)
        {
            candidate.possessions.push(PossessionFact {
                key,
                offered_name: name.clone(),
                state: PossessionState::Partial,
                receipt_proven: false,
            });
        }
        let receipt = self.persist(
            candidate,
            Some(StagedArtifact {
                artifact_id: key.artifact,
                name,
                envelope: artifact,
            }),
            Durability::Durable,
        )?;
        Ok(ArtifactCommit::Staged { receipt })
    }

    /// Records completion for the exact transfer/artifact pair.
    pub fn record_completion(
        &mut self,
        key: ArtifactKey,
        landed_name: Option<LandedName>,
    ) -> Result<CommitReceipt, StoreError<S::Error>> {
        self.refresh()?;
        let mut candidate = self.image.clone();
        let possession = candidate
            .possessions
            .iter_mut()
            .find(|possession| possession.key == key)
            .ok_or(StoreError::UnknownArtifact)?;
        possession.state = PossessionState::Complete { landed_name };
        // A now-complete artifact must never be discarded as a "partial"; retire
        // any queued `DiscardPartial` for this key so it is not left permanently
        // pending (`operation_is_safe` would hide it forever, never confirmed).
        candidate.outbox.retain(|entry| {
            entry.operation
                != DestructiveOperation::DiscardPartial {
                    card: self.card,
                    key,
                }
        });
        self.persist(candidate, None, Durability::Durable)
    }

    /// Records receipt proof for the exact completed transfer/artifact pair.
    pub fn record_receipt(
        &mut self,
        key: ArtifactKey,
    ) -> Result<CommitReceipt, StoreError<S::Error>> {
        self.refresh()?;
        let mut candidate = self.image.clone();
        let possession = candidate
            .possessions
            .iter_mut()
            .find(|possession| possession.key == key)
            .ok_or(StoreError::UnknownArtifact)?;
        if !possession.completion_proven() {
            return Err(StoreError::CompletionNotProven);
        }
        possession.receipt_proven = true;
        self.persist(candidate, None, Durability::Durable)
    }

    pub fn possession(&self, key: ArtifactKey) -> Option<&PossessionFact> {
        self.image
            .possessions
            .iter()
            .find(|possession| possession.key == key)
    }

    pub fn possessions(&self) -> &[PossessionFact] {
        &self.image.possessions
    }

    pub const fn is_tombstoned(&self) -> bool {
        self.image.tombstoned
    }

    /// Durably records the deletion fact and tombstone outbox entry together.
    pub fn commit_tombstone(&mut self) -> Result<OutboxStatus, StoreError<S::Error>> {
        self.refresh()?;
        let operation = DestructiveOperation::TombstoneCard { card: self.card };
        let mut candidate = self.image.clone();
        candidate.tombstoned = true;
        let status = insert_outbox(&mut candidate, operation);
        if candidate == self.image {
            return Ok(status);
        }
        self.persist(candidate, None, Durability::Durable)?;
        Ok(status)
    }

    /// Durably queues collection only after a tombstone fact exists.
    pub fn queue_artifact_gc(
        &mut self,
        key: ArtifactKey,
    ) -> Result<OutboxStatus, StoreError<S::Error>> {
        self.refresh()?;
        if !self.image.tombstoned {
            return Err(StoreError::TombstoneRequired);
        }
        let possession = self.possession(key).ok_or(StoreError::UnknownArtifact)?;
        if !safe_to_remove(possession) {
            return Err(StoreError::WouldLoseLastGoodCopy);
        }
        self.queue_outbox(DestructiveOperation::CollectArtifact {
            card: self.card,
            key,
        })
    }

    pub fn queue_discard_partial(
        &mut self,
        key: ArtifactKey,
    ) -> Result<OutboxStatus, StoreError<S::Error>> {
        self.refresh()?;
        let possession = self.possession(key).ok_or(StoreError::UnknownArtifact)?;
        if !matches!(possession.state, PossessionState::Partial) {
            return Err(StoreError::NotPartial);
        }
        self.queue_outbox(DestructiveOperation::DiscardPartial {
            card: self.card,
            key,
        })
    }

    /// Returns the at-least-once replay queue, withholding unsafe last-copy deletion.
    pub fn replayable_outbox(&self) -> Vec<DestructiveOperation> {
        self.image
            .outbox
            .iter()
            .filter(|entry| !entry.confirmed)
            .filter(|entry| self.operation_is_safe(entry.operation))
            .map(|entry| entry.operation)
            .collect()
    }

    pub fn outbox_is_pending(&self, operation: DestructiveOperation) -> bool {
        self.image
            .outbox
            .iter()
            .any(|entry| entry.operation == operation && !entry.confirmed)
    }

    /// Durably confirms successful idempotent execution; failures leave the entry pending.
    pub fn confirm_outbox(
        &mut self,
        operation: DestructiveOperation,
    ) -> Result<OutboxStatus, StoreError<S::Error>> {
        self.refresh()?;
        let mut candidate = self.image.clone();
        let entry = candidate
            .outbox
            .iter_mut()
            .find(|entry| entry.operation == operation)
            .ok_or(StoreError::UnknownOutboxOperation)?;
        if entry.confirmed {
            return Ok(OutboxStatus::AlreadyConfirmed);
        }
        entry.confirmed = true;
        self.persist(candidate, None, Durability::Durable)?;
        Ok(OutboxStatus::Confirmed)
    }

    pub fn advance_generation(
        &mut self,
        generation: AttemptGen,
    ) -> Result<GenerationUpdate, StoreError<S::Error>> {
        self.refresh()?;
        let update = self.ledger.advance_generation(self.card, generation);
        if !matches!(
            update,
            GenerationUpdate::Initialized | GenerationUpdate::Advanced
        ) {
            return Ok(update);
        }

        let mut candidate = self.image.clone();
        candidate.current_generation = Some(generation);
        candidate
            .duties
            .retain(|entry| entry.duty.provenance.generation >= generation);
        self.persist(candidate, None, Durability::Durable)?;
        Ok(update)
    }

    pub fn register_duty(&mut self, duty: Duty) -> Result<Registration, StoreError<S::Error>> {
        self.refresh()?;
        if duty.provenance.card != self.card {
            return Err(StoreError::CardMismatch {
                expected: self.card,
                actual: duty.provenance.card,
            });
        }
        let registration = self.ledger.register(duty);
        if registration != Registration::Registered {
            return Ok(registration);
        }

        let mut candidate = self.image.clone();
        candidate.duties.push(PersistedDuty { duty, result: None });
        self.persist(candidate, None, Durability::Durable)?;
        Ok(registration)
    }

    pub fn admit_duty(&mut self, result: DutyResult) -> Result<Admission, StoreError<S::Error>> {
        self.refresh()?;
        let admission = self.ledger.admit(result);
        if !matches!(admission, Admission::Fresh(_)) {
            return Ok(admission);
        }

        let mut candidate = self.image.clone();
        let entry = candidate
            .duties
            .iter_mut()
            .find(|entry| entry.duty.provenance == result.provenance)
            .ok_or(StoreError::CorruptState)?;
        entry.result = Some(result);
        self.persist(candidate, None, Durability::Durable)?;
        Ok(admission)
    }

    pub fn outstanding_duties(&self) -> Vec<Duty> {
        self.image
            .duties
            .iter()
            .filter(|entry| entry.result.is_none())
            .map(|entry| entry.duty)
            .collect()
    }

    fn validate_new_artifact_identity(&self, key: ArtifactKey) -> Result<(), StoreError<S::Error>> {
        if self.image.possessions.iter().any(|possession| {
            possession.key != key
                && (possession.key.artifact == key.artifact
                    || possession.key.transfer == key.transfer)
        }) {
            return Err(StoreError::ArtifactIdentityConflict);
        }
        Ok(())
    }

    fn queue_outbox(
        &mut self,
        operation: DestructiveOperation,
    ) -> Result<OutboxStatus, StoreError<S::Error>> {
        let mut candidate = self.image.clone();
        let status = insert_outbox(&mut candidate, operation);
        if candidate == self.image {
            return Ok(status);
        }
        self.persist(candidate, None, Durability::Durable)?;
        Ok(status)
    }

    fn operation_is_safe(&self, operation: DestructiveOperation) -> bool {
        match operation {
            DestructiveOperation::DiscardPartial { key, .. } => self
                .possession(key)
                .is_some_and(|fact| matches!(fact.state, PossessionState::Partial)),
            DestructiveOperation::CollectArtifact { key, .. } => {
                self.image.tombstoned && self.possession(key).is_some_and(safe_to_remove)
            }
            DestructiveOperation::TombstoneCard { .. } => {
                self.image.tombstoned && self.image.possessions.iter().all(safe_to_remove)
            }
        }
    }

    fn refresh(&mut self) -> Result<(), StoreError<S::Error>> {
        let image = load_image(&mut self.storage, self.card)?;
        let ledger = rebuild_ledger(&image)?;
        self.image = image;
        self.ledger = ledger;
        Ok(())
    }

    fn persist(
        &mut self,
        candidate: StoreImage,
        artifact: Option<StagedArtifact>,
        durability: Durability,
    ) -> Result<CommitReceipt, StoreError<S::Error>> {
        validate_image(&candidate, self.card)?;
        let encoded = codec::to_vec(&candidate).map_err(|_| StoreError::CorruptState)?;
        let envelope = OperationEnvelope::new(encoded).map_err(StoreError::Envelope)?;
        let lease = match self
            .storage
            .acquire_writer(self.card)
            .map_err(StoreError::Backend)?
        {
            LeaseAcquisition::Acquired(lease) => lease,
            LeaseAcquisition::Busy => return Err(StoreError::WriterBusy),
        };

        let commit_result = match self.storage.begin(&lease) {
            Ok(mut transaction) => {
                transaction.put_operation(envelope);
                if let Some(artifact) = artifact {
                    transaction.put_artifact(
                        artifact.artifact_id,
                        artifact.name,
                        artifact.envelope,
                    );
                }
                transaction.commit(durability)
            }
            Err(error) => Err(error),
        };
        let release_result = self.storage.release_writer(lease);
        let receipt = commit_result.map_err(StoreError::Backend)?;
        release_result.map_err(StoreError::Backend)?;

        self.ledger = rebuild_ledger(&candidate)?;
        self.image = candidate;
        Ok(receipt)
    }
}

struct StagedArtifact {
    artifact_id: ArtifactId,
    name: OfferedName,
    envelope: OperationEnvelope,
}

fn load_image<S: Storage>(
    storage: &mut S,
    card: RecordId,
) -> Result<StoreImage, StoreError<S::Error>> {
    match storage
        .get(EnvelopeKey::Operation(card))
        .map_err(StoreError::Backend)?
    {
        LoadOutcome::Loaded(envelope) => {
            let image = codec::from_slice(envelope.body().as_bytes())
                .map_err(|_| StoreError::CorruptState)?;
            validate_image(&image, card)?;
            Ok(image)
        }
        LoadOutcome::Quarantined { reason } => Err(StoreError::Quarantined(reason)),
        LoadOutcome::Absent => Ok(StoreImage::empty(card)),
    }
}

fn validate_image<E>(image: &StoreImage, card: RecordId) -> Result<(), StoreError<E>> {
    if image.schema != OPERATION_STORE_STATE_SCHEMA_ID {
        return Err(StoreError::UnsupportedStateSchema);
    }
    if image.card != card {
        return Err(StoreError::CardMismatch {
            expected: card,
            actual: image.card,
        });
    }
    if image.next_record_revision == 0
        || image
            .records
            .windows(2)
            .any(|pair| pair[0].revision >= pair[1].revision)
        || image
            .records
            .last()
            .is_some_and(|last| last.revision >= image.next_record_revision)
    {
        return Err(StoreError::CorruptState);
    }
    Ok(())
}

fn rebuild_ledger<E>(image: &StoreImage) -> Result<DutyLedger, StoreError<E>> {
    let mut ledger = DutyLedger::new();
    if let Some(generation) = image.current_generation {
        let update = ledger.advance_generation(image.card, generation);
        if update != GenerationUpdate::Initialized {
            return Err(StoreError::CorruptState);
        }
    }
    for entry in &image.duties {
        if ledger.register(entry.duty) != Registration::Registered {
            return Err(StoreError::CorruptState);
        }
        if let Some(result) = entry.result
            && !matches!(ledger.admit(result), Admission::Fresh(_))
        {
            return Err(StoreError::CorruptState);
        }
    }
    Ok(ledger)
}

fn insert_outbox(image: &mut StoreImage, operation: DestructiveOperation) -> OutboxStatus {
    if let Some(entry) = image
        .outbox
        .iter()
        .find(|entry| entry.operation == operation)
    {
        return if entry.confirmed {
            OutboxStatus::AlreadyConfirmed
        } else {
            OutboxStatus::AlreadyPending
        };
    }
    image.outbox.push(OutboxEntry {
        operation,
        confirmed: false,
    });
    OutboxStatus::Recorded
}

fn safe_to_remove(fact: &PossessionFact) -> bool {
    match &fact.state {
        PossessionState::Partial => true,
        PossessionState::Complete { landed_name } => landed_name.is_some() || fact.receipt_proven,
    }
}

/// Byte budget for retained record-revision history within the single per-card
/// image envelope (which is capped at `MAX_ENVELOPE_BODY_BYTES` = 1 MiB). Well
/// under the cap so the rest of the image (duties, possessions, outbox) always
/// fits; the newest revision is retained regardless.
const RECORD_HISTORY_BUDGET_BYTES: usize = 256 * 1024;

/// Drop the oldest record revisions until the retained bodies fit the history
/// budget, always keeping at least the newest revision.
fn prune_record_history(records: &mut Vec<RecordRevision>) {
    let mut total: usize = records.iter().map(|revision| revision.body.len()).sum();
    while records.len() > 1 && total > RECORD_HISTORY_BUDGET_BYTES {
        let dropped = records.remove(0);
        total -= dropped.body.len();
    }
}

#[derive(Debug)]
pub enum StoreError<E> {
    Backend(E),
    Envelope(EnvelopeError),
    WriterBusy,
    Quarantined(QuarantineReason),
    CorruptState,
    UnsupportedStateSchema,
    RevisionExhausted,
    CardMismatch {
        expected: RecordId,
        actual: RecordId,
    },
    ArtifactIdentityConflict,
    ArtifactMetadataConflict,
    ArtifactBytesConflict,
    UnknownArtifact,
    CompletionRegression,
    CompletionNotProven,
    TombstoneRequired,
    WouldLoseLastGoodCopy,
    NotPartial,
    UnknownOutboxOperation,
}

impl<E: fmt::Display> fmt::Display for StoreError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => write!(formatter, "storage backend failed: {error}"),
            Self::Envelope(error) => error.fmt(formatter),
            Self::WriterBusy => formatter.write_str("the card already has a live writer"),
            Self::Quarantined(reason) => {
                write!(
                    formatter,
                    "the operation envelope is quarantined: {reason:?}"
                )
            }
            Self::CorruptState => formatter.write_str("the operation-store state is corrupt"),
            Self::UnsupportedStateSchema => {
                formatter.write_str("the operation-store state schema is unsupported")
            }
            Self::RevisionExhausted => formatter.write_str("record revision space is exhausted"),
            Self::CardMismatch { expected, actual } => {
                write!(
                    formatter,
                    "state card {actual} does not match requested card {expected}"
                )
            }
            Self::ArtifactIdentityConflict => {
                formatter.write_str("artifact or transfer identity is already paired differently")
            }
            Self::ArtifactMetadataConflict => {
                formatter.write_str("artifact metadata conflicts with the durable fact")
            }
            Self::ArtifactBytesConflict => {
                formatter.write_str("artifact bytes conflict with the durable identity")
            }
            Self::UnknownArtifact => formatter.write_str("artifact identity is unknown"),
            Self::CompletionRegression => {
                formatter.write_str("a completed artifact cannot return to partial")
            }
            Self::CompletionNotProven => {
                formatter.write_str("artifact completion is not durably proven")
            }
            Self::TombstoneRequired => {
                formatter.write_str("artifact collection requires a durable tombstone")
            }
            Self::WouldLoseLastGoodCopy => {
                formatter.write_str("destruction would remove the last durable good copy")
            }
            Self::NotPartial => formatter.write_str("artifact is not partial"),
            Self::UnknownOutboxOperation => formatter.write_str("outbox operation is unknown"),
        }
    }
}

impl<E> std::error::Error for StoreError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            Self::Envelope(error) => Some(error),
            _ => None,
        }
    }
}
