//! A filesystem backend for bulk artifacts.
//!
//! Layout, per card:
//!
//! ```text
//! blobs/<card>/<work>/bytes        the artifact itself, appended to
//! blobs/<card>/<work>/checkpoint   the last durable prefix, if any
//! blobs/<card>/<work>/seal         present only when the blob is complete
//! ```
//!
//! Flat and by INCARNATION, not versioned: bytes are never copied forward, which
//! is the whole reason this is not the operation store. A record commit rewrites
//! nothing here.
//!
//! The seal is a separate file so that "complete" is a fact with its own atomic
//! publication rather than a property of `bytes`. Nothing here ever concludes a
//! blob is finished from the length of `bytes` — that is exactly what a
//! half-written one also has.
//!
//! Every published fact is written to a temporary file, synced, renamed into
//! place, and the directory synced after. Rename is the atomic step; the
//! directory sync is what makes the rename itself survive.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use envoix_blob_api::{
    BlobBackend, BlobError, BlobKey, BlobState, BlobWorkId, CopyCheckpoint, SealFact,
};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, ContentHash, RecordId, TransferId};
use serde::{Deserialize, Serialize};

const BLOBS_DIR: &str = "blobs";
const BYTES_FILE: &str = "bytes";
const CHECKPOINT_FILE: &str = "checkpoint";
const SEAL_FILE: &str = "seal";

/// How a checkpoint or a seal is written down. The port deliberately does not
/// say, so this is the local backend's own shape and no other backend inherits
/// it.
#[derive(Deserialize, Serialize)]
struct FactDto {
    length: u64,
    digest: [u8; 32],
    fingerprint: [u8; 32],
}

#[derive(Clone, Default)]
pub struct LocalBlobs {
    root: PathBuf,
    /// Which blobs are being written, in THIS process.
    ///
    /// Deliberately not a lock file. A lease exists to stop two workers in one
    /// process from interleaving appends; a crashed process leaves no writer to
    /// exclude, and the next `begin` truncates to the last durable checkpoint
    /// anyway — so a durable lock would only ever lock out the recovery it is
    /// supposed to protect.
    writing: Arc<Mutex<HashSet<BlobKey>>>,
}

impl LocalBlobs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writing: Arc::default(),
        }
    }

    /// One directory per incarnation, named by which KIND of work made it.
    ///
    /// The arm is in the name because the two are stable under different facts —
    /// a derivation under its acquisition generation, a reception under its
    /// transfer — and a single flat encoding would let one be read as the other
    /// after a restart.
    fn dir(&self, blob: BlobKey) -> PathBuf {
        self.root
            .join(BLOBS_DIR)
            .join(format!("{:016x}", blob.card().get()))
            .join(Self::leaf(blob.work()))
    }

    fn leaf(work: BlobWorkId) -> String {
        match work {
            BlobWorkId::Derivation {
                acquisition,
                artifact,
            } => format!("d{:08x}-{artifact}", acquisition.get()),
            BlobWorkId::Reception { transfer, artifact } => format!("r{transfer}-{artifact}"),
        }
    }

    fn work_from_leaf(leaf: &str) -> Option<BlobWorkId> {
        let (kind, rest) = leaf.split_at(leaf.char_indices().nth(1)?.0);
        let (left, artifact) = rest.split_once('-')?;
        let artifact =
            ArtifactId::from_bytes(u128::from_str_radix(artifact, 16).ok()?.to_be_bytes());
        Some(match kind {
            "d" => BlobWorkId::of_derivation(
                AttemptGen::new(u32::from_str_radix(left, 16).ok()?),
                artifact,
            ),
            "r" => BlobWorkId::of_reception(
                TransferId::from_bytes(u128::from_str_radix(left, 16).ok()?.to_be_bytes()),
                artifact,
            ),
            _ => return None,
        })
    }

    fn read_fact(path: &Path, blob: BlobKey) -> Option<(ByteCount, ContentHash, ContentHash)> {
        let bytes = fs::read(path).ok()?;
        let dto: FactDto = serde_json::from_slice(&bytes).ok()?;
        let _ = blob;
        Some((
            ByteCount::new(dto.length),
            ContentHash::from_bytes(dto.digest),
            ContentHash::from_bytes(dto.fingerprint),
        ))
    }

    /// Writes one fact atomically: temp file, sync, rename, sync the directory.
    /// The rename publishes it; the directory sync is what makes the rename
    /// itself survive a power loss.
    fn publish(path: &Path, dto: &FactDto) -> Result<(), BlobError> {
        let directory = path.parent().ok_or(BlobError::Storage)?;
        let temporary = directory.join(format!(
            "{}.publishing",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("fact")
        ));
        let encoded = serde_json::to_vec(dto).map_err(|_| BlobError::Storage)?;
        {
            let mut file = File::create(&temporary).map_err(map_io)?;
            file.write_all(&encoded).map_err(map_io)?;
            file.sync_all().map_err(map_io)?;
        }
        fs::rename(&temporary, path).map_err(map_io)?;
        File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(map_io)
    }
}

