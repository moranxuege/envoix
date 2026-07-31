//! The state machine, and the backend it drives.
//!
//! Split in two on purpose. [`BlobBackend`] is what a medium implements — raw
//! writes, syncs, and publishing two small facts — and it can live anywhere.
//! [`BlobStore`] is the rules, and it lives HERE, in the same crate as
//! [`SealedArtifact`], because minting that witness is the one thing no backend
//! may do for itself. A backend hands up bytes and durability; whether they
//! amount to a sealed artifact is this crate's decision.

use envoix_types::{ByteCount, ContentHash, RecordId};

use crate::seal::SealedArtifact;
use crate::{BlobKey, CopyCheckpoint, SealFact};

/// What a blob is, as the store can honestly say after any crash.
///
/// Three states and no fourth. In particular there is no "complete but
/// unsealed": completion is a fact the store publishes atomically, never
/// something a reader infers from a file's length, because a length is exactly
/// what a half-written blob also has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlobState {
    Absent,
    /// Bytes exist. `durable_checkpoint` is the last prefix the store promised —
    /// `None` when a run wrote and died before promising anything, which is a
    /// partial with nothing usable in it.
    Partial {
        durable_checkpoint: Option<CopyCheckpoint>,
    },
    Sealed(SealFact),
}

/// Why a bulk operation did not happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlobError {
    /// Another writer holds this blob. One incarnation, one writer.
    AlreadyWriting,
    /// The blob is sealed. Sealed bytes are immutable — a second run wanting to
    /// change them wants a different incarnation.
    Sealed,
    /// A write arrived for an offset that is not where this blob currently ends.
    /// The engine owns the offset; a gap or an overlap is a lost or doubled
    /// write, not something to reconcile.
    OffsetMismatch { expected: ByteCount },
    /// The volume is full. Distinct from every other fault because it is the one
    /// a person can act on — and re-choosing the same documents does not fix it,
    /// which is why it must not become a re-pick.
    OutOfSpace,
    /// The backend failed.
    Storage,
}

/// A medium that can hold bulk bytes.
///
/// Deliberately small and dumb. It writes, it syncs, and it publishes two facts
/// atomically; it decides nothing about what those facts mean. Every rule —
/// what may follow what, what a crash leaves, when a seal is allowed — is
/// [`BlobStore`]'s, so a second backend cannot get the rules subtly different.
pub trait BlobBackend: Send + Sync + 'static {
    /// The bytes and facts currently on the medium.
    fn state(&self, blob: BlobKey) -> Result<BlobState, BlobError>;

    /// Takes exclusive write ownership. `AlreadyWriting` if someone holds it.
    fn acquire(&self, blob: BlobKey) -> Result<(), BlobError>;

    fn release(&self, blob: BlobKey);

    /// Discards everything after `length`, so a lease opens at a prefix the
    /// medium has actually promised rather than at whatever a dead writer left.
    fn truncate(&self, blob: BlobKey, length: ByteCount) -> Result<(), BlobError>;

    /// Appends at exactly `offset`. The caller has already checked it; this may
    /// check again but must never silently write somewhere else.
    fn append_at(&self, blob: BlobKey, offset: ByteCount, bytes: &[u8]) -> Result<(), BlobError>;

    /// Makes every byte written so far durable. Returns when they are.
    fn sync(&self, blob: BlobKey) -> Result<(), BlobError>;

    /// Publishes a checkpoint atomically. Called only after [`Self::sync`], so
    /// it may assume the prefix it names is durable.
    fn publish_checkpoint(&self, checkpoint: CopyCheckpoint) -> Result<(), BlobError>;

    /// Publishes a seal atomically, making the blob immutable. Called only after
    /// [`Self::sync`].
    fn publish_seal(&self, fact: SealFact) -> Result<(), BlobError>;

    fn remove(&self, blob: BlobKey) -> Result<(), BlobError>;

    fn owned(&self, card: RecordId) -> Result<Vec<BlobKey>, BlobError>;

    /// Reads `length` bytes of a sealed blob at `offset`.
    fn read_at(
        &self,
        blob: BlobKey,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, BlobError>;
}

/// Bulk storage for produced artifacts: the rules, over any medium.
///
/// Its own writer lease, independent of the operation store's. The derivation
/// worker holds this one for as long as it takes to produce gigabytes; the card
/// actor goes on committing progress and commands through the record store
/// meanwhile. Sharing one lease would let a crashed worker look like a card
/// writer and block every record commit for that card.
///
/// The two leases never overlap in the other direction either: a worker seals
/// and releases before the actor commits `Ready`, so there is no order in which
/// they can deadlock.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlobStore<B> {
    backend: B,
}

impl<B: BlobBackend> BlobStore<B> {
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    /// What this blob is, after whatever happened last time. Never infers
    /// completion from a length.
    pub fn inspect(&self, blob: BlobKey) -> Result<BlobState, BlobError> {
        self.backend.state(blob)
    }

