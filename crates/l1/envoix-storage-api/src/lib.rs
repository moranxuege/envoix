//! Storage contracts with explicit durability, quarantine, and writer ownership.

#![forbid(unsafe_code)]

mod contract;
mod envelope;
mod manifest;
mod memory;

pub mod identifiers;

pub use contract::{
    CommitReceipt, Durability, DurabilityContractError, EnvelopeKey, LeaseAcquisition, LoadOutcome,
    QuarantineReason, QuarantinedEnvelope, Storage, StorageTransaction, WriterLease,
};
pub use envelope::{
    CURRENT_ENVELOPE_VERSION, EnvelopeDecode, EnvelopeError, MAX_ENVELOPE_BODY_BYTES, OpaqueBody,
    OperationEnvelope,
};
pub use manifest::{ArtifactManifestEntry, CardManifest};
pub use memory::{InMemoryStorage, InMemoryTransaction, InMemoryWriterLease, MemoryStorageError};

#[cfg(test)]
mod tests;
