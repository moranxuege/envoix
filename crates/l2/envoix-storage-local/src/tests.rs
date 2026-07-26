use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use envoix_storage_api::{
    Durability, EnvelopeKey, LeaseAcquisition, LoadOutcome, MAX_ENVELOPE_BODY_BYTES,
    OperationEnvelope, QuarantineReason, Storage, StorageTransaction, WriterLease,
};
use envoix_types::{ArtifactId, OfferedName, RecordId};

use crate::local::CommitStage;
use crate::{LocalStorage, LocalStorageError};

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(1);

#[test]
fn storage_local_fault_each_commit_stage() {
    for stage in CommitStage::ALL {
        let root = TempRoot::new("fault-stage");
        let card = RecordId::new(7);
        let operation_key = EnvelopeKey::Operation(card);
        let first_artifact = ArtifactId::from_bytes([0x11; 16]);
        let second_artifact = ArtifactId::from_bytes([0x22; 16]);
        let first_key = EnvelopeKey::Artifact {
            record_id: card,
            artifact_id: first_artifact,
        };
        let second_key = EnvelopeKey::Artifact {
            record_id: card,
            artifact_id: second_artifact,
        };

        let mut store = LocalStorage::open(root.path()).unwrap();
        let lease = acquired(store.acquire_writer(card).unwrap());
        let mut baseline = store.begin(&lease).unwrap();
        baseline.put_operation(envelope(b"operation-old"));
        baseline.put_artifact(
            first_artifact,
            OfferedName::from_untrusted("same-name.bin").unwrap(),
            envelope(b"artifact-old"),
        );
        baseline.commit(Durability::Durable).unwrap();

        store.inject_fault(stage);
        let mut update = store.begin(&lease).unwrap();
        update.put_operation(envelope(b"operation-new"));
        update.put_artifact(
            first_artifact,
            OfferedName::from_untrusted("same-name.bin").unwrap(),
            envelope(b"artifact-new"),
        );
        update.put_artifact(
            second_artifact,
            OfferedName::from_untrusted("same-name.bin").unwrap(),
            envelope(b"artifact-second"),
        );
        assert!(matches!(
            update.commit(Durability::Durable),
            Err(LocalStorageError::InjectedFault)
        ));
        drop(store);

        let mut reopened = LocalStorage::open(root.path()).unwrap();
        if stage.is_after_linearization() {
            assert_loaded(&mut reopened, operation_key, b"operation-new");
            assert_loaded(&mut reopened, first_key, b"artifact-new");
            assert_loaded(&mut reopened, second_key, b"artifact-second");
            assert_eq!(
                reopened.manifest(card).unwrap().unwrap().artifacts().len(),
                2
            );
        } else {
            assert_loaded(&mut reopened, operation_key, b"operation-old");
            assert_loaded(&mut reopened, first_key, b"artifact-old");
            assert_eq!(reopened.get(second_key).unwrap(), LoadOutcome::Absent);
            assert_eq!(
                reopened.manifest(card).unwrap().unwrap().artifacts().len(),
                1
            );
        }
    }
}

#[test]
fn durable_round_trip_and_identity_keyed_manifest() {
    let root = TempRoot::new("round-trip");
    let card = RecordId::new(20);
    let operation_key = EnvelopeKey::Operation(card);
    let first_artifact = ArtifactId::from_bytes([0x31; 16]);
    let second_artifact = ArtifactId::from_bytes([0x32; 16]);
    let first_key = EnvelopeKey::Artifact {
        record_id: card,
        artifact_id: first_artifact,
    };
    let second_key = EnvelopeKey::Artifact {
        record_id: card,
        artifact_id: second_artifact,
    };

    let mut store = LocalStorage::open(root.path()).unwrap();
    assert_eq!(store.maximum_durability(), Durability::Durable);
    assert_eq!(store.get(operation_key).unwrap(), LoadOutcome::Absent);
    let lease = acquired(store.acquire_writer(card).unwrap());

    let same_name = OfferedName::from_untrusted("../photo.jpg").unwrap();
    let mut buffered = store.begin(&lease).unwrap();
    buffered.put_operation(envelope([]));
    buffered.put_artifact(
        first_artifact,
        same_name.clone(),
        envelope(vec![0x5a; MAX_ENVELOPE_BODY_BYTES]),
    );
    buffered.put_artifact(second_artifact, same_name.clone(), envelope(b"second"));
    let receipt = buffered.commit(Durability::Buffered).unwrap();
    assert_eq!(receipt.requested(), Durability::Buffered);
    assert_eq!(receipt.achieved(), Durability::Buffered);

    let mut flushed = store.begin(&lease).unwrap();
    flushed.put_operation(envelope(b"flushed"));
    let receipt = flushed.commit(Durability::Flushed).unwrap();
    assert_eq!(receipt.requested(), Durability::Flushed);
    assert_eq!(receipt.achieved(), Durability::Flushed);

    let mut durable = store.begin(&lease).unwrap();
    durable.put_operation(envelope(b"durable"));
    let receipt = durable.commit(Durability::Durable).unwrap();
    assert_eq!(receipt.requested(), Durability::Durable);
    assert_eq!(receipt.achieved(), Durability::Durable);
    store.release_writer(lease).unwrap();
    drop(store);

    let mut reopened = LocalStorage::open(root.path()).unwrap();
    assert_loaded(&mut reopened, operation_key, b"durable");
    assert_loaded(
        &mut reopened,
        first_key,
        &vec![0x5a; MAX_ENVELOPE_BODY_BYTES],
    );
    assert_loaded(&mut reopened, second_key, b"second");

    let manifest = reopened.manifest(card).unwrap().unwrap();
    assert_eq!(manifest.record_id(), card);
    assert_eq!(manifest.committed_at(), Durability::Durable);
    assert_eq!(manifest.artifacts().len(), 2);
    assert_eq!(
        manifest.artifacts().get(&first_artifact).unwrap().name(),
        &same_name
    );
    assert_eq!(
        manifest
            .artifacts()
            .get(&first_artifact)
            .unwrap()
            .durability(),
        Durability::Buffered
    );
}