    /// Takes the writer lease for one blob, positioned at its last durable
    /// checkpoint.
    ///
    /// An uncheckpointed tail is DISCARDED rather than trusted: nothing promised
    /// it, and resuming from bytes whose durability nobody stated is how a
    /// digest ends up covering something that is not on disk.
    ///
    /// A checkpoint from different work is ineligible however useful its offset
    /// looks — resuming a copy from a prefix another selection wrote splices two
    /// documents together — so the lease opens at zero instead.
    pub fn begin(
        &self,
        blob: BlobKey,
        fingerprint: ContentHash,
    ) -> Result<BlobLease<'_, B>, BlobError> {
        match self.backend.state(blob)? {
            BlobState::Sealed(_) => return Err(BlobError::Sealed),
            BlobState::Absent | BlobState::Partial { .. } => {}
        }
        self.backend.acquire(blob)?;
        let resume = match self.backend.state(blob) {
            Ok(BlobState::Partial {
                durable_checkpoint: Some(checkpoint),
            }) if checkpoint.fingerprint == fingerprint => checkpoint.length,
            Ok(_) => ByteCount::new(0),
            Err(error) => {
                self.backend.release(blob);
                return Err(error);
            }
        };
        if let Err(error) = self.backend.truncate(blob, resume) {
            self.backend.release(blob);
            return Err(error);
        }
        Ok(BlobLease {
            store: self,
            blob,
            fingerprint,
            offset: resume,
            released: false,
        })
    }

    /// The seal of a sealed blob. `None` for anything else — including a
    /// complete-looking partial, which is the whole point.
    pub fn sealed(&self, blob: BlobKey) -> Result<Option<SealFact>, BlobError> {
        Ok(match self.backend.state(blob)? {
            BlobState::Sealed(fact) => Some(fact),
            BlobState::Absent | BlobState::Partial { .. } => None,
        })
    }

    /// Reads a SEALED blob positionally. Refuses anything else: an artifact with
    /// no seal is not a source.
    pub fn read_at(
        &self,
        blob: BlobKey,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, BlobError> {
        match self.backend.state(blob)? {
            BlobState::Sealed(_) => self.backend.read_at(blob, offset, destination),
            BlobState::Absent | BlobState::Partial { .. } => Err(BlobError::Storage),
        }
    }

    /// Removes a blob and everything it left behind, sealed or partial.
    /// Idempotent: the caller's job is to make it absent, and it already is.
    pub fn delete(&self, blob: BlobKey) -> Result<(), BlobError> {
        self.backend.remove(blob)
    }

    /// Every blob this card owns, in any state. What a sweeper compares against
    /// the durable records to find what nothing references.
    pub fn owned(&self, card: RecordId) -> Result<Vec<BlobKey>, BlobError> {
        self.backend.owned(card)
    }
}

/// One held writer lease.
///
/// Dropping it releases the lease and leaves the blob `Partial` at its last
/// durable checkpoint. That is the honest outcome of a worker that stopped: the
/// bytes it did not promise are gone, and the ones it did are still there.
pub struct BlobLease<'a, B: BlobBackend> {
    store: &'a BlobStore<B>,
    blob: BlobKey,
    fingerprint: ContentHash,
    offset: ByteCount,
    released: bool,
}

impl<B: BlobBackend> BlobLease<'_, B> {
    /// Where the next append must start.
    pub const fn offset(&self) -> ByteCount {
        self.offset
    }

    /// Appends at exactly `offset`. The engine owns the offset, so a mismatch is
    /// a lost or doubled write and is refused rather than reconciled.
    pub fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), BlobError> {
        if offset != self.offset {
            return Err(BlobError::OffsetMismatch {
                expected: self.offset,
            });
        }
        self.store.backend.append_at(self.blob, offset, bytes)?;
        self.offset = ByteCount::new(self.offset.get().saturating_add(bytes.len() as u64));
        Ok(())
    }

    /// Promises everything written so far and publishes the prefix naming it.
    ///
    /// Sync FIRST, then publish. The other order names a prefix that may not
    /// survive, and a resumed run would then append to bytes that are not there.
    pub fn checkpoint(&mut self, prefix_digest: ContentHash) -> Result<CopyCheckpoint, BlobError> {
        self.store.backend.sync(self.blob)?;
        let checkpoint = CopyCheckpoint {
            blob: self.blob,
            length: self.offset,
            prefix_digest,
            fingerprint: self.fingerprint,
        };
        self.store.backend.publish_checkpoint(checkpoint)?;
        Ok(checkpoint)
    }

    /// Makes the blob complete and immutable at exactly its current length, and
    /// returns the store's own word for it.
    ///
    /// Sync before publish, for the same reason a checkpoint does: a seal is the
    /// strongest promise this store makes, and making it about bytes that are
    /// not durable would be the one lie nothing downstream could detect.
    pub fn seal(mut self, digest: ContentHash) -> Result<SealedArtifact, BlobError> {
        self.store.backend.sync(self.blob)?;
        let fact = SealFact {
            blob: self.blob,
            length: self.offset,
            digest,
            fingerprint: self.fingerprint,
        };
        self.store.backend.publish_seal(fact)?;
        self.released = true;
        self.store.backend.release(self.blob);
        Ok(SealedArtifact::new(fact))
    }
}

impl<B: BlobBackend> Drop for BlobLease<'_, B> {
    fn drop(&mut self) {
        if !self.released {
            self.store.backend.release(self.blob);
        }
    }
}
