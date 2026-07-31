//! Sans-io one-file transfer, verified resume, and durable completion.

#![forbid(unsafe_code)]

mod error;
mod machine;
mod port;

pub use error::{MachineFailure, ProtocolViolation, StorageFault, StorageOperation, TransferError};
pub use machine::{
    CHECKPOINT_INTERVAL, ClaimedComplete, Deadline, MonotonicMillis, ReceiverAwaitHeader,
    ReceiverAwaitHello, ReceiverCompleted, ReceiverProgress, ReceiverReadyToCommit,
    ReceiverReceiving, ReceiverStep, SenderAwaitAck, SenderAwaitReady, SenderAwaitResume,
    SenderCompleted, SenderProgress, SenderRequest, SenderSending, SenderStep, next_chunk_index,
    receiver_start, sender_start,
};
pub use port::{DurablePrefix, SourceReader, StagingSink};

#[cfg(test)]
mod tests;
