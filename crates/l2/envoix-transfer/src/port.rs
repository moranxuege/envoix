use envoix_protocol::ContentHash;
use envoix_types::{ByteCount, TransferId};

use crate::StorageFault;

pub trait SourceReader {
    /// Positional reads may be short; returning zero means end of source.
    fn read_at(&mut self, offset: ByteCount, destination: &mut [u8])
    -> Result<usize, StorageFault>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumeFact {
    /// Durable staged prefix length. Hash checkpoints are deliberately absent.
    pub bytes_staged: ByteCount,
    pub next_chunk_index: u64,
}

pub trait StagingSink {
    /// Loads only a fact whose staged prefix was made durable first.
    fn load_resume(&mut self, transfer_id: TransferId) -> Result<Option<ResumeFact>, StorageFault>;

    /// Reads staged bytes back so the engine can recompute the prefix hash.
    fn read_staged(
        &mut self,
        transfer_id: TransferId,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault>;

    /// Appends at the exact engine-owned offset. Durability is established by checkpoint.
    fn append(
        &mut self,
        transfer_id: TransferId,
        offset: ByteCount,
        bytes: &[u8],
    ) -> Result<(), StorageFault>;

    /// Removes uncheckpointed tail bytes or resets staging to zero.
    fn truncate(&mut self, transfer_id: TransferId, length: ByteCount) -> Result<(), StorageFault>;

    /// Makes the stated prefix durable before publishing this resume fact.
    fn checkpoint(&mut self, transfer_id: TransferId, fact: ResumeFact)
    -> Result<(), StorageFault>;

    /// Makes the verified staged bytes durable. Success is the completion fact.
    fn seal(
        &mut self,
        transfer_id: TransferId,
        file_size: ByteCount,
        file_hash: ContentHash,
    ) -> Result<(), StorageFault>;
}
