use std::fmt;

use envoix_types::{ByteCount, OfferedName, TransferId};

use crate::frame::{
    Abort, Chunk, Complete, CompleteAck, ContentHash, FileHeader, Frame, FrameKind, Hello,
    IngressState, ProtocolReason, Ready, ResumeMode, ResumeStatus,
};
use crate::identifiers::{DATA_MAGIC, DATA_WIRE_VERSION};

pub const HEADER_LEN: usize = 12;
pub const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;
pub const MAX_FRAME_SIZE: usize = MAX_CHUNK_SIZE + 64 * 1024;
/// The wire's bound on an offered name, taken from the layer that owns the
/// type rather than restated: a name this frame cannot carry is one no peer
/// could have written to disk in the first place.
pub const MAX_OFFERED_NAME_SIZE: usize = OfferedName::MAX_BYTES;

const TRANSFER_ID_LEN: usize = 16;
const HASH_LEN: usize = 32;
const FILE_HEADER_FIXED_LEN: usize = TRANSFER_ID_LEN + 4 + 8 + 4 + 1;
const RESUME_STATUS_WITHOUT_HASH_LEN: usize = TRANSFER_ID_LEN + 8 + 8 + 1;
const CHUNK_FIXED_LEN: usize = TRANSFER_ID_LEN + 8 + 8 + 4;

const _: () = assert!(MAX_FRAME_SIZE <= u32::MAX as usize);
const _: () = assert!(MAX_OFFERED_NAME_SIZE <= u32::MAX as usize);
const _: () = assert!(MAX_CHUNK_SIZE <= u32::MAX as usize);
const _: () = assert!(MAX_FRAME_SIZE >= CHUNK_FIXED_LEN + MAX_CHUNK_SIZE);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    OfferedName,
    ChunkBytes,
}

