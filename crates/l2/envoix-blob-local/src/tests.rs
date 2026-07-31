use envoix_blob_api::{BlobKey, BlobState, BlobStore, DerivationWorkId};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, ContentHash, RecordId};
use tempfile::TempDir;

use crate::LocalBlobs;

fn blob(generation: u32) -> BlobKey {
    BlobKey::new(
        RecordId::new(9),
        DerivationWorkId::of(AttemptGen::new(generation), ArtifactId::from_bytes([3; 16])),
    )
}

fn hash(byte: u8) -> ContentHash {
    ContentHash::from_bytes([byte; 32])
}

fn store(root: &TempDir) -> BlobStore<LocalBlobs> {
    BlobStore::new(LocalBlobs::new(root.path()))
}

/// The one rule: an artifact with no seal is not a source.
///
/// A partial the same length as a finished one is exactly what a crash leaves,
/// so completion can never be a property a reader measures. It is a fact the
/// store publishes, and until it has, the bytes cannot be read at all.
#[test]
fn a_partial_is_not_readable_however_complete_it_looks() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);

    let mut lease = store.begin(key, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"whole file")
        .expect("appends");
    lease.checkpoint(hash(2)).expect("a checkpoint");
    drop(lease);

    assert!(matches!(
        store.inspect(key),
        Ok(BlobState::Partial {
            durable_checkpoint: Some(_)
        })
    ));
    assert_eq!(store.sealed(key), Ok(None));
    let mut into = [0_u8; 10];
    assert!(
        store.read_at(key, ByteCount::new(0), &mut into).is_err(),
        "an unsealed blob was read as a source"
    );

    // And once sealed, the same bytes ARE readable — so the refusal is about
    // the seal, not about the bytes.
    let lease = store.begin(key, hash(1)).expect("a lease");
    let sealed = lease.seal(hash(3)).expect("seals");
    assert_eq!(sealed.length(), ByteCount::new(10));
    assert_eq!(store.read_at(key, ByteCount::new(0), &mut into), Ok(10));
    assert_eq!(&into, b"whole file");
}

/// A lease opens at the last DURABLE prefix, and an uncheckpointed tail is gone.
///
/// Nothing promised those bytes. Resuming over them would let a digest cover
/// something no sync ever made durable, which is the one lie the seal exists to
/// prevent.
#[test]
fn an_uncheckpointed_tail_does_not_survive_the_lease() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);

    let mut lease = store.begin(key, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"promised")
        .expect("appends");
    lease.checkpoint(hash(2)).expect("a checkpoint");
    lease
        .append(ByteCount::new(8), b"NOT PROMISED")
        .expect("appends");
    // The worker dies here: no second checkpoint, no seal.
    drop(lease);

    let lease = store.begin(key, hash(1)).expect("a second lease");
    assert_eq!(
        lease.offset(),
        ByteCount::new(8),
        "a resumed lease opened over bytes nothing had promised"
    );
    drop(lease);

    let mut lease = store.begin(key, hash(1)).expect("a third lease");
    lease.append(ByteCount::new(8), b" tail").expect("appends");
    lease.seal(hash(3)).expect("seals");
    let mut into = [0_u8; 13];
    assert_eq!(store.read_at(key, ByteCount::new(0), &mut into), Ok(13));
    assert_eq!(&into, b"promised tail");
}

/// A checkpoint belongs to the work that produced it.
///
/// Resuming a copy from a prefix a DIFFERENT selection wrote splices two
/// documents together, and the offset looks perfectly usable while it does. So
/// the fingerprint decides eligibility, not the offset.
#[test]
fn a_checkpoint_from_other_work_is_ineligible() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);

    let mut lease = store.begin(key, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"from work one")
        .expect("appends");
    lease.checkpoint(hash(2)).expect("a checkpoint");
    drop(lease);

    let lease = store
        .begin(key, hash(0xaa))
        .expect("a lease for other work");
    assert_eq!(
        lease.offset(),
        ByteCount::new(0),
        "a prefix from different work was resumed over"
    );
}

/// Sealed bytes are immutable, and one incarnation has one writer.
#[test]
fn a_sealed_blob_refuses_a_writer_and_a_live_one_refuses_a_second() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);

    let held = store.begin(key, hash(1)).expect("a lease");
    assert_eq!(
        store.begin(key, hash(1)).err(),
        Some(envoix_blob_api::BlobError::AlreadyWriting)
    );
    held.seal(hash(2)).expect("seals");

    assert_eq!(
        store.begin(key, hash(1)).err(),
        Some(envoix_blob_api::BlobError::Sealed),
        "sealed bytes were reopened for writing"
    );
}

/// The engine owns the offset. A gap or an overlap is a lost or doubled write,
/// not something to reconcile into place.
#[test]
fn an_append_at_the_wrong_offset_is_refused() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let mut lease = store.begin(blob(1), hash(1)).expect("a lease");
    lease.append(ByteCount::new(0), b"abc").expect("appends");

    assert_eq!(
        lease.append(ByteCount::new(7), b"gap").err(),
        Some(envoix_blob_api::BlobError::OffsetMismatch {
            expected: ByteCount::new(3)
        })
    );
    assert_eq!(
        lease.append(ByteCount::new(1), b"over").err(),
        Some(envoix_blob_api::BlobError::OffsetMismatch {
            expected: ByteCount::new(3)
        })
    );
}

/// A re-derivation is a different INCARNATION, so it cannot touch the previous
/// one's bytes — even though both name the same logical artifact.
#[test]
fn a_later_incarnation_does_not_disturb_the_earlier_one() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);

    let mut first = store.begin(blob(1), hash(1)).expect("a lease");
    first.append(ByteCount::new(0), b"first").expect("appends");
    first.seal(hash(2)).expect("seals");

    let mut second = store.begin(blob(2), hash(1)).expect("a lease");
    assert_eq!(second.offset(), ByteCount::new(0));
    second
        .append(ByteCount::new(0), b"second")
        .expect("appends");
    second.seal(hash(3)).expect("seals");

    let mut into = [0_u8; 5];
    assert_eq!(store.read_at(blob(1), ByteCount::new(0), &mut into), Ok(5));
    assert_eq!(&into, b"first");

    // Both are the card's, so a sweeper can see both — which is what lets one be
    // deleted without guessing at the other.
    assert_eq!(store.owned(RecordId::new(9)), Ok(vec![blob(1), blob(2)]));
    store.delete(blob(1)).expect("deletes");
    assert_eq!(store.owned(RecordId::new(9)), Ok(vec![blob(2)]));
    assert_eq!(store.inspect(blob(1)), Ok(BlobState::Absent));
    // Idempotent: the caller asked for absent, and it already is.
    assert_eq!(store.delete(blob(1)), Ok(()));
}
