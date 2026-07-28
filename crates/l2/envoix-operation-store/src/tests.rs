use std::fs;
use std::path::PathBuf;

use envoix_capabilities::{
    Admission, Duty, DutyKind, DutyProvenance, DutyResult, GenerationUpdate, Registration,
};
use envoix_outcomes::OutcomeCode;
use envoix_storage_api::{
    CardManifest, CommitReceipt, Durability, EnvelopeKey, InMemoryStorage, InMemoryTransaction,
    InMemoryWriterLease, LeaseAcquisition, LoadOutcome, MemoryStorageError, OperationEnvelope,
    QuarantineReason, QuarantinedEnvelope, Storage, StorageTransaction,
};
use envoix_storage_local::LocalStorage;
use envoix_types::{
    ArtifactId, AttemptGen, LandedName, OfferedName, RecordId, RequestId, TransferId,
};
use tempfile::TempDir;

use crate::{
    ArtifactCommit, ArtifactKey, DestructiveOperation, OperationStore, OutboxStatus,
    PossessionState, RecordCommit, StoreError,
};

#[test]
fn record_revisions_are_durable_and_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(1);
    let mut store = open(&root, card);

    assert_eq!(
        store
            .commit_record(b"record-v1", Durability::Durable)
            .unwrap(),
        RecordCommit::Committed {
            revision: 1,
            receipt: envoix_storage_api::CommitReceipt::observed(
                Durability::Durable,
                Durability::Durable
            )
            .unwrap(),
        }
    );
    assert_eq!(
        store
            .commit_record(b"record-v1", Durability::Durable)
            .unwrap(),
        RecordCommit::AlreadyCommitted { revision: 1 }
    );
    assert!(matches!(
        store.commit_record(b"record-v2", Durability::Durable),
        Ok(RecordCommit::Committed { revision: 2, .. })
    ));
    drop(store);

    let reopened = open(&root, card);
    assert_eq!(reopened.record_revision_count(), 2);
    assert_eq!(reopened.record_revision(1), Some(b"record-v1".as_slice()));
    assert_eq!(reopened.record_revision(2), Some(b"record-v2".as_slice()));
    assert_eq!(reopened.latest_record(), Some(b"record-v2".as_slice()));
}

#[test]
fn ambiguous_record_commit_retry_is_idempotent() {
    let card = RecordId::new(70);
    let backend = AmbiguousStorage {
        inner: InMemoryStorage::new(),
        fail_after_commit: true,
    };
    let mut store = OperationStore::open(backend, card).unwrap();

    assert!(matches!(
        store.commit_record(b"record", Durability::Buffered),
        Err(StoreError::Backend(AmbiguousError::AfterCommit))
    ));
    assert_eq!(
        store
            .commit_record(b"record", Durability::Buffered)
            .unwrap(),
        RecordCommit::AlreadyCommitted { revision: 1 }
    );
    assert_eq!(store.record_revision_count(), 1);
}

#[test]
fn gc_only_after_durable_tombstone() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(2);
    let key = artifact_key(1);
    let mut store = open(&root, card);
    stage(&mut store, key, "photo.jpg", b"partial");

    assert!(matches!(
        store.queue_artifact_gc(key),
        Err(StoreError::TombstoneRequired)
    ));
    assert_eq!(store.commit_tombstone().unwrap(), OutboxStatus::Recorded);
    assert_eq!(
        store.queue_artifact_gc(key).unwrap(),
        OutboxStatus::Recorded
    );
    let collect = DestructiveOperation::CollectArtifact { card, key };
    assert!(store.outbox_is_pending(collect));
    drop(store);

    let reopened = open(&root, card);
    assert!(reopened.is_tombstoned());
    assert!(reopened.replayable_outbox().contains(&collect));
}

