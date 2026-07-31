use envoix_types::{ByteCount, ContentHash, DurablePrefix};

use crate::StorageFault;

pub trait SourceReader {
    /// Positional reads may be short; returning zero means end of source.
    fn read_at(&mut self, offset: ByteCount, destination: &mut [u8])
    -> Result<usize, StorageFault>;
}

/// Where one receive puts its bytes.
///
/// Bound to ONE artifact before any peer frame arrives, which is why no method
/// names a transfer. The old shape took a `TransferId` everywhere, so a peer
/// frame could in principle choose which local file an operation opened; an
/// attempt owns one sink, and the sink knows which.
pub trait StagingSink {
    /// What sealing produces. Production returns the blob store's non-forgeable
    /// witness; a test double may return its own token.
    ///
    /// Associated rather than `()` so the machine stays storage-neutral WITHOUT
    /// discarding the witness at the one boundary that earns it — throwing it
    /// away would put the card back to trusting a worker-authored id.
    type Seal;

    /// The durable prefix this sink resumes at. Zero length when there is none.
    fn resume(&mut self) -> Result<DurablePrefix, StorageFault>;

    /// Reads back what this sink has staged, so the engine can recompute the
    /// prefix hash rather than persist an opaque hasher state.
    fn read_partial_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault>;

    /// Appends at the exact engine-owned offset. Durability is established by
    /// checkpoint.
    fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), StorageFault>;

    /// Promises that `prefix` is durable, and makes it so. Every byte is durable
    /// BEFORE the prefix that names them becomes readable.
    ///
    /// The engine passes BOTH numbers because they are one fact and it is the
    /// engine's fact: `length` is what the engine has ACCEPTED, and the digest
    /// covers exactly those bytes. A sink cannot infer the length from its own
    /// file — a torn append leaves bytes on disk the engine never accepted, and
    /// inferring would publish a length and a digest describing different
    /// ranges. A sink may hold MORE bytes than it promises; `resume` discards
    /// that tail.
    fn checkpoint(&mut self, prefix: DurablePrefix) -> Result<(), StorageFault>;

    /// Discards everything and starts over.
    ///
    /// ONE operation, because the two it replaces have an order — publish the
    /// zero prefix, then truncate — and a caller doing them itself has no way to
    /// notice it got them backwards. Backwards leaves a promise about bytes that
    /// are no longer there.
    fn reset(&mut self) -> Result<(), StorageFault>;

    /// Makes the verified staged bytes durable and immutable. Success is the
    /// completion fact, and what it returns is the proof of it.
    fn seal(
        &mut self,
        expected_size: ByteCount,
        digest: ContentHash,
    ) -> Result<Self::Seal, StorageFault>;
}