impl fmt::Display for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OfferedName => formatter.write_str("offered name"),
            Self::ChunkBytes => formatter.write_str("chunk bytes"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    FieldTooLarge {
        field: Field,
        actual: usize,
        maximum: usize,
    },
    InvalidChunkSize {
        actual: u64,
        maximum: usize,
    },
    FrameTooLarge {
        actual: usize,
        maximum: usize,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldTooLarge {
                field,
                actual,
                maximum,
            } => write!(
                formatter,
                "{field} length {actual} exceeds maximum {maximum}"
            ),
            Self::InvalidChunkSize { actual, maximum } => write!(
                formatter,
                "chunk size {actual} must be between 1 and {maximum} bytes"
            ),
            Self::FrameTooLarge { actual, maximum } => write!(
                formatter,
                "frame payload length {actual} exceeds maximum {maximum}"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    TruncatedHeader {
        actual: usize,
    },
    WrongMagic {
        actual: [u8; 4],
    },
    UnsupportedVersion {
        actual: u16,
    },
    UnknownFrameType {
        wire_id: u8,
    },
    NonZeroReservedByte {
        actual: u8,
    },
    FrameTooLarge {
        declared: usize,
        maximum: usize,
    },
    WrongState {
        state: IngressState,
        kind: FrameKind,
    },
    TruncatedFrame {
        declared: usize,
        actual: usize,
    },
    TrailingFrameBytes {
        count: usize,
    },
    InvalidPayloadLength {
        kind: FrameKind,
        actual: usize,
    },
    TruncatedPayload {
        needed: usize,
        remaining: usize,
    },
    TrailingPayloadBytes {
        count: usize,
    },
    FieldTooLarge {
        field: Field,
        declared: usize,
        maximum: usize,
    },
    InvalidUtf8 {
        field: Field,
    },
    InvalidOfferedName,
    InvalidChunkSize {
        actual: u64,
        maximum: usize,
    },
    InvalidResumeMode {
        wire_id: u8,
    },
    InvalidOptionTag {
        wire_id: u8,
    },
    UnknownProtocolReason {
        wire_id: u8,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { actual } => {
                write!(formatter, "frame header has {actual} of {HEADER_LEN} bytes")
            }
            Self::WrongMagic { actual } => write!(formatter, "wrong frame magic {actual:?}"),
            Self::UnsupportedVersion { actual } => {
                write!(formatter, "unsupported frame version {actual}")
            }
            Self::UnknownFrameType { wire_id } => {
                write!(formatter, "unknown frame type {wire_id}")
            }
            Self::NonZeroReservedByte { actual } => {
                write!(formatter, "reserved frame byte must be zero, got {actual}")
            }
            Self::FrameTooLarge { declared, maximum } => write!(
                formatter,
                "declared frame payload {declared} exceeds maximum {maximum}"
            ),
            Self::WrongState { state, kind } => {
                write!(formatter, "frame {kind:?} is illegal in state {state:?}")
            }
            Self::TruncatedFrame { declared, actual } => write!(
                formatter,
                "frame declares {declared} payload bytes but only {actual} are present"
            ),
            Self::TrailingFrameBytes { count } => {
                write!(formatter, "frame has {count} trailing bytes")
            }
            Self::InvalidPayloadLength { kind, actual } => {
                write!(
                    formatter,
                    "frame {kind:?} has invalid payload length {actual}"
                )
            }
            Self::TruncatedPayload { needed, remaining } => write!(
                formatter,
                "payload needs {needed} bytes but only {remaining} remain"
            ),
            Self::TrailingPayloadBytes { count } => {
                write!(formatter, "payload has {count} trailing bytes")
            }
            Self::FieldTooLarge {
                field,
                declared,
                maximum,
            } => write!(
                formatter,
                "declared {field} length {declared} exceeds maximum {maximum}"
            ),
            Self::InvalidUtf8 { field } => write!(formatter, "{field} is not valid UTF-8"),
            Self::InvalidOfferedName => formatter.write_str("offered name is not canonical"),
            Self::InvalidChunkSize { actual, maximum } => write!(
                formatter,
                "chunk size {actual} must be between 1 and {maximum} bytes"
            ),
            Self::InvalidResumeMode { wire_id } => {
                write!(formatter, "invalid resume mode {wire_id}")
            }
            Self::InvalidOptionTag { wire_id } => {
                write!(formatter, "invalid option tag {wire_id}")
            }
            Self::UnknownProtocolReason { wire_id } => {
                write!(formatter, "unknown protocol reason {wire_id}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, EncodeError> {
    let kind = frame.kind();
    let mut encoded = Vec::new();
    encoded.extend_from_slice(DATA_MAGIC);
    encoded.extend_from_slice(&DATA_WIRE_VERSION.to_be_bytes());
    encoded.extend_from_slice(&[kind.wire_id(), 0]);
    encoded.extend_from_slice(&[0; 4]);

    match frame {
        Frame::Hello(_) | Frame::Ready(_) => {}
        Frame::FileHeader(header) => encode_file_header(&mut encoded, header)?,
        Frame::ResumeStatus(status) => encode_resume_status(&mut encoded, status),
        Frame::Chunk(chunk) => encode_chunk(&mut encoded, chunk)?,
        Frame::Complete(complete) => {
            write_transfer_id(&mut encoded, complete.transfer_id);
            encoded.extend_from_slice(complete.file_hash.as_bytes());
        }
        Frame::CompleteAck(ack) => write_transfer_id(&mut encoded, ack.transfer_id),
        Frame::Abort(abort) => encode_abort(&mut encoded, abort),
    }

    let payload_len = encoded.len() - HEADER_LEN;
    if payload_len > MAX_FRAME_SIZE {
        return Err(EncodeError::FrameTooLarge {
            actual: payload_len,
            maximum: MAX_FRAME_SIZE,
        });
    }
    encoded[8..12].copy_from_slice(&(payload_len as u32).to_be_bytes());
    Ok(encoded)
}

/// Returns the complete encoded length declared by a common ENVX frame header.
///
/// Authentication and transfer codecs share this envelope. Stream adapters use
/// this helper to bound the payload read without duplicating codec-owned layout.
pub fn encoded_frame_len(header: &[u8; HEADER_LEN]) -> Result<usize, DecodeError> {
    let magic = [header[0], header[1], header[2], header[3]];
    if &magic != DATA_MAGIC {
        return Err(DecodeError::WrongMagic { actual: magic });
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != DATA_WIRE_VERSION {
        return Err(DecodeError::UnsupportedVersion { actual: version });
    }
    if header[7] != 0 {
        return Err(DecodeError::NonZeroReservedByte { actual: header[7] });
    }
    let payload_len = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    if payload_len > MAX_FRAME_SIZE {
        return Err(DecodeError::FrameTooLarge {
            declared: payload_len,
            maximum: MAX_FRAME_SIZE,
        });
    }
    Ok(HEADER_LEN + payload_len)
}

pub fn decode_frame(input: &[u8], state: IngressState) -> Result<Frame, DecodeError> {
    if input.len() < HEADER_LEN {
        return Err(DecodeError::TruncatedHeader {
            actual: input.len(),
        });
    }

    let magic = [input[0], input[1], input[2], input[3]];
    if &magic != DATA_MAGIC {
        return Err(DecodeError::WrongMagic { actual: magic });
    }
    let version = u16::from_be_bytes([input[4], input[5]]);
    if version != DATA_WIRE_VERSION {
        return Err(DecodeError::UnsupportedVersion { actual: version });
    }
    let kind = FrameKind::from_wire_id(input[6])
        .ok_or(DecodeError::UnknownFrameType { wire_id: input[6] })?;
    if input[7] != 0 {
        return Err(DecodeError::NonZeroReservedByte { actual: input[7] });
    }

    let payload_len = u32::from_be_bytes([input[8], input[9], input[10], input[11]]) as usize;
    if payload_len > MAX_FRAME_SIZE {
        return Err(DecodeError::FrameTooLarge {
            declared: payload_len,
            maximum: MAX_FRAME_SIZE,
        });
    }
    if !state.accepts(kind) {
        return Err(DecodeError::WrongState { state, kind });
    }

    let actual_payload_len = input.len() - HEADER_LEN;
    if actual_payload_len < payload_len {
        return Err(DecodeError::TruncatedFrame {
            declared: payload_len,
            actual: actual_payload_len,
        });
    }
    if actual_payload_len > payload_len {
        return Err(DecodeError::TrailingFrameBytes {
            count: actual_payload_len - payload_len,
        });
    }
    validate_payload_length(kind, payload_len)?;

    let mut reader = PayloadReader::new(&input[HEADER_LEN..]);
    let frame = match kind {
        FrameKind::Hello => Frame::Hello(Hello),
        FrameKind::Ready => Frame::Ready(Ready),
        FrameKind::FileHeader => Frame::FileHeader(decode_file_header(&mut reader)?),
        FrameKind::ResumeStatus => Frame::ResumeStatus(decode_resume_status(&mut reader)?),
        FrameKind::Chunk => Frame::Chunk(decode_chunk(&mut reader)?),
        FrameKind::Complete => Frame::Complete(Complete {
            transfer_id: reader.read_transfer_id()?,
            file_hash: reader.read_hash()?,
        }),
        FrameKind::CompleteAck => Frame::CompleteAck(CompleteAck {
            transfer_id: reader.read_transfer_id()?,
        }),
        FrameKind::Abort => Frame::Abort(decode_abort(&mut reader)?),
    };
    reader.finish()?;
    Ok(frame)
}

fn encode_file_header(output: &mut Vec<u8>, header: &FileHeader) -> Result<(), EncodeError> {
    let name = header.offered_name.as_str().as_bytes();
    write_transfer_id(output, header.transfer_id);
    write_bounded_bytes(output, Field::OfferedName, name, MAX_OFFERED_NAME_SIZE)?;
    output.extend_from_slice(&header.file_size.get().to_be_bytes());
    let chunk_size = encode_chunk_size(header.chunk_size)?;
    output.extend_from_slice(&chunk_size.to_be_bytes());
    output.push(match header.resume {
        ResumeMode::Disabled => 0,
        ResumeMode::Allowed => 1,
    });
    Ok(())
}

fn encode_resume_status(output: &mut Vec<u8>, status: &ResumeStatus) {
    write_transfer_id(output, status.transfer_id);
    output.extend_from_slice(&status.next_chunk_index.to_be_bytes());
    output.extend_from_slice(&status.bytes_received.get().to_be_bytes());
    match status.prefix_hash {
        None => output.push(0),
        Some(hash) => {
            output.push(1);
            output.extend_from_slice(hash.as_bytes());
        }
    }
}

fn encode_chunk(output: &mut Vec<u8>, chunk: &Chunk) -> Result<(), EncodeError> {
    write_transfer_id(output, chunk.transfer_id);
    output.extend_from_slice(&chunk.index.to_be_bytes());
    output.extend_from_slice(&chunk.offset.get().to_be_bytes());
    write_bounded_bytes(output, Field::ChunkBytes, &chunk.bytes, MAX_CHUNK_SIZE)
}

fn encode_abort(output: &mut Vec<u8>, abort: &Abort) {
    match abort.transfer_id {
        None => output.push(0),
        Some(transfer_id) => {
            output.push(1);
            write_transfer_id(output, transfer_id);
        }
    }
    output.push(encode_reason(abort.reason));
}

fn decode_file_header(reader: &mut PayloadReader<'_>) -> Result<FileHeader, DecodeError> {
    let transfer_id = reader.read_transfer_id()?;
    let offered_name = reader.read_offered_name()?;
    let file_size = ByteCount::new(reader.read_u64()?);
    let chunk_size = reader.read_u32()? as u64;
    validate_decoded_chunk_size(chunk_size)?;
    let resume = match reader.read_u8()? {
        0 => ResumeMode::Disabled,
        1 => ResumeMode::Allowed,
        wire_id => return Err(DecodeError::InvalidResumeMode { wire_id }),
    };
    Ok(FileHeader {
        transfer_id,
        offered_name,
        file_size,
        chunk_size: ByteCount::new(chunk_size),
        resume,
    })
}

fn decode_resume_status(reader: &mut PayloadReader<'_>) -> Result<ResumeStatus, DecodeError> {
    let transfer_id = reader.read_transfer_id()?;
    let next_chunk_index = reader.read_u64()?;
    let bytes_received = ByteCount::new(reader.read_u64()?);
    let prefix_hash = match reader.read_u8()? {
        0 => None,
        1 => Some(reader.read_hash()?),
        wire_id => return Err(DecodeError::InvalidOptionTag { wire_id }),
    };
    Ok(ResumeStatus {
        transfer_id,
        next_chunk_index,
        bytes_received,
        prefix_hash,
    })
}

fn decode_chunk(reader: &mut PayloadReader<'_>) -> Result<Chunk, DecodeError> {
    Ok(Chunk {
        transfer_id: reader.read_transfer_id()?,
        index: reader.read_u64()?,
        offset: ByteCount::new(reader.read_u64()?),
        bytes: reader.read_owned_bytes(Field::ChunkBytes, MAX_CHUNK_SIZE)?,
    })
}

fn decode_abort(reader: &mut PayloadReader<'_>) -> Result<Abort, DecodeError> {
    let transfer_id = match reader.read_u8()? {
        0 => None,
        1 => Some(reader.read_transfer_id()?),
        wire_id => return Err(DecodeError::InvalidOptionTag { wire_id }),
    };
    let reason = decode_reason(reader.read_u8()?)?;
    Ok(Abort {
        transfer_id,
        reason,
    })
}

fn validate_payload_length(kind: FrameKind, actual: usize) -> Result<(), DecodeError> {
    let valid = match kind {
        FrameKind::Hello | FrameKind::Ready => actual == 0,
        FrameKind::FileHeader => (FILE_HEADER_FIXED_LEN
            ..=FILE_HEADER_FIXED_LEN + MAX_OFFERED_NAME_SIZE)
            .contains(&actual),
        FrameKind::ResumeStatus => {
            actual == RESUME_STATUS_WITHOUT_HASH_LEN
                || actual == RESUME_STATUS_WITHOUT_HASH_LEN + HASH_LEN
        }
        FrameKind::Chunk => (CHUNK_FIXED_LEN..=CHUNK_FIXED_LEN + MAX_CHUNK_SIZE).contains(&actual),
        FrameKind::Complete => actual == TRANSFER_ID_LEN + HASH_LEN,
        FrameKind::CompleteAck => actual == TRANSFER_ID_LEN,
        FrameKind::Abort => actual == 2 || actual == TRANSFER_ID_LEN + 2,
    };
    if valid {
        Ok(())
    } else {
        Err(DecodeError::InvalidPayloadLength { kind, actual })
    }
}

fn write_transfer_id(output: &mut Vec<u8>, transfer_id: TransferId) {
    output.extend_from_slice(&transfer_id.to_bytes());
}

fn write_bounded_bytes(
    output: &mut Vec<u8>,
    field: Field,
    bytes: &[u8],
    maximum: usize,
) -> Result<(), EncodeError> {
    if bytes.len() > maximum {
        return Err(EncodeError::FieldTooLarge {
            field,
            actual: bytes.len(),
            maximum,
        });
    }
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn encode_chunk_size(chunk_size: ByteCount) -> Result<u32, EncodeError> {
    let actual = chunk_size.get();
    if actual == 0 || actual > MAX_CHUNK_SIZE as u64 {
        return Err(EncodeError::InvalidChunkSize {
            actual,
            maximum: MAX_CHUNK_SIZE,
        });
    }
    Ok(actual as u32)
}

fn validate_decoded_chunk_size(actual: u64) -> Result<(), DecodeError> {
    if actual == 0 || actual > MAX_CHUNK_SIZE as u64 {
        Err(DecodeError::InvalidChunkSize {
            actual,
            maximum: MAX_CHUNK_SIZE,
        })
    } else {
        Ok(())
    }
}

const fn encode_reason(reason: ProtocolReason) -> u8 {
    match reason {
        ProtocolReason::Cancelled => 1,
        ProtocolReason::Paused => 2,
        ProtocolReason::Unauthenticated => 3,
        ProtocolReason::VersionMismatch => 4,
        ProtocolReason::ProtocolViolation => 5,
        ProtocolReason::IntegrityMismatch => 6,
        ProtocolReason::StorageFault => 7,
        ProtocolReason::Internal => 8,
        ProtocolReason::ContentConflict => 9,
    }
}

const fn decode_reason(wire_id: u8) -> Result<ProtocolReason, DecodeError> {
    match wire_id {
        1 => Ok(ProtocolReason::Cancelled),
        2 => Ok(ProtocolReason::Paused),
        3 => Ok(ProtocolReason::Unauthenticated),
        4 => Ok(ProtocolReason::VersionMismatch),
        5 => Ok(ProtocolReason::ProtocolViolation),
        6 => Ok(ProtocolReason::IntegrityMismatch),
        7 => Ok(ProtocolReason::StorageFault),
        8 => Ok(ProtocolReason::Internal),
        9 => Ok(ProtocolReason::ContentConflict),
        wire_id => Err(DecodeError::UnknownProtocolReason { wire_id }),
    }
}

struct PayloadReader<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    const fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, DecodeError> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_transfer_id(&mut self) -> Result<TransferId, DecodeError> {
        let bytes = self.take(TRANSFER_ID_LEN)?;
        let mut transfer_id = [0; TRANSFER_ID_LEN];
        transfer_id.copy_from_slice(bytes);
        Ok(TransferId::from_bytes(transfer_id))
    }

    fn read_hash(&mut self) -> Result<ContentHash, DecodeError> {
        let bytes = self.take(HASH_LEN)?;
        let mut hash = [0; HASH_LEN];
        hash.copy_from_slice(bytes);
        Ok(ContentHash::from_bytes(hash))
    }

    fn read_offered_name(&mut self) -> Result<OfferedName, DecodeError> {
        let bytes = self.read_bounded_bytes(Field::OfferedName, MAX_OFFERED_NAME_SIZE)?;
        let value = std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8 {
            field: Field::OfferedName,
        })?;
        let offered_name =
            OfferedName::from_untrusted(value).map_err(|_| DecodeError::InvalidOfferedName)?;
        if offered_name.as_str() != value {
            return Err(DecodeError::InvalidOfferedName);
        }
        Ok(offered_name)
    }

    fn read_owned_bytes(&mut self, field: Field, maximum: usize) -> Result<Vec<u8>, DecodeError> {
        Ok(self.read_bounded_bytes(field, maximum)?.to_vec())
    }

    fn read_bounded_bytes(
        &mut self,
        field: Field,
        maximum: usize,
    ) -> Result<&'a [u8], DecodeError> {
        let declared = self.read_u32()? as usize;
        if declared > maximum {
            return Err(DecodeError::FieldTooLarge {
                field,
                declared,
                maximum,
            });
        }
        self.take(declared)
    }

    fn finish(&self) -> Result<(), DecodeError> {
        if self.offset == self.payload.len() {
            Ok(())
        } else {
            Err(DecodeError::TrailingPayloadBytes {
                count: self.payload.len() - self.offset,
            })
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let remaining = self.payload.len() - self.offset;
        if count > remaining {
            return Err(DecodeError::TruncatedPayload {
                needed: count,
                remaining,
            });
        }
        let end = self.offset + count;
        let bytes = &self.payload[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}