#[test]
fn record_absence_never_deletes_artifact() {
    for quarantine in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let card = RecordId::new(10 + u64::from(quarantine));
        let key = artifact_key(10 + u8::from(quarantine));
        let artifact_envelope_key = EnvelopeKey::Artifact {
            record_id: card,
            artifact_id: key.artifact,
        };
        let mut store = open(&root, card);
        stage(&mut store, key, "kept.bin", b"must-survive");
        drop(store);

        let operation_path = current_revision(&root, card).join("operation.env");
        if quarantine {
            fs::write(&operation_path, [0, 1, 2]).unwrap();
            assert!(matches!(
                OperationStore::open(LocalStorage::open(root.path()).unwrap(), card),
                Err(StoreError::Quarantined(QuarantineReason::Corrupt))
            ));
        } else {
            fs::remove_file(operation_path).unwrap();
            let reopened = open(&root, card);
            assert!(!reopened.is_tombstoned());
            assert!(reopened.replayable_outbox().is_empty());
            drop(reopened);
        }

        let mut backend = LocalStorage::open(root.path()).unwrap();
        let LoadOutcome::Loaded(envelope) = backend.get(artifact_envelope_key).unwrap() else {
            panic!("artifact was removed after record absence/quarantine");
        };
        assert_eq!(envelope.body().as_bytes(), b"must-survive");
    }
}

#[test]
fn durable_receipt_duty_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(20);
    let current = AttemptGen::new(4);
    let duty = courier_duty(card, current, 1);
    let mut store = open(&root, card);
    assert_eq!(
        store.advance_generation(current).unwrap(),
        GenerationUpdate::Initialized
    );
    assert_eq!(store.register_duty(duty).unwrap(), Registration::Registered);
    drop(store);

    let mut reopened = open(&root, card);
    assert_eq!(reopened.outstanding_duties(), vec![duty]);
    let stale = DutyResult {
        provenance: DutyProvenance {
            card,
            generation: AttemptGen::new(3),
            request: RequestId::from_bytes([9; 16]),
        },
        outcome: OutcomeCode::Completed,
    };
    assert_eq!(reopened.admit_duty(stale).unwrap(), Admission::Stale);

    let correct = DutyResult {
        provenance: duty.provenance,
        outcome: OutcomeCode::Completed,
    };
    let Admission::Fresh(admitted) = reopened.admit_duty(correct).unwrap() else {
        panic!("current outstanding result should be admitted");
    };
    assert_eq!(admitted.duty(), duty);
    assert_eq!(admitted.outcome(), OutcomeCode::Completed);
    assert_eq!(reopened.admit_duty(correct).unwrap(), Admission::Duplicate);
    drop(reopened);

    let mut reopened = open(&root, card);
    assert!(reopened.outstanding_duties().is_empty());
    assert_eq!(reopened.admit_duty(correct).unwrap(), Admission::Duplicate);
}

#[test]
fn same_name_concurrent_and_crash_matrix() {
    for partial_before_completion in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let card = RecordId::new(30 + u64::from(partial_before_completion));
        let final_key = artifact_key(30);
        let partial_key = artifact_key(31);
        let duplicate_name = "duplicate.jpg";

        let mut store = open(&root, card);
        stage(&mut store, final_key, duplicate_name, b"final-bytes");
        drop(store);

        let mut store = open(&root, card);
        if partial_before_completion {
            stage(&mut store, partial_key, duplicate_name, b"partial-bytes");
        } else {
            store
                .record_completion(final_key, Some(LandedName::new("final-copy.jpg")))
                .unwrap();
        }
        drop(store);

        let mut store = open(&root, card);
        if partial_before_completion {
            store
                .record_completion(final_key, Some(LandedName::new("final-copy.jpg")))
                .unwrap();
        } else {
            stage(&mut store, partial_key, duplicate_name, b"partial-bytes");
        }
        drop(store);

        let mut reopened = open(&root, card);
        assert!(reopened.possession(final_key).unwrap().completion_proven());
        assert!(matches!(
            reopened.possession(partial_key).unwrap().state(),
            PossessionState::Partial
        ));
        reopened.record_receipt(final_key).unwrap();
        drop(reopened);

        let reopened = open(&root, card);
        assert!(reopened.possession(final_key).unwrap().receipt_proven());
        assert!(!reopened.possession(partial_key).unwrap().receipt_proven());

        let manifest = reopened.storage().manifest(card).unwrap().unwrap();
        assert_eq!(manifest.artifacts().len(), 2);
        assert_eq!(
            manifest
                .artifacts()
                .get(&final_key.artifact)
                .unwrap()
                .name()
                .as_str(),
            duplicate_name
        );
        assert_eq!(
            manifest
                .artifacts()
                .get(&partial_key.artifact)
                .unwrap()
                .name()
                .as_str(),
            duplicate_name
        );
    }
}

