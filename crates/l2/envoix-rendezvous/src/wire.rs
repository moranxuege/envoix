use std::fmt;
use std::io::ErrorKind;

use envoix_invite::NamespacedRoomKey;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::config::ControlLimits;
use crate::error::{IoOperation, RendezvousError};
use crate::identifiers::{RENDEZVOUS_MAGIC, RENDEZVOUS_WIRE_VERSION};

pub const CONTROL_HEADER_LEN: usize = 12;

const JOIN_KIND: u8 = 1;
const PAIRED_KIND: u8 = 2;
const EXPIRED_KIND: u8 = 3;
const REJECTED_KIND: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Initiator,
    Responder,
}

impl Role {
    const fn wire_id(self) -> u8 {
        match self {
            Self::Initiator => 0,
            Self::Responder => 1,
        }
    }

    fn from_wire_id(wire_id: u8) -> Result<Self, ControlError> {
        match wire_id {
            0 => Ok(Self::Initiator),
            1 => Ok(Self::Responder),
            _ => Err(ControlError::InvalidRole),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Join {
    room_key: NamespacedRoomKey,
}

impl Join {
    pub const fn new(room_key: NamespacedRoomKey) -> Self {
        Self { room_key }
    }

    pub const fn room_key(&self) -> &NamespacedRoomKey {
        &self.room_key
    }

    pub fn into_room_key(self) -> NamespacedRoomKey {
        self.room_key
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Paired {
    pub role: Role,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionReason {
    InvalidControl,
    InvalidRoomKey,
    WaitingRoomsFull,
    JoinDeadline,
    PeerNotSilent,
}

impl RejectionReason {
    const fn wire_id(self) -> u8 {
        match self {
            Self::InvalidControl => 1,
            Self::InvalidRoomKey => 2,
            Self::WaitingRoomsFull => 3,
            Self::JoinDeadline => 4,
            Self::PeerNotSilent => 5,
        }
    }

    fn from_wire_id(wire_id: u8) -> Result<Self, ControlError> {
        match wire_id {
            1 => Ok(Self::InvalidControl),
            2 => Ok(Self::InvalidRoomKey),
            3 => Ok(Self::WaitingRoomsFull),
            4 => Ok(Self::JoinDeadline),
            5 => Ok(Self::PeerNotSilent),
            _ => Err(ControlError::InvalidRejectionReason),
        }
    }
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidControl => formatter.write_str("invalid control message"),
            Self::InvalidRoomKey => formatter.write_str("invalid room key"),
            Self::WaitingRoomsFull => formatter.write_str("waiting-room capacity reached"),
            Self::JoinDeadline => formatter.write_str("join deadline exceeded"),
            Self::PeerNotSilent => formatter.write_str("peer sent data while waiting"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reply {
    Paired(Paired),
    Expired,
    Rejected(RejectionReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlFrame {
    Join(Join),
    Reply(Reply),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    TruncatedHeader,
    WrongMagic,
    UnsupportedVersion,
    UnknownKind,
    NonZeroReserved,
    FrameTooLarge,
    TruncatedFrame,
    TrailingBytes,
    InvalidPayloadLength,
    InvalidRoomKey,
    InvalidRole,
    InvalidRejectionReason,
    UnexpectedFrame,
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => formatter.write_str("rendezvous header is truncated"),
            Self::WrongMagic => formatter.write_str("rendezvous magic is invalid"),
            Self::UnsupportedVersion => formatter.write_str("rendezvous version is unsupported"),
            Self::UnknownKind => formatter.write_str("rendezvous control kind is unknown"),
            Self::NonZeroReserved => formatter.write_str("rendezvous reserved byte is non-zero"),
            Self::FrameTooLarge => formatter.write_str("rendezvous control frame is too large"),
            Self::TruncatedFrame => formatter.write_str("rendezvous control frame is truncated"),
            Self::TrailingBytes => {
                formatter.write_str("rendezvous control frame has trailing bytes")
            }
            Self::InvalidPayloadLength => {
                formatter.write_str("rendezvous control payload length is invalid")
            }
            Self::InvalidRoomKey => formatter.write_str("rendezvous room key is invalid"),
            Self::InvalidRole => formatter.write_str("rendezvous role is invalid"),
            Self::InvalidRejectionReason => {
                formatter.write_str("rendezvous rejection reason is invalid")
            }
            Self::UnexpectedFrame => {
                formatter.write_str("rendezvous control frame is not legal in this state")
            }
        }
    }
}

impl std::error::Error for ControlError {}

#[derive(Clone, Copy)]
struct Header {
    kind: u8,
    payload_len: usize,
}

pub fn encode_control(
    frame: &ControlFrame,
    limits: ControlLimits,
) -> Result<Vec<u8>, ControlError> {
    let (kind, payload) = match frame {
        ControlFrame::Join(join) => {
            let payload = join.room_key().as_str().as_bytes();
            if payload.len() > limits.max_room_key_length() {
                return Err(ControlError::FrameTooLarge);
            }
            (JOIN_KIND, payload.to_vec())
        }
        ControlFrame::Reply(Reply::Paired(paired)) => (PAIRED_KIND, vec![paired.role.wire_id()]),
        ControlFrame::Reply(Reply::Expired) => (EXPIRED_KIND, Vec::new()),
        ControlFrame::Reply(Reply::Rejected(reason)) => (REJECTED_KIND, vec![reason.wire_id()]),
    };
    let payload_len = u32::try_from(payload.len()).map_err(|_| ControlError::FrameTooLarge)?;
    let mut encoded = Vec::with_capacity(CONTROL_HEADER_LEN + payload.len());
    encoded.extend_from_slice(RENDEZVOUS_MAGIC);
    encoded.extend_from_slice(&RENDEZVOUS_WIRE_VERSION.to_be_bytes());
    encoded.push(kind);
    encoded.push(0);
    encoded.extend_from_slice(&payload_len.to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode_control(input: &[u8], limits: ControlLimits) -> Result<ControlFrame, ControlError> {
    if input.len() < CONTROL_HEADER_LEN {
        return Err(ControlError::TruncatedHeader);
    }
    let mut header_bytes = [0; CONTROL_HEADER_LEN];
    header_bytes.copy_from_slice(&input[..CONTROL_HEADER_LEN]);
    let header = decode_header(&header_bytes, limits)?;
    let total = CONTROL_HEADER_LEN
        .checked_add(header.payload_len)
        .ok_or(ControlError::FrameTooLarge)?;
    if input.len() < total {
        return Err(ControlError::TruncatedFrame);
    }
    if input.len() > total {
        return Err(ControlError::TrailingBytes);
    }
    decode_payload(header.kind, &input[CONTROL_HEADER_LEN..])
}

pub async fn read_control<R>(
    reader: &mut R,
    limits: ControlLimits,
) -> Result<ControlFrame, RendezvousError>
where
    R: AsyncRead + Unpin,
{
    let mut header_bytes = [0; CONTROL_HEADER_LEN];
    read_exact_control(reader, &mut header_bytes, ControlError::TruncatedHeader).await?;
    let header = decode_header(&header_bytes, limits)?;
    let mut payload = vec![0; header.payload_len];
    read_exact_control(reader, &mut payload, ControlError::TruncatedFrame).await?;
    decode_payload(header.kind, &payload).map_err(Into::into)
}

pub async fn write_control<W>(
    writer: &mut W,
    frame: &ControlFrame,
    limits: ControlLimits,
) -> Result<(), RendezvousError>
where
    W: AsyncWrite + Unpin,
{
    let encoded = encode_control(frame, limits)?;
    writer
        .write_all(&encoded)
        .await
        .map_err(|_| RendezvousError::Io {
            operation: IoOperation::WriteControl,
        })?;
    writer.flush().await.map_err(|_| RendezvousError::Io {
        operation: IoOperation::WriteControl,
    })
}

fn decode_header(
    header: &[u8; CONTROL_HEADER_LEN],
    limits: ControlLimits,
) -> Result<Header, ControlError> {
    if &header[..4] != RENDEZVOUS_MAGIC {
        return Err(ControlError::WrongMagic);
    }
    if u16::from_be_bytes([header[4], header[5]]) != RENDEZVOUS_WIRE_VERSION {
        return Err(ControlError::UnsupportedVersion);
    }
    let kind = header[6];
    if !matches!(kind, JOIN_KIND | PAIRED_KIND | EXPIRED_KIND | REJECTED_KIND) {
        return Err(ControlError::UnknownKind);
    }
    if header[7] != 0 {
        return Err(ControlError::NonZeroReserved);
    }
    let payload_len = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let maximum = if kind == JOIN_KIND {
        limits.max_room_key_length()
    } else {
        1
    };
    if payload_len > maximum {
        return Err(ControlError::FrameTooLarge);
    }
    Ok(Header { kind, payload_len })
}

fn decode_payload(kind: u8, payload: &[u8]) -> Result<ControlFrame, ControlError> {
    match kind {
        JOIN_KIND => {
            if payload.is_empty() {
                return Err(ControlError::InvalidRoomKey);
            }
            let key = std::str::from_utf8(payload).map_err(|_| ControlError::InvalidRoomKey)?;
            let key = NamespacedRoomKey::parse(key).map_err(|_| ControlError::InvalidRoomKey)?;
            Ok(ControlFrame::Join(Join::new(key)))
        }
        PAIRED_KIND if payload.len() == 1 => Ok(ControlFrame::Reply(Reply::Paired(Paired {
            role: Role::from_wire_id(payload[0])?,
        }))),
        PAIRED_KIND => Err(ControlError::InvalidPayloadLength),
        EXPIRED_KIND if payload.is_empty() => Ok(ControlFrame::Reply(Reply::Expired)),
        EXPIRED_KIND => Err(ControlError::InvalidPayloadLength),
        REJECTED_KIND if payload.len() == 1 => Ok(ControlFrame::Reply(Reply::Rejected(
            RejectionReason::from_wire_id(payload[0])?,
        ))),
        REJECTED_KIND => Err(ControlError::InvalidPayloadLength),
        _ => Err(ControlError::UnknownKind),
    }
}

async fn read_exact_control(
    reader: &mut (impl AsyncRead + Unpin),
    destination: &mut [u8],
    truncated: ControlError,
) -> Result<(), RendezvousError> {
    reader
        .read_exact(destination)
        .await
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == ErrorKind::UnexpectedEof {
                RendezvousError::Control(truncated)
            } else {
                RendezvousError::Io {
                    operation: IoOperation::ReadControl,
                }
            }
        })
}