#[test]
fn quarantine_absent_and_replacement_remain_distinct() {
    let root = TempRoot::new("quarantine");
    let card = RecordId::new(30);
    let operation_key = EnvelopeKey::Operation(card);
    let artifact_id = ArtifactId::from_bytes([0x44; 16]);
    let artifact_key = EnvelopeKey::Artifact {
        record_id: card,
        artifact_id,
    };
    let corrupt_bytes = vec![0x00, 0x01, 0x02];
    let future_bytes = raw_envelope("envoix/operation-envelope/2", 2, b"future");

    let mut store = LocalStorage::open(root.path()).unwrap();
    let lease = acquired(store.acquire_writer(card).unwrap());
    let mut initial = store.begin(&lease).unwrap();
    initial.put_operation(envelope(b"operation"));
    initial.put_artifact(
        artifact_id,
        OfferedName::from_untrusted("data.bin").unwrap(),
        envelope(b"artifact"),
    );
    initial.commit(Durability::Durable).unwrap();

    store.overwrite_live(operation_key, &corrupt_bytes).unwrap();
    assert_eq!(
        store.get(operation_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::Corrupt
        }
    );
    assert_eq!(
        store.get(operation_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::Corrupt
        }
    );
    let operation_history = store.quarantined(operation_key).unwrap();
    assert_eq!(operation_history.len(), 1);
    assert_eq!(operation_history[0].bytes(), corrupt_bytes);

    store.release_writer(lease).unwrap();
    drop(store);
    let mut store = LocalStorage::open(root.path()).unwrap();
    assert_eq!(
        store.get(operation_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::Corrupt
        }
    );
    let lease = acquired(store.acquire_writer(card).unwrap());
    let mut replace_operation = store.begin(&lease).unwrap();
    replace_operation.put_operation(envelope(b"replacement"));
    replace_operation.commit(Durability::Durable).unwrap();
    assert_loaded(&mut store, operation_key, b"replacement");
    assert_eq!(store.quarantined(operation_key).unwrap().len(), 1);

    store.overwrite_live(artifact_key, &future_bytes).unwrap();
    assert_eq!(
        store.get(artifact_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::UnsupportedFuture
        }
    );
    let artifact_history = store.quarantined(artifact_key).unwrap();
    assert_eq!(artifact_history.len(), 1);
    assert_eq!(artifact_history[0].bytes(), future_bytes);

    store.release_writer(lease).unwrap();
    drop(store);
    let mut store = LocalStorage::open(root.path()).unwrap();
    assert_eq!(
        store.get(artifact_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::UnsupportedFuture
        }
    );
    let lease = acquired(store.acquire_writer(card).unwrap());
    let mut replace_artifact = store.begin(&lease).unwrap();
    replace_artifact.put_artifact(
        artifact_id,
        OfferedName::from_untrusted("data.bin").unwrap(),
        envelope(b"artifact-replacement"),
    );
    replace_artifact.commit(Durability::Durable).unwrap();
    store.release_writer(lease).unwrap();
    drop(store);

    let mut reopened = LocalStorage::open(root.path()).unwrap();
    assert_loaded(&mut reopened, operation_key, b"replacement");
    assert_loaded(&mut reopened, artifact_key, b"artifact-replacement");
    assert_eq!(reopened.quarantined(operation_key).unwrap().len(), 1);
    assert_eq!(reopened.quarantined(artifact_key).unwrap().len(), 1);
    assert_eq!(
        reopened
            .get(EnvelopeKey::Operation(RecordId::new(999)))
            .unwrap(),
        LoadOutcome::Absent
    );
}