#[test]
fn completion_receipt_proof_keyed_by_identity() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(40);
    let first = artifact_key(40);
    let second = artifact_key(41);
    let mut store = open(&root, card);
    stage(&mut store, first, "same.bin", b"first");
    stage(&mut store, second, "same.bin", b"second");
    store
        .record_completion(first, Some(LandedName::new("renamed.bin")))
        .unwrap();
    store.record_receipt(first).unwrap();
    drop(store);

    let reopened = open(&root, card);
    let first_fact = reopened.possession(first).unwrap();
    assert!(first_fact.completion_proven());
    assert!(first_fact.receipt_proven());
    assert!(matches!(
        first_fact.state(),
        PossessionState::Complete {
            landed_name: Some(name)
        } if name.as_str() == "renamed.bin"
    ));
    let second_fact = reopened.possession(second).unwrap();
    assert!(!second_fact.completion_proven());
    assert!(!second_fact.receipt_proven());
    assert!(
        reopened
            .possession(ArtifactKey {
                transfer: first.transfer,
                artifact: second.artifact,
            })
            .is_none()
    );
}

#[test]
fn durable_outbox_replays_a_destructive_op_after_crash() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(50);
    let key = artifact_key(50);
    let operation = DestructiveOperation::DiscardPartial { card, key };
    let mut store = open(&root, card);
    stage(&mut store, key, "partial.bin", b"partial");
    assert_eq!(
        store.queue_discard_partial(key).unwrap(),
        OutboxStatus::Recorded
    );
    drop(store);

    let reopened = open(&root, card);
    assert_eq!(reopened.replayable_outbox(), vec![operation]);
    drop(reopened);

    let mut retry = open(&root, card);
    assert_eq!(retry.replayable_outbox(), vec![operation]);
    assert_eq!(
        retry.settle_destructive(operation).unwrap(),
        OutboxStatus::Confirmed
    );
    assert_eq!(
        retry.settle_destructive(operation).unwrap(),
        OutboxStatus::AlreadyConfirmed
    );
    drop(retry);

    assert!(open(&root, card).replayable_outbox().is_empty());
}

/// Executing a destructive operation and confirming it are ONE durable image,
/// so there is no window in which the fact is gone but the entry is still
/// pending: such an entry is hidden by its own safety predicate (never
/// replayed, never confirmable) and re-arms itself the moment the same
/// artifact key is staged again, destroying the fresh bytes.
#[test]
fn destructive_settlement_is_one_image_and_never_resurrects() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(51);
    let key = artifact_key(51);
    let operation = DestructiveOperation::DiscardPartial { card, key };
    let mut store = open(&root, card);
    stage(&mut store, key, "partial.bin", b"partial");
    store.queue_discard_partial(key).unwrap();

    // Executed and confirmed in a single durable transaction: the backend
    // revision advances exactly once (two writes would leave a crash window).
    let before = durable_revision(&root, card);
    assert_eq!(
        store.settle_destructive(operation).unwrap(),
        OutboxStatus::Confirmed
    );
    assert_eq!(
        durable_revision(&root, card),
        before + 1,
        "execute and confirm are one durable write"
    );
    assert_eq!(
        store.settle_destructive(operation).unwrap(),
        OutboxStatus::AlreadyConfirmed
    );
    drop(store);

    // The next boot sees BOTH halves: the possession fact is gone and nothing
    // is left to replay.
    let mut reopened = open(&root, card);
    assert!(reopened.possession(key).is_none());
    assert!(!reopened.outbox_is_pending(operation));
    assert!(reopened.replayable_outbox().is_empty());

    // Re-staging the same identity restores the discard's safety predicate. A
    // settled entry must not become replayable again.
    stage(&mut reopened, key, "partial.bin", b"fresh-partial");
    assert!(
        reopened.replayable_outbox().is_empty(),
        "a settled discard never resurrects against freshly staged bytes"
    );
    assert!(reopened.possession(key).is_some());
}

