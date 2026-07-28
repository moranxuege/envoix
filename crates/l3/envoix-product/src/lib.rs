//! Durable product authority.

#![forbid(unsafe_code)]

mod commit;
mod identity;
mod model;
mod pairing;
mod reducer;
mod source;
mod source_lifecycle;

pub mod record;

pub use commit::{
    ApplyOutcome, CommandApplied, CommitError, CommitFailure, CommitStatus, CommittedSession,
    NoRecordStore, RecordStore,
};
// The invite grammar's own published maxima, re-exported beside the channel
// that carries them: an observer sizes its contract from the layer that owns
// invites (`XI02`) instead of restating a number about somebody else's data.
pub use envoix_invite::{
    MAX_BROKER_LENGTH, MAX_INVITE_INPUT_LENGTH, MAX_INVITE_LINK_LENGTH, MAX_RELAY_LENGTH,
    MAX_ROOM_CODE_LENGTH, QrMatrix,
};
pub use identity::{IdentityError, IdentitySource, ProductIdentity, SystemIdentitySource};
pub use model::{
    AppliedCommand, CapabilityAction, CommandLedger, Facts, LedgerHit, NewTransfer, PauseOrigin,
    ProductCommand, ProductEffect, ProductInput, ProductState, Quiescence, SourceDecision,
    StorageAction, TransferRecord, WorkerKind,
};
pub use pairing::PairingChannel;
pub use record::{
    OLDEST_READABLE_RECORD_VERSION, PRODUCT_RECORD_VERSION, RecordCodecError, RecordDecode,
    decode_record, encode_record,
};
pub use source::resolve_source;
pub use source_lifecycle::{
    AcceptedSourceOffer, SelectionGate, SourceLifecycle, SourcePromptReason, SourceRetention,
    StagedContent,
};

#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod quiescence_tests;
#[cfg(test)]
mod tests;