#[test]
fn checked_cleanup_does_not_adopt_unindexed_artifacts() {
    let root = TempRoot::new("checked-cleanup");
    let card = RecordId::new(35);
    let operation_key = EnvelopeKey::Operation(card);
    let stray_artifact = ArtifactId::from_bytes([0x99; 16]);
    let stray_key = EnvelopeKey::Artifact {
        record_id: card,
        artifact_id: stray_artifact,
    };

    let mut store = LocalStorage::open(root.path()).unwrap();
    let lease = acquired(store.acquire_writer(card).unwrap());
    let mut transaction = store.begin(&lease).unwrap();
    transaction.put_operation(envelope(b"committed"));
    transaction.commit(Durability::Durable).unwrap();
    store.release_writer(lease).unwrap();
    drop(store);

    let card_dir = root.path().join("cards").join(card.get().to_string());
    let revision = fs::read_to_string(card_dir.join("current")).unwrap();
    let artifacts_dir = card_dir
        .join("revisions")
        .join(revision.trim())
        .join("artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();
    let stray_path = artifacts_dir.join(format!("{}.env", artifact_hex(stray_artifact)));
    fs::write(&stray_path, envelope(b"stray").encode()).unwrap();

    let mut reopened = LocalStorage::open(root.path()).unwrap();
    assert_loaded(&mut reopened, operation_key, b"committed");
    assert_eq!(reopened.get(stray_key).unwrap(), LoadOutcome::Absent);
    assert!(!stray_path.exists());
}

#[test]
fn leases_are_exclusive_backend_bound_and_reacquirable() {
    let first_root = TempRoot::new("lease-first");
    let second_root = TempRoot::new("lease-second");
    let card = RecordId::new(40);
    let dropped_card = RecordId::new(41);
    let mut first = LocalStorage::open(first_root.path()).unwrap();
    let mut second = LocalStorage::open(second_root.path()).unwrap();

    let lease = acquired(first.acquire_writer(card).unwrap());
    assert_eq!(lease.record_id(), card);
    assert_eq!(first.acquire_writer(card).unwrap(), LeaseAcquisition::Busy);
    assert!(matches!(
        second.begin(&lease),
        Err(LocalStorageError::InvalidLease)
    ));
    first.release_writer(lease).unwrap();
    let reacquired = acquired(first.acquire_writer(card).unwrap());
    first.release_writer(reacquired).unwrap();

    let dropped_lease = acquired(first.acquire_writer(dropped_card).unwrap());
    {
        let mut transaction = first.begin(&dropped_lease).unwrap();
        transaction.put_operation(envelope(b"never committed"));
    }
    assert_eq!(
        first.get(EnvelopeKey::Operation(dropped_card)).unwrap(),
        LoadOutcome::Absent
    );
    first.release_writer(dropped_lease).unwrap();

    let stale_lease = acquired(first.acquire_writer(card).unwrap());
    drop(first);
    let mut reopened = LocalStorage::open(first_root.path()).unwrap();
    assert!(matches!(
        reopened.begin(&stale_lease),
        Err(LocalStorageError::InvalidLease)
    ));
}

fn envelope(body: impl Into<Vec<u8>>) -> OperationEnvelope {
    OperationEnvelope::new(body).unwrap()
}

fn acquired<L>(result: LeaseAcquisition<L>) -> L {
    match result {
        LeaseAcquisition::Acquired(lease) => lease,
        LeaseAcquisition::Busy => panic!("writer lease unexpectedly busy"),
    }
}

fn assert_loaded(store: &mut LocalStorage, key: EnvelopeKey, expected: &[u8]) {
    let LoadOutcome::Loaded(envelope) = store.get(key).unwrap() else {
        panic!("expected a loaded envelope for {key:?}");
    };
    assert_eq!(envelope.body().as_bytes(), expected);
}

fn raw_envelope(schema: &str, version: u32, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(schema.len() as u16).to_be_bytes());
    bytes.extend_from_slice(schema.as_bytes());
    bytes.extend_from_slice(&version.to_be_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

fn artifact_hex(artifact_id: ArtifactId) -> String {
    artifact_id
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "envoix-storage-local-{label}-{}-{sequence}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
