//! Durable product authority.

#![forbid(unsafe_code)]

mod commit;
mod identity;
mod model;
mod reducer;
mod source;

pub mod record;

pub use commit::{
    ApplyOutcome, CommandApplied, CommitError, CommitFailure, CommitStatus, CommittedSession,
    NoRecordStore, RecordStore,
};
pub use identity::{IdentityError, IdentitySource, ProductIdentity, SystemIdentitySource};
pub use model::{
    AppliedCommand, CapabilityAction, CommandLedger, Facts, LedgerHit, NewTransfer, PauseOrigin,
    ProductCommand, ProductEffect, ProductInput, ProductState, Quiescence, SourceDecision,
    StorageAction, TransferRecord, WorkerKind,
};
pub use record::{
    OLDEST_READABLE_RECORD_VERSION, PRODUCT_RECORD_VERSION, RecordCodecError, RecordDecode,
    decode_record, encode_record,
};
pub use source::resolve_source;

#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod quiescence_tests;
#[cfg(test)]
mod tests;