#[test]
fn last_good_copy_is_never_dispatched_for_destruction() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(60);
    let key = artifact_key(60);
    let tombstone = DestructiveOperation::TombstoneCard { card };
    let mut store = open(&root, card);
    stage(&mut store, key, "only-copy.bin", b"complete");
    store.record_completion(key, None).unwrap();
    store.commit_tombstone().unwrap();

    assert!(!store.replayable_outbox().contains(&tombstone));
    assert!(matches!(
        store.queue_artifact_gc(key),
        Err(StoreError::WouldLoseLastGoodCopy)
    ));

    store.record_receipt(key).unwrap();
    assert!(store.replayable_outbox().contains(&tombstone));
    assert_eq!(
        store.queue_artifact_gc(key).unwrap(),
        OutboxStatus::Recorded
    );
}

#[test]
fn record_history_is_bounded_and_never_wedges_the_card() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(70);
    let mut store = open(&root, card);
    // Many large distinct revisions that would blow past the 1 MiB envelope cap
    // if history were retained without bound.
    for n in 0..30u8 {
        let mut body = vec![7u8; 100 * 1024];
        body[0] = n;
        assert!(matches!(
            store.commit_record(&body, Durability::Durable).unwrap(),
            RecordCommit::Committed { .. }
        ));
    }
    // The card still commits (no wedge) and the latest revision is intact.
    let latest = vec![0xAAu8; 100 * 1024];
    assert!(matches!(
        store.commit_record(&latest, Durability::Durable).unwrap(),
        RecordCommit::Committed { .. }
    ));
    assert_eq!(store.latest_record(), Some(latest.as_slice()));
    drop(store);
    // The bounded image survives a reopen.
    let reopened = open(&root, card);
    assert_eq!(reopened.latest_record(), Some(latest.as_slice()));
}

#[test]
fn completing_an_artifact_retires_a_queued_discard() {
    let root = tempfile::tempdir().unwrap();
    let card = RecordId::new(80);
    let key = artifact_key(80);
    let discard = DestructiveOperation::DiscardPartial { card, key };
    let mut store = open(&root, card);
    stage(&mut store, key, "partial.bin", b"partial");
    assert_eq!(
        store.queue_discard_partial(key).unwrap(),
        OutboxStatus::Recorded
    );
    assert!(store.replayable_outbox().contains(&discard));

    // Completion retires the now-moot discard so it is not hidden forever.
    store.record_completion(key, None).unwrap();
    assert!(
        !store.outbox_is_pending(discard),
        "a completed artifact's stale discard is retired, not left permanently pending"
    );
    drop(store);
    let reopened = open(&root, card);
    assert!(!reopened.replayable_outbox().contains(&discard));
}

fn open(root: &TempDir, card: RecordId) -> OperationStore<LocalStorage> {
    OperationStore::open(LocalStorage::open(root.path()).unwrap(), card).unwrap()
}

fn artifact_key(seed: u8) -> ArtifactKey {
    ArtifactKey {
        transfer: TransferId::from_bytes([seed; 16]),
        artifact: ArtifactId::from_bytes([seed.wrapping_add(1); 16]),
    }
}

fn stage<S: Storage>(store: &mut OperationStore<S>, key: ArtifactKey, name: &str, bytes: &[u8]) {
    assert!(matches!(
        store.stage_artifact(key, OfferedName::from_untrusted(name).unwrap(), bytes),
        Ok(ArtifactCommit::Staged { .. })
    ));
}

