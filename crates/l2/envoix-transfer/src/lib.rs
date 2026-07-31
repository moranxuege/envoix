//! Sans-io one-file transfer, verified resume, and durable completion.

#![forbid(unsafe_code)]

mod error;
mod machine;
mod port;

/// Re-exported so a sink implementation needs only this crate in scope. It is
/// defined in L0 because the bulk store speaks it too.
pub use envoix_types::{DurablePrefix, PeerContentDeclaration};
pub use error::{MachineFailure, ProtocolViolation, StorageFault, StorageOperation, TransferError};
pub use machine::{
    CHECKPOINT_INTERVAL, ClaimedComplete, Deadline, MonotonicMillis, ReceiveCommit,
    ReceiverAwaitHeader, ReceiverAwaitHello, ReceiverCompleted, ReceiverHeaderAdmitted,
    ReceiverProgress, ReceiverReadyToCommit, ReceiverReceiving, ReceiverStep, SenderAwaitAck,
    SenderAwaitReady, SenderAwaitResume, SenderCompleted, SenderProgress, SenderRequest,
    SenderSending, SenderStep, next_chunk_index, receiver_start, sender_start,
};
pub use port::{SourceReader, StagingSink};

#[cfg(test)]
mod tests;