fn map_io(error: std::io::Error) -> BlobError {
    match error.raw_os_error() {
        // ENOSPC and EDQUOT are the one fault a person can act on, and they must
        // never become "the source could not be read".
        Some(28 | 122) => BlobError::OutOfSpace,
        _ => BlobError::Storage,
    }
}

impl BlobBackend for LocalBlobs {
    fn state(&self, blob: BlobKey) -> Result<BlobState, BlobError> {
        let dir = self.dir(blob);
        if let Some((length, digest, fingerprint)) = Self::read_fact(&dir.join(SEAL_FILE), blob) {
            return Ok(BlobState::Sealed(SealFact {
                blob,
                length,
                digest,
                fingerprint,
            }));
        }
        if !dir.join(BYTES_FILE).try_exists().map_err(map_io)? {
            return Ok(BlobState::Absent);
        }
        Ok(BlobState::Partial {
            durable_checkpoint: Self::read_fact(&dir.join(CHECKPOINT_FILE), blob).map(
                |(length, prefix_digest, fingerprint)| CopyCheckpoint {
                    blob,
                    length,
                    prefix_digest,
                    fingerprint,
                },
            ),
        })
    }

    fn acquire(&self, blob: BlobKey) -> Result<(), BlobError> {
        let mut writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        if !writing.insert(blob) {
            return Err(BlobError::AlreadyWriting);
        }
        drop(writing);
        fs::create_dir_all(self.dir(blob)).map_err(map_io)
    }

    fn release(&self, blob: BlobKey) {
        self.writing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&blob);
    }

    fn truncate(&self, blob: BlobKey, length: ByteCount) -> Result<(), BlobError> {
        let bytes = self.dir(blob).join(BYTES_FILE);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&bytes)
            .map_err(map_io)?;
        file.set_len(length.get()).map_err(map_io)?;
        file.sync_all().map_err(map_io)
    }

    fn append_at(&self, blob: BlobKey, offset: ByteCount, bytes: &[u8]) -> Result<(), BlobError> {
        let file = OpenOptions::new()
            .write(true)
            .open(self.dir(blob).join(BYTES_FILE))
            .map_err(map_io)?;
        file.write_all_at(bytes, offset.get()).map_err(map_io)
    }

    fn sync(&self, blob: BlobKey) -> Result<(), BlobError> {
        File::open(self.dir(blob).join(BYTES_FILE))
            .and_then(|file| file.sync_all())
            .map_err(map_io)
    }

    fn publish_checkpoint(&self, checkpoint: CopyCheckpoint) -> Result<(), BlobError> {
        Self::publish(
            &self.dir(checkpoint.blob).join(CHECKPOINT_FILE),
            &FactDto {
                length: checkpoint.length.get(),
                digest: checkpoint.prefix_digest.to_bytes(),
                fingerprint: checkpoint.fingerprint.to_bytes(),
            },
        )
    }

    fn publish_seal(&self, fact: SealFact) -> Result<(), BlobError> {
        Self::publish(
            &self.dir(fact.blob).join(SEAL_FILE),
            &FactDto {
                length: fact.length.get(),
                digest: fact.digest.to_bytes(),
                fingerprint: fact.fingerprint.to_bytes(),
            },
        )
    }

    fn remove(&self, blob: BlobKey) -> Result<(), BlobError> {
        match fs::remove_dir_all(self.dir(blob)) {
            Ok(()) => Ok(()),
            // Already absent is the outcome the caller asked for.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(map_io(error)),
        }
    }

    fn owned(&self, card: RecordId) -> Result<Vec<BlobKey>, BlobError> {
        let dir = self
            .root
            .join(BLOBS_DIR)
            .join(format!("{:016x}", card.get()));
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(map_io(error)),
        };
        let mut owned = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(work) = name.to_str().and_then(Self::work_from_leaf) else {
                continue;
            };
            owned.push(BlobKey::new(card, work));
        }
        owned.sort_unstable();
        Ok(owned)
    }

    fn read_at(
        &self,
        blob: BlobKey,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, BlobError> {
        File::open(self.dir(blob).join(BYTES_FILE))
            .and_then(|file| file.read_at(destination, offset.get()))
            .map_err(map_io)
    }
}

#[cfg(test)]
mod tests;
