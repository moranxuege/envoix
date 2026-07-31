//! Writing the bulk bytes of one incarnation this app owns.
//!
//! The mirror of [`SourceSession`], and here for the same reason that one is in
//! `envoix-capabilities`: the runtime has to NAME the capability in order to
//! carry it from the resolver that opened it to the executor that writes through
//! it, and the runtime may not depend on L2. `envoix-transfer`'s `StagingSink`
//! is the same six operations, but it is L2 and it is generic in its seal, so it
//! cannot be the thing L4 holds.
//!
//! Unlike the source port, this one is NOT vocabulary-neutral about its seal. It
//! returns [`SealedArtifact`] concretely, because the whole reason a receive is
//! worth carrying to the card is that it ends in a witness only this store can
//! mint. A port that returned an opaque token would put the card back to
//! trusting a worker's word for what it produced.
//!
//! [`SourceSession`]: https://docs.rs/envoix-capabilities

use envoix_types::{ByteCount, ContentHash, DurablePrefix};

use crate::{BlobError, SealedArtifact};

/// One open destination for bulk bytes, appendable at an offset.
///
/// `Send` and not `Sync`, like a source session: it is moved into the one
/// attempt that writes through it, and writing needs `&mut self`. Two attempts
/// holding one session would be two attempts writing one artifact, which is the
/// exclusion the underlying lease exists to hold.
///
/// Sealing takes `self: Box<Self>` rather than `&mut self` so that producing the
/// witness CONSUMES the session. A sealed artifact is immutable, so a session
/// that survived its own seal would be a writer for something that can no longer
/// be written — reachable only by writing an unreachable error path at every
/// method.
pub trait SinkSession: Send {
    /// The durable prefix this session opens at, discarding anything past it.
    ///
    /// Called once, before any append. A torn write from a previous run can
    /// leave bytes on disk that were never promised, and no reader may see them.
    fn resume(&mut self) -> Result<DurablePrefix, BlobError>;

    /// Reads back what this session has written, up to its own offset. Reaching
    /// a partial is legitimate HERE and nowhere else: holding the session is
    /// what proves the caller is the party that wrote the bytes.
    fn read_partial_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, BlobError>;

    /// Appends at exactly `offset`. A mismatch is a lost or doubled write.
    fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), BlobError>;

    /// Makes `prefix` durable. The caller passes both the length and the digest
    /// because they are one fact and it is the CALLER's fact — the bytes it has
    /// accepted. A session that inferred the length from its own file would
    /// publish a length and a digest describing different ranges the first time
    /// an append tore.
    fn checkpoint(&mut self, prefix: DurablePrefix) -> Result<(), BlobError>;

    /// Discards everything and starts this incarnation over, as one transition.
    fn reset(&mut self) -> Result<(), BlobError>;

    /// Makes the bytes complete and immutable, and returns the store's own word
    /// for it.
    fn seal(self: Box<Self>, digest: ContentHash) -> Result<SealedArtifact, BlobError>;
}
