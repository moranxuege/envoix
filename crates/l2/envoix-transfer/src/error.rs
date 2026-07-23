use std::fmt;

use envoix_outcomes::OutcomeCode;
use envoix_protocol::{Abort, Frame, FrameKind, IngressState, ProtocolReason};
use envoix_types::TransferId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageOperation {
    ReadSource,
    LoadResume,
    ReadStaging,
    AppendStaging,
    TruncateStaging,
    Checkpoint,
    Seal,
}

impl fmt::Display for StorageOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSource => formatter.write_str("read source"),
            Self::LoadResume => formatter.write_str("load resume fact"),
            Self::ReadStaging => formatter.write_str("read staging"),
            Self::AppendStaging => formatter.write_str("append staging"),
            Self::TruncateStaging => formatter.write_str("truncate staging"),
            Self::Checkpoint => formatter.write_str("checkpoint staging"),
            Self::Seal => formatter.write_str("seal staging"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageFault {
    operation: StorageOperation,
}

impl StorageFault {
    pub const fn new(operation: StorageOperation) -> Self {
        Self { operation }
    }

    pub const fn operation(self) -> StorageOperation {
        self.operation
    }
}

impl fmt::Display for StorageFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "storage operation failed: {}", self.operation)
    }
}

impl std::error::Error for StorageFault {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolViolation {
    UnexpectedFrame {
        state: IngressState,
        actual: FrameKind,
    },
    TransferIdMismatch,
    ChunkSizeMismatch {
        sender: u64,
        receiver: u64,
    },
    InvalidChunkSize {
        actual: u64,
        maximum: usize,
    },
    ResumeOffsetExceedsFile {
        offset: u64,
        file_size: u64,
    },
    ResumeNotAllowed {
        offset: u64,
    },
    ResumeIndexInconsistent {
        actual: u64,
        expected: u64,
    },
    MissingPrefixHash,
    ChunkIndex {
        actual: u64,
        expected: u64,
    },
    ChunkOffset {
        actual: u64,
        expected: u64,
    },
    ChunkLength {
        actual: usize,
        expected: usize,
    },
    CompleteBeforeEnd {
        received: u64,
        expected: u64,
    },
}

impl fmt::Display for ProtocolViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedFrame { state, actual } => {
                write!(formatter, "frame {actual:?} is invalid in state {state:?}")
            }
            Self::TransferIdMismatch => formatter.write_str("transfer identity does not match"),
            Self::ChunkSizeMismatch { sender, receiver } => write!(
                formatter,
                "sender chunk size {sender} does not match receiver chunk size {receiver}"
            ),
            Self::InvalidChunkSize { actual, maximum } => write!(
                formatter,
                "chunk size {actual} must be between 1 and {maximum}"
            ),
            Self::ResumeOffsetExceedsFile { offset, file_size } => {
                write!(
                    formatter,
                    "resume offset {offset} exceeds file size {file_size}"
                )
            }
            Self::ResumeNotAllowed { offset } => {
                write!(
                    formatter,
                    "resume offset {offset} was offered for a fresh transfer"
                )
            }
            Self::ResumeIndexInconsistent { actual, expected } => write!(
                formatter,
                "resume chunk index {actual} does not match expected {expected}"
            ),
            Self::MissingPrefixHash => {
                formatter.write_str("non-empty resume status has no prefix hash")
            }
            Self::ChunkIndex { actual, expected } => {
                write!(
                    formatter,
                    "chunk index {actual} does not match expected {expected}"
                )
            }
            Self::ChunkOffset { actual, expected } => {
                write!(
                    formatter,
                    "chunk offset {actual} does not match expected {expected}"
                )
            }
            Self::ChunkLength { actual, expected } => {
                write!(
                    formatter,
                    "chunk length {actual} does not match expected {expected}"
                )
            }
            Self::CompleteBeforeEnd { received, expected } => write!(
                formatter,
                "completion arrived after {received} of {expected} bytes"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferError {
    Protocol(ProtocolViolation),
    IntegrityMismatch,
    Storage(StorageFault),
    UnexpectedSourceEnd { offset: u64, expected: u64 },
    Timeout,
    Cancelled,
    Paused,
    PeerClosed,
    PeerAborted(ProtocolReason),
}

impl TransferError {
    pub const fn outcome_code(self) -> OutcomeCode {
        match self {
            Self::Storage(fault) if matches!(fault.operation(), StorageOperation::ReadSource) => {
                OutcomeCode::SourceUnreadable
            }
            Self::Storage(_) => OutcomeCode::StorageFault,
            Self::UnexpectedSourceEnd { .. } => OutcomeCode::SourceUnreadable,
            Self::Timeout => OutcomeCode::Timeout,
            Self::Cancelled => OutcomeCode::Cancelled,
            Self::Paused => OutcomeCode::Paused,
            Self::PeerClosed => OutcomeCode::PeerLost,
            Self::PeerAborted(reason) => match reason.outcome_code() {
                Some(code) => code,
                None => OutcomeCode::Internal,
            },
            Self::Protocol(_) | Self::IntegrityMismatch => OutcomeCode::Internal,
        }
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(violation) => violation.fmt(formatter),
            Self::IntegrityMismatch => formatter.write_str("transfer integrity mismatch"),
            Self::Storage(fault) => fault.fmt(formatter),
            Self::UnexpectedSourceEnd { offset, expected } => write!(
                formatter,
                "source ended at byte {offset}, expected {expected}"
            ),
            Self::Timeout => formatter.write_str("transfer deadline exceeded"),
            Self::Cancelled => formatter.write_str("transfer cancelled"),
            Self::Paused => formatter.write_str("transfer paused"),
            Self::PeerClosed => formatter.write_str("transfer peer closed"),
            Self::PeerAborted(reason) => write!(formatter, "peer aborted transfer: {reason:?}"),
        }
    }
}

impl std::error::Error for TransferError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MachineFailure {
    error: TransferError,
    abort: Option<Abort>,
}

impl MachineFailure {
    pub(crate) const fn terminal(error: TransferError) -> Self {
        Self { error, abort: None }
    }

    pub(crate) const fn notifying(error: TransferError, abort: Abort) -> Self {
        Self {
            error,
            abort: Some(abort),
        }
    }

    pub(crate) const fn for_local(
        error: TransferError,
        transfer_id: Option<TransferId>,
        reason: ProtocolReason,
    ) -> Self {
        Self::notifying(
            error,
            Abort {
                transfer_id,
                reason,
            },
        )
    }

    pub(crate) const fn from_engine_error(error: TransferError, transfer_id: TransferId) -> Self {
        let reason = match error {
            TransferError::IntegrityMismatch => ProtocolReason::IntegrityMismatch,
            TransferError::Storage(_) | TransferError::UnexpectedSourceEnd { .. } => {
                ProtocolReason::StorageFault
            }
            TransferError::Protocol(_) => ProtocolReason::ProtocolViolation,
            TransferError::Cancelled => ProtocolReason::Cancelled,
            TransferError::Paused => ProtocolReason::Paused,
            TransferError::Timeout | TransferError::PeerClosed | TransferError::PeerAborted(_) => {
                return Self::terminal(error);
            }
        };
        Self::for_local(error, Some(transfer_id), reason)
    }

    pub const fn error(self) -> TransferError {
        self.error
    }

    pub const fn abort(self) -> Option<Abort> {
        self.abort
    }

    pub fn outbound(self) -> Option<Frame> {
        self.abort.map(Frame::Abort)
    }
}