fn courier_duty(card: RecordId, generation: AttemptGen, request: u8) -> Duty {
    Duty {
        provenance: DutyProvenance {
            card,
            generation,
            request: RequestId::from_bytes([request; 16]),
        },
        kind: DutyKind::Courier,
    }
}

fn current_revision(root: &TempDir, card: RecordId) -> PathBuf {
    let card_root = root.path().join("cards").join(card.get().to_string());
    let revision = fs::read_to_string(card_root.join("current")).unwrap();
    card_root.join("revisions").join(revision.trim())
}

/// The backend's published revision counter for a card: it advances once per
/// committed transaction, so it counts durable WRITES rather than facts.
fn durable_revision(root: &TempDir, card: RecordId) -> u64 {
    let card_root = root.path().join("cards").join(card.get().to_string());
    fs::read_to_string(card_root.join("current"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

struct AmbiguousStorage {
    inner: InMemoryStorage,
    fail_after_commit: bool,
}

impl Storage for AmbiguousStorage {
    type Error = AmbiguousError;
    type Lease = InMemoryWriterLease;
    type Transaction<'a> = AmbiguousTransaction<'a>;

    fn maximum_durability(&self) -> Durability {
        self.inner.maximum_durability()
    }

    fn get(&mut self, key: EnvelopeKey) -> Result<LoadOutcome, Self::Error> {
        self.inner.get(key).map_err(AmbiguousError::Inner)
    }

    fn quarantined(&self, key: EnvelopeKey) -> Result<Vec<QuarantinedEnvelope>, Self::Error> {
        self.inner.quarantined(key).map_err(AmbiguousError::Inner)
    }

    fn acquire_writer(
        &mut self,
        record_id: RecordId,
    ) -> Result<LeaseAcquisition<Self::Lease>, Self::Error> {
        self.inner
            .acquire_writer(record_id)
            .map_err(AmbiguousError::Inner)
    }

    fn release_writer(&mut self, lease: Self::Lease) -> Result<(), Self::Error> {
        self.inner
            .release_writer(lease)
            .map_err(AmbiguousError::Inner)
    }

    fn begin<'a>(&'a mut self, lease: &Self::Lease) -> Result<Self::Transaction<'a>, Self::Error> {
        let transaction = self.inner.begin(lease).map_err(AmbiguousError::Inner)?;
        Ok(AmbiguousTransaction {
            transaction,
            fail_after_commit: &mut self.fail_after_commit,
        })
    }

    fn manifest(&self, record_id: RecordId) -> Result<Option<CardManifest>, Self::Error> {
        self.inner
            .manifest(record_id)
            .map_err(AmbiguousError::Inner)
    }
}

struct AmbiguousTransaction<'a> {
    transaction: InMemoryTransaction<'a>,
    fail_after_commit: &'a mut bool,
}

impl StorageTransaction for AmbiguousTransaction<'_> {
    type Error = AmbiguousError;

    fn record_id(&self) -> RecordId {
        self.transaction.record_id()
    }

    fn put_operation(&mut self, envelope: OperationEnvelope) {
        self.transaction.put_operation(envelope);
    }

    fn put_artifact(
        &mut self,
        artifact_id: ArtifactId,
        name: OfferedName,
        envelope: OperationEnvelope,
    ) {
        self.transaction.put_artifact(artifact_id, name, envelope);
    }

    fn commit(self, durability: Durability) -> Result<CommitReceipt, Self::Error> {
        let receipt = self
            .transaction
            .commit(durability)
            .map_err(AmbiguousError::Inner)?;
        if std::mem::take(self.fail_after_commit) {
            return Err(AmbiguousError::AfterCommit);
        }
        Ok(receipt)
    }
}

#[derive(Debug)]
enum AmbiguousError {
    Inner(MemoryStorageError),
    AfterCommit,
}

impl std::fmt::Display for AmbiguousError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inner(error) => error.fmt(formatter),
            Self::AfterCommit => formatter.write_str("commit outcome is ambiguous"),
        }
    }
}

impl std::error::Error for AmbiguousError {}

// ---- EH-01: the state frame, and what it decides before the payload ----

