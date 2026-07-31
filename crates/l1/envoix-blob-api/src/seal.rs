//! What a finished blob promises, and what a partial one remembers.

use crate::BlobKey;
use envoix_types::{ByteCount, ContentHash};

/// A durable prefix, and everything needed to decide whether it is still usable.
///
/// More than an offset, deliberately. A checkpoint's offset is only meaningful
/// for the work that produced it: resuming a copy from a prefix written by a
/// different selection, or by a different version of the derivation, would
/// splice two documents together. So a checkpoint names what produced it and is
/// refused when that no longer matches, however useful the offset looks.
///
/// `prefix_digest` covers exactly `length` bytes, so a resumed run can rebuild
/// its hasher by reading the prefix back rather than persisting an opaque
/// hasher state that a future library version might not understand.
/// Not `Serialize`: how a checkpoint is written down is the BACKEND's, and a
/// port that dictated the on-disk shape would make every backend store it the
/// same way whether or not that suited the medium.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyCheckpoint {
    pub blob: BlobKey,
    /// How many bytes of output are durable. Never how many were written — an
    /// uncheckpointed tail is discarded on restart precisely because nothing
    /// promised it.
    pub length: ByteCount,
    pub prefix_digest: ContentHash,
    /// What produced this prefix: the derivation and the selection, folded. A
    /// checkpoint whose fingerprint does not match the current commissioning is
    /// ineligible.
    pub fingerprint: ContentHash,
}

/// A finished blob, as the store durably records it.
///
/// Plain data: this is what is read back after a restart, so it must survive
/// being written down. It is NOT proof — anyone can construct one — which is why
/// the thing the authority accepts is [`SealedArtifact`], and why revalidating a
/// stored `SealFact` means asking the store rather than trusting the bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SealFact {
    pub blob: BlobKey,
    pub length: ByteCount,
    pub digest: ContentHash,
    pub fingerprint: ContentHash,
}

/// The store's own word that a blob was sealed, durably, just now.
///
/// No public constructor and no `Deserialize`: the only way to hold one is to
/// have called [`BlobWriter::seal`] and had it succeed. That is the difference
/// between this and [`SealFact`] — a fact can be written down and read back by
/// anyone, and a witness can only be earned.
///
/// It exists because an `ArtifactId` proves nothing. `ArtifactId::from_bytes` is
/// public, so a worker can name an artifact it never wrote; the reducer needs
/// something a worker without a store cannot produce.
///
/// [`BlobWriter::seal`]: crate::BlobWriter::seal
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SealedArtifact(SealFact);

impl SealedArtifact {
    /// Minted by the store, and only by the store: this is `pub(crate)`, so no
    /// caller outside this crate can build one however much it knows.
    pub(crate) const fn new(fact: SealFact) -> Self {
        Self(fact)
    }

    pub const fn fact(&self) -> SealFact {
        self.0
    }

    pub const fn blob(&self) -> BlobKey {
        self.0.blob
    }

    pub const fn length(&self) -> ByteCount {
        self.0.length
    }

    pub const fn digest(&self) -> ContentHash {
        self.0.digest
    }
}
