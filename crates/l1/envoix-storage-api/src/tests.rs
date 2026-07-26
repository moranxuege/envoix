use envoix_types::{ArtifactId, OfferedName, RecordId};

use crate::identifiers::OPERATION_ENVELOPE_SCHEMA_ID;
use crate::{
    CURRENT_ENVELOPE_VERSION, CommitReceipt, Durability, DurabilityContractError, EnvelopeError,
    EnvelopeKey, InMemoryStorage, LeaseAcquisition, LoadOutcome, MAX_ENVELOPE_BODY_BYTES,
    MemoryStorageError, OperationEnvelope, QuarantineReason, Storage, StorageTransaction,
    WriterLease,
};

const ENVELOPE_FIXTURE: &[u8] = &[
    0x00, 0x1b, b'e', b'n', b'v', b'o', b'i', b'x', b'/', b'o', b'p', b'e', b'r', b'a', b't', b'i',
    b'o', b'n', b'-', b'e', b'n', b'v', b'e', b'l', b'o', b'p', b'e', b'/', b'1', 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x03, 0xaa, 0xbb, 0xcc,
];

#[test]
fn storage_contract_semantics() {
    let mut store = InMemoryStorage::new();
    let record_id = RecordId::new(7);
    let operation_key = EnvelopeKey::Operation(record_id);
    let first_artifact = ArtifactId::from_bytes([0x11; 16]);
    let second_artifact = ArtifactId::from_bytes([0x22; 16]);

    assert_eq!(store.maximum_durability(), Durability::Buffered);
    assert_eq!(store.get(operation_key).unwrap(), LoadOutcome::Absent);

    let lease = acquired(store.acquire_writer(record_id).unwrap());
    assert_eq!(lease.record_id(), record_id);
    assert_eq!(
        store.acquire_writer(record_id).unwrap(),
        LeaseAcquisition::Busy
    );

    let envelope = OperationEnvelope::new([0xaa, 0xbb, 0xcc]).unwrap();
    assert_eq!(envelope.schema_id(), OPERATION_ENVELOPE_SCHEMA_ID);
    assert_eq!(envelope.version(), CURRENT_ENVELOPE_VERSION);
    assert!(OPERATION_ENVELOPE_SCHEMA_ID.ends_with(&format!("/{CURRENT_ENVELOPE_VERSION}")));
    assert_eq!(envelope.encode(), ENVELOPE_FIXTURE);

    let duplicate_name = OfferedName::from_untrusted("photo.jpg").unwrap();
    let mut transaction = store.begin(&lease).unwrap();
    assert_eq!(transaction.record_id(), record_id);
    transaction.put_operation(envelope.clone());
    transaction.put_artifact(
        first_artifact,
        duplicate_name.clone(),
        OperationEnvelope::new(b"artifact-one".to_vec()).unwrap(),
    );
    transaction.put_artifact(
        second_artifact,
        duplicate_name.clone(),
        OperationEnvelope::new(b"artifact-two".to_vec()).unwrap(),
    );
    let receipt = transaction.commit(Durability::Buffered).unwrap();
    assert_eq!(receipt.requested(), Durability::Buffered);
    assert_eq!(receipt.achieved(), Durability::Buffered);
    assert_eq!(
        CommitReceipt::observed(Durability::Durable, Durability::Flushed),
        Err(DurabilityContractError::AchievedBelowRequested {
            requested: Durability::Durable,
            achieved: Durability::Flushed,
        })
    );

    assert_eq!(
        store.get(operation_key).unwrap(),
        LoadOutcome::Loaded(envelope.clone())
    );
    let manifest = store.manifest(record_id).unwrap().unwrap();
    assert_eq!(manifest.record_id(), record_id);
    assert_eq!(manifest.committed_at(), Durability::Buffered);
    assert_eq!(manifest.artifacts().len(), 2);
    assert_eq!(
        manifest.artifacts().get(&first_artifact).unwrap().name(),
        &duplicate_name
    );
    assert_eq!(
        manifest
            .artifacts()
            .get(&second_artifact)
            .unwrap()
            .durability(),
        Durability::Buffered
    );

    let mut rejected = store.begin(&lease).unwrap();
    rejected.put_operation(OperationEnvelope::new(b"must-not-commit".to_vec()).unwrap());
    assert_eq!(
        rejected.commit(Durability::Flushed),
        Err(MemoryStorageError::UnsupportedDurability {
            requested: Durability::Flushed,
            maximum: Durability::Buffered,
        })
    );
    assert_eq!(
        store.get(operation_key).unwrap(),
        LoadOutcome::Loaded(envelope)
    );

    store.release_writer(lease).unwrap();
    let reacquired = acquired(store.acquire_writer(record_id).unwrap());
    store.release_writer(reacquired).unwrap();

    let corrupt_record = RecordId::new(90);
    let corrupt_key = EnvelopeKey::Operation(corrupt_record);
    let corrupt_bytes = ENVELOPE_FIXTURE[..ENVELOPE_FIXTURE.len() - 1].to_vec();
    store.seed_raw(corrupt_key, corrupt_bytes.clone());
    assert_eq!(
        store.get(corrupt_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::Corrupt
        }
    );
    assert_eq!(
        store.get(corrupt_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::Corrupt
        }
    );
    let quarantined = store.quarantined(corrupt_key).unwrap();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].reason(), QuarantineReason::Corrupt);
    assert_eq!(quarantined[0].bytes(), corrupt_bytes);

    let replacement_lease = acquired(store.acquire_writer(corrupt_record).unwrap());
    let replacement = OperationEnvelope::new(b"replacement".to_vec()).unwrap();
    let mut replacement_transaction = store.begin(&replacement_lease).unwrap();
    replacement_transaction.put_operation(replacement.clone());
    replacement_transaction
        .commit(Durability::Buffered)
        .unwrap();
    assert_eq!(
        store.get(corrupt_key).unwrap(),
        LoadOutcome::Loaded(replacement)
    );
    assert_eq!(
        store.quarantined(corrupt_key).unwrap()[0].bytes(),
        corrupt_bytes
    );
    store.release_writer(replacement_lease).unwrap();

    let future_key = EnvelopeKey::Operation(RecordId::new(91));
    let future_bytes = raw_envelope("envoix/operation-envelope/2", 2, b"future");
    store.seed_raw(future_key, future_bytes.clone());
    assert_eq!(
        store.get(future_key).unwrap(),
        LoadOutcome::Quarantined {
            reason: QuarantineReason::UnsupportedFuture
        }
    );
    assert_eq!(
        store.quarantined(future_key).unwrap()[0].bytes(),
        future_bytes
    );

    assert_eq!(
        store
            .get(EnvelopeKey::Operation(RecordId::new(999)))
            .unwrap(),
        LoadOutcome::Absent
    );
}