/// THE defect, tested at the seam where it actually lived.
///
/// `load_image` used to run the positional decoder over the whole body and only
/// then compare the schema string it found inside. So a build whose shape had
/// moved interpreted old bytes under its own layout and answered
/// `CorruptState` — the version it needed was behind the parse it was meant to
/// guard.
///
/// The payload here is bytes the current decoder CANNOT parse. Answering
/// `UnsupportedStateSchema` is therefore only possible if the version was read
/// first; a reader that decoded the payload before checking would have to say
/// `CorruptState`.
#[test]
fn an_unknown_version_is_answered_before_its_payload_is_decoded() {
    let card = RecordId::new(71);
    let mut framed = crate::state_envelope::wrap(b"\xff\xff\xff\xff undecodable");
    // Bump the version in place: magic(4) + schema_len(2) + schema.
    let at = 4 + 2 + crate::identifiers::OPERATION_STORE_STATE_SCHEMA_ID.len();
    framed[at..at + 4].copy_from_slice(&99u32.to_be_bytes());

    let mut backend = InMemoryStorage::new();
    let lease = match backend.acquire_writer(card).expect("lease") {
        LeaseAcquisition::Acquired(lease) => lease,
        LeaseAcquisition::Busy => panic!("a fresh backend is not busy"),
    };
    let mut transaction = backend.begin(&lease).expect("begin");
    transaction.put_operation(OperationEnvelope::new(framed).expect("envelope"));
    transaction.commit(Durability::Buffered).expect("commit");
    backend.release_writer(lease).expect("release");

    assert!(
        matches!(
            OperationStore::open(backend, card),
            Err(StoreError::UnsupportedStateSchema)
        ),
        "an unreadable version must be typed as a version, not as corruption"
    );
}

/// A pre-frame body is recognisably not ours. It must never be handed to the
/// current decoder on the chance that it fits: the old layout begins with the
/// u32 length of its schema string, which is not the magic.
#[test]
fn a_body_without_the_frame_is_never_decoded_as_one() {
    let legacy =
        crate::codec::to_vec(&crate::StoreImage::empty(RecordId::new(1))).expect("encodes");
    assert!(matches!(
        crate::state_envelope::unwrap(&legacy),
        Err(crate::state_envelope::StateEnvelopeError::NotEnveloped)
    ));
}

/// Trailing bytes mean the writer and this reader disagree about where the
/// image ends. Accepting the declared prefix would turn that disagreement into
/// a successful load of the wrong thing.
#[test]
fn the_declared_payload_length_must_be_exact() {
    let mut framed = crate::state_envelope::wrap(b"payload");
    framed.push(0);
    assert!(matches!(
        crate::state_envelope::unwrap(&framed),
        Err(crate::state_envelope::StateEnvelopeError::Malformed)
    ));

    let framed = crate::state_envelope::wrap(b"payload");
    for truncated in 1..framed.len() {
        assert!(
            crate::state_envelope::unwrap(&framed[..truncated]).is_err(),
            "a frame cut at {truncated} bytes must not decode"
        );
    }
}

/// The frame's bytes, stated exactly. Any layout change — a reordered header
/// field, a different width, a renamed schema — fails here, which is what makes
/// a version bump a deliberate act rather than something to forget.
#[test]
fn the_state_frame_has_a_byte_exact_layout() {
    let framed = crate::state_envelope::wrap(b"ab");
    let schema = crate::identifiers::OPERATION_STORE_STATE_SCHEMA_ID.as_bytes();
    let mut expected = Vec::new();
    expected.extend_from_slice(b"EVOS");
    expected.extend_from_slice(&(schema.len() as u16).to_be_bytes());
    expected.extend_from_slice(schema);
    expected.extend_from_slice(&crate::state_envelope::STATE_FORMAT_VERSION.to_be_bytes());
    expected.extend_from_slice(&2u32.to_be_bytes());
    expected.extend_from_slice(b"ab");
    assert_eq!(framed, expected);
    assert_eq!(
        crate::state_envelope::unwrap(&framed).expect("round trips"),
        b"ab"
    );
}
