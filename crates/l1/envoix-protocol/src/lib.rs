//! Versioned, synchronous data-plane codec and service-protocol identifiers.

#![forbid(unsafe_code)]

mod codec;
mod frame;

pub mod identifiers;
pub mod mailbox;

pub use codec::{
    DecodeError, EncodeError, Field, HEADER_LEN, MAX_CHUNK_SIZE, MAX_FRAME_SIZE,
    MAX_OFFERED_NAME_SIZE, decode_frame, encode_frame, encoded_frame_len,
};
pub use frame::{
    Abort, Chunk, Complete, CompleteAck, ContentHash, FileHeader, Frame, FrameKind, Hello,
    IngressState, ProtocolReason, Ready, ResumeMode, ResumeStatus,
};