#[test]
fn envelope_and_lease_boundaries_are_closed() {
    let empty = OperationEnvelope::new(Vec::new()).unwrap();
    assert_eq!(empty.body().as_bytes(), []);

    let maximum = OperationEnvelope::new(vec![0x5a; MAX_ENVELOPE_BODY_BYTES]).unwrap();
    let maximum_key = EnvelopeKey::Operation(RecordId::new(1));
    let mut store = InMemoryStorage::new();
    store.seed_raw(maximum_key, maximum.encode());
    assert!(matches!(
        store.get(maximum_key).unwrap(),
        LoadOutcome::Loaded(envelope)
            if envelope.body().as_bytes().len() == MAX_ENVELOPE_BODY_BYTES
    ));
    assert_eq!(
        OperationEnvelope::new(vec![0; MAX_ENVELOPE_BODY_BYTES + 1]),
        Err(EnvelopeError::BodyTooLarge {
            actual: MAX_ENVELOPE_BODY_BYTES + 1,
            maximum: MAX_ENVELOPE_BODY_BYTES,
        })
    );

    for truncated_at in 0..ENVELOPE_FIXTURE.len() {
        let key = EnvelopeKey::Operation(RecordId::new(100 + truncated_at as u64));
        store.seed_raw(key, ENVELOPE_FIXTURE[..truncated_at].to_vec());
        assert_eq!(
            store.get(key).unwrap(),
            LoadOutcome::Quarantined {
                reason: QuarantineReason::Corrupt
            },
            "truncation at {truncated_at}"
        );
    }

    for (record, bytes) in [
        (300, raw_envelope(OPERATION_ENVELOPE_SCHEMA_ID, 0, b"no-v0")),
        (
            301,
            raw_envelope("unknown/operation-envelope/1", 1, b"unknown"),
        ),
    ] {
        let key = EnvelopeKey::Operation(RecordId::new(record));
        store.seed_raw(key, bytes);
        assert_eq!(
            store.get(key).unwrap(),
            LoadOutcome::Quarantined {
                reason: QuarantineReason::Corrupt
            }
        );
    }

    let first_store_lease = acquired(store.acquire_writer(RecordId::new(400)).unwrap());
    let mut other_store = InMemoryStorage::new();
    assert_eq!(
        other_store.begin(&first_store_lease).err(),
        Some(MemoryStorageError::InvalidLease)
    );
    store.release_writer(first_store_lease).unwrap();

    let dropped_record = RecordId::new(401);
    let dropped_lease = acquired(store.acquire_writer(dropped_record).unwrap());
    {
        let mut transaction = store.begin(&dropped_lease).unwrap();
        transaction.put_operation(OperationEnvelope::new(b"uncommitted".to_vec()).unwrap());
    }
    assert_eq!(
        store.get(EnvelopeKey::Operation(dropped_record)).unwrap(),
        LoadOutcome::Absent
    );
    store.release_writer(dropped_lease).unwrap();
}

fn acquired<L>(result: LeaseAcquisition<L>) -> L {
    match result {
        LeaseAcquisition::Acquired(lease) => lease,
        LeaseAcquisition::Busy => panic!("writer lease unexpectedly busy"),
    }
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
