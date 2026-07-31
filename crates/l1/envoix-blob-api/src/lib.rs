//! Storage for bulk bytes: the artifacts a derivation produces.
//!
//! A PEER of the operation store, not a corner of it. The operation store keeps
//! small facts that must become visible together, and it keeps them by writing a
//! new revision and copying the previous one forward — every artifact, by value,
//! once per commit (`envoix-storage-local`, `copy_prior_revision`). That model is
//! right for a record and catastrophic for a multi-gigabyte artifact: with
//! progress committing per signal, one send would rewrite the bytes thousands of
//! times. The 1 MiB envelope cap is the other reason and the lesser one.
//!
//! So bulk bytes live here, and what the operation store holds about an artifact
//! is a FACT naming it — id, length, digest, seal — never its bytes.
//!
//! # The one rule
//!
//! **An artifact with no seal is not a source.** Everything else follows: a
//! partial cannot be opened, a crash leaves either the last durable checkpoint
//! or a complete seal, and completion is never inferred from a file's length.
//!
//! # Ordering across the two stores
//!
//! There is no shared transaction, and pretending otherwise is how a record ends
//! up naming bytes that never became durable. The order is monotone instead:
//!
//! ```text
//! durable Staging work -> durable bytes + checkpoints -> durable seal -> durable Ready reference
//! ```
//!
//! A crash between the seal and the reference leaves a sealed ORPHAN. That is
//! the deliberate direction: a card still staging that exact work adopts the
//! seal instead of re-deriving it, and anything unreferenced is swept. The
//! opposite order leaves `Ready` pointing at bytes that do not exist, which
//! nothing can recover.

#![forbid(unsafe_code)]

mod key;
mod seal;
mod store;

pub use key::{BlobKey, BlobWorkId};
pub use seal::{CopyCheckpoint, SealFact, SealedArtifact};
pub use store::{BlobBackend, BlobError, BlobLease, BlobState, BlobStore};
