use envoix_outcomes::OutcomeCode;
pub use envoix_types::ContentHash;
use envoix_types::{ByteCount, OfferedName, TransferId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hello;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ready;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeMode {
    Disabled,
    Allowed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileHeader {
    pub transfer_id: TransferId,
    pub offered_name: OfferedName,
    pub file_size: ByteCount,
    pub chunk_size: ByteCount,
    pub resume: ResumeMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeStatus {
    pub transfer_id: TransferId,
    pub next_chunk_index: u64,
    pub bytes_received: ByteCount,
    pub prefix_hash: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub transfer_id: TransferId,
    pub index: u64,
    pub offset: ByteCount,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Complete {
    pub transfer_id: TransferId,
    pub file_hash: ContentHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompleteAck {
    pub transfer_id: TransferId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProtocolReason {
    Cancelled,
    Paused,
    Unauthenticated,
    VersionMismatch,
    ProtocolViolation,
    IntegrityMismatch,
    StorageFault,
    Internal,
}

impl ProtocolReason {
    /// Returns the product outcome when the wire reason has an exact L0 equivalent.
    pub const fn outcome_code(self) -> Option<OutcomeCode> {
        match self {
            Self::Cancelled => Some(OutcomeCode::Cancelled),
            Self::Paused => Some(OutcomeCode::Paused),
            Self::Unauthenticated => Some(OutcomeCode::Unauthenticated),
            Self::VersionMismatch => Some(OutcomeCode::VersionMismatch),
            Self::StorageFault => Some(OutcomeCode::StorageFault),
            Self::Internal => Some(OutcomeCode::Internal),
            Self::ProtocolViolation | Self::IntegrityMismatch => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Abort {
    pub transfer_id: Option<TransferId>,
    pub reason: ProtocolReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    Hello(Hello),
    Ready(Ready),
    FileHeader(FileHeader),
    ResumeStatus(ResumeStatus),
    Chunk(Chunk),
    Complete(Complete),
    CompleteAck(CompleteAck),
    Abort(Abort),
}

impl Frame {
    pub const fn kind(&self) -> FrameKind {
        match self {
            Self::Hello(_) => FrameKind::Hello,
            Self::Ready(_) => FrameKind::Ready,
            Self::FileHeader(_) => FrameKind::FileHeader,
            Self::ResumeStatus(_) => FrameKind::ResumeStatus,
            Self::Chunk(_) => FrameKind::Chunk,
            Self::Complete(_) => FrameKind::Complete,
            Self::CompleteAck(_) => FrameKind::CompleteAck,
            Self::Abort(_) => FrameKind::Abort,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    // Wire ID 1 remains reserved for the C3 authentication handshake.
    Hello = 2,
    Ready = 3,
    FileHeader = 4,
    ResumeStatus = 5,
    Chunk = 6,
    Complete = 7,
    CompleteAck = 8,
    Abort = 9,
}

impl FrameKind {
    pub const fn wire_id(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_wire_id(value: u8) -> Option<Self> {
        match value {
            2 => Some(Self::Hello),
            3 => Some(Self::Ready),
            4 => Some(Self::FileHeader),
            5 => Some(Self::ResumeStatus),
            6 => Some(Self::Chunk),
            7 => Some(Self::Complete),
            8 => Some(Self::CompleteAck),
            9 => Some(Self::Abort),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IngressState {
    AwaitHello,
    AwaitReady,
    AwaitFileHeader,
    AwaitResumeStatus,
    ReceivingData,
    AwaitCompleteAck,
}

impl IngressState {
    pub const fn accepts(self, kind: FrameKind) -> bool {
        matches!(kind, FrameKind::Abort)
            || matches!(
                (self, kind),
                (Self::AwaitHello, FrameKind::Hello)
                    | (Self::AwaitReady, FrameKind::Ready)
                    | (Self::AwaitFileHeader, FrameKind::FileHeader)
                    | (Self::AwaitResumeStatus, FrameKind::ResumeStatus)
                    | (Self::ReceivingData, FrameKind::Chunk | FrameKind::Complete)
                    | (Self::AwaitCompleteAck, FrameKind::CompleteAck)
            )
    }
}
