use envoix_blob_api::{BlobKey, BlobState, BlobStore, BlobWorkId};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, ContentHash, RecordId};
use tempfile::TempDir;

use crate::LocalBlobs;

fn blob(generation: u32) -> BlobKey {
    BlobKey::new(
        RecordId::new(9),
        BlobWorkId::of_derivation(AttemptGen::new(generation), ArtifactId::from_bytes([3; 16])),
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

/// A reception keeps its partial across an attempt resume; a derivation keeps
/// its across a re-pick. Neither key contains what the other's retries move.
///
/// This is the fourth place the attempt-versus-stable generation distinction has
/// decided something, and the first where the type prevents it: a reception has
/// no generation to accidentally reach for.
#[test]
fn the_two_kinds_of_work_are_stable_under_different_retries() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let card = RecordId::new(9);
    let artifact = ArtifactId::from_bytes([3; 16]);
    let transfer = envoix_types::TransferId::from_bytes([8; 16]);

    let received = BlobKey::new(card, BlobWorkId::of_reception(transfer, artifact));
    let mut lease = store.begin(received, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"received")
        .expect("appends");
    lease.checkpoint(hash(2)).expect("a checkpoint");
    drop(lease);

    // Nothing an attempt generation could change is in this key, so the same
    // transfer re-opens the same partial.
    let again = BlobKey::new(card, BlobWorkId::of_reception(transfer, artifact));
    assert_eq!(
        store.begin(again, hash(1)).expect("a lease").offset(),
        ByteCount::new(8)
    );

    // A derivation of the same artifact is a DIFFERENT blob, so neither can
    // stumble onto the other's bytes.
    let derived = BlobKey::new(
        card,
        BlobWorkId::of_derivation(AttemptGen::new(1), artifact),
    );
    assert_eq!(
        store.begin(derived, hash(1)).expect("a lease").offset(),
        ByteCount::new(0)
    );
    let mut owned = store.owned(card).expect("owned");
    owned.sort_unstable();
    assert_eq!(owned.len(), 2, "the two kinds collapsed onto one blob");
    assert!(owned.contains(&received) && owned.contains(&derived));
}

/// A writer may read back what IT wrote, and only up to its own offset.
///
/// Not an exception to "an unsealed artifact is not a source" — the ambient
/// `read_at` still refuses this blob. The lease is the capability, and holding
/// it is what makes reading your own work legitimate.
#[test]
fn a_lease_reads_its_own_partial_and_no_further() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);
    let mut lease = store.begin(key, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"written")
        .expect("appends");

    let mut into = [0_u8; 7];
    assert_eq!(lease.read_partial_at(ByteCount::new(0), &mut into), Ok(7));
    assert_eq!(&into, b"written");
    // Past its own offset there is nothing to promise, whatever the medium has.
    assert_eq!(lease.read_partial_at(ByteCount::new(7), &mut into), Ok(0));

    // And the ambient reader still refuses the same blob, because it has no
    // lease and the blob has no seal.
    assert!(store.read_at(key, ByteCount::new(0), &mut into).is_err());
}

/// `reset` publishes the zero prefix BEFORE truncating.
///
/// The other order leaves a checkpoint naming bytes that were already
/// shortened — a promise about something that is not there — and a caller doing
/// the two operations itself has no way to notice it got them backwards.
#[test]
fn a_reset_never_leaves_a_checkpoint_ahead_of_the_bytes() {
    let root = TempDir::new().expect("a root");
    let store = store(&root);
    let key = blob(1);

    let mut lease = store.begin(key, hash(1)).expect("a lease");
    lease
        .append(ByteCount::new(0), b"a longer prefix")
        .expect("appends");
    lease.checkpoint(hash(2)).expect("a checkpoint");
    lease.reset().expect("resets");
    assert_eq!(lease.offset(), ByteCount::new(0));
    drop(lease);

    // Whatever a crash interrupts, the durable checkpoint never names more than
    // the bytes: a reopened lease starts at zero rather than at the old prefix.
    assert_eq!(
        store.begin(key, hash(1)).expect("a lease").offset(),
        ByteCount::new(0),
        "a reset left a checkpoint naming bytes that had been truncated"
    );
}
