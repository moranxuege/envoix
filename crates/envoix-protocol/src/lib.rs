//! Authentication envelope and canonical Manifest v2 wire protocol.

pub mod manifest_v2;
pub mod manifest_v2_frames;

pub use manifest_v2_frames::{
    ManifestV2FrameConnection, read_manifest_v2_frame, write_manifest_v2_frame,
};

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use async_trait::async_trait;
use envoix_error::CoreError;
use envoix_types::PeerRole;
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const AUTH_MAGIC: &[u8; 4] = b"ENVA";
const AUTH_WIRE_VERSION: u16 = 2;
const AUTH_HEADER_BYTES: usize = 12;
const MAX_AUTH_FRAME_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
enum AuthEnvelopeType {
    Auth = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
enum AuthFrameType {
    Start = 1,
    Message = 2,
    Confirm = 3,
}

pub type ProtocolError = CoreError;

/// Only authentication travels through this envelope. Payload starts after
/// authentication and uses [`ManifestV2FrameConnection`].
#[async_trait]
pub trait FrameConnection: Send {
    async fn send_frame(&mut self, frame: Frame) -> Result<(), ProtocolError>;
    async fn recv_frame(&mut self) -> Result<Frame, ProtocolError>;

    fn export_keying_material(
        &self,
        _label: &[u8],
        _context: &[u8],
    ) -> Result<[u8; 32], ProtocolError> {
        Err(CoreError::Transport(
            "transport channel binding is unavailable".into(),
        ))
    }

    async fn close(&mut self) -> Result<(), ProtocolError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferProtocol {
    ManifestV2,
}

impl TransferProtocol {
    pub const fn alpn(self) -> &'static [u8] {
        match self {
            Self::ManifestV2 => manifest_v2::MANIFEST_V2_ALPN,
        }
    }

    pub fn from_alpn(alpn: &[u8]) -> Option<Self> {
        (alpn == manifest_v2::MANIFEST_V2_ALPN).then_some(Self::ManifestV2)
    }
}

/// Direct addressing data needed to dial an iroh endpoint without a relay.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub struct PeerDescriptor {
    pub endpoint_id: String,
    pub direct_addrs: Vec<SocketAddr>,
}

impl PeerDescriptor {
    pub fn new(
        endpoint_id: impl Into<String>,
        direct_addrs: Vec<SocketAddr>,
    ) -> Result<Self, ProtocolError> {
        let descriptor = Self {
            endpoint_id: endpoint_id.into(),
            direct_addrs,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.endpoint_id.trim().is_empty() {
            return Err(CoreError::InvalidInput("endpoint id is empty".into()));
        }
        if self.direct_addrs.is_empty() {
            return Err(CoreError::InvalidInput(
                "peer descriptor has no direct addresses".into(),
            ));
        }
        Ok(())
    }

    pub fn parse_compact(input: &str) -> Result<Self, ProtocolError> {
        input.parse()
    }
}

impl fmt::Display for PeerDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@", self.endpoint_id)?;
        for (index, address) in self.direct_addrs.iter().enumerate() {
            if index > 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{address}")?;
        }
        Ok(())
    }
}

impl FromStr for PeerDescriptor {
    type Err = ProtocolError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (endpoint_id, addresses) = input
            .trim()
            .split_once('@')
            .ok_or_else(|| CoreError::InvalidInput("peer descriptor must contain '@'".into()))?;
        let direct_addrs = addresses
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(|address| {
                address.parse::<SocketAddr>().map_err(|_| {
                    CoreError::InvalidInput(format!("malformed peer address {address:?}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(endpoint_id.trim(), direct_addrs)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Frame {
    Auth(AuthFrame),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AuthFrame {
    Spake2Start(Spake2Start),
    Spake2Message(Spake2Message),
    Spake2Confirm(Spake2Confirm),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Spake2Start {
    pub protocol_version: u32,
    pub role: PeerRole,
    pub nonce: Vec<u8>,
    pub message: Vec<u8>,
    pub remember_consent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Spake2Message {
    pub nonce: Vec<u8>,
    pub message: Vec<u8>,
    pub remember_consent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Spake2Confirm {
    pub proof: Vec<u8>,
    pub remember_contribution: Option<Vec<u8>>,
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Frame, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; AUTH_HEADER_BYTES];
    reader.read_exact(&mut header).await?;
    if &header[..4] != AUTH_MAGIC {
        return Err(CoreError::Protocol("bad authentication frame magic".into()));
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != AUTH_WIRE_VERSION {
        return Err(CoreError::Protocol(format!(
            "unsupported authentication frame version {version}"
        )));
    }
    let envelope = AuthEnvelopeType::try_from(header[6])
        .map_err(|error| CoreError::Protocol(error.to_string()))?;
    if envelope != AuthEnvelopeType::Auth || header[7] != 0 {
        return Err(CoreError::Protocol(
            "invalid authentication frame header".into(),
        ));
    }
    let length = u32::from_be_bytes(header[8..12].try_into().expect("fixed header")) as usize;
    if length > MAX_AUTH_FRAME_BYTES {
        return Err(CoreError::Protocol(
            "authentication frame exceeds its allocation bound".into(),
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    decode_auth_frame(&payload)
}

pub async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let payload = encode_auth_frame(frame)?;
    if payload.len() > MAX_AUTH_FRAME_BYTES {
        return Err(CoreError::Protocol(
            "authentication frame exceeds its allocation bound".into(),
        ));
    }
    writer.write_all(AUTH_MAGIC).await?;
    writer.write_all(&AUTH_WIRE_VERSION.to_be_bytes()).await?;
    writer.write_all(&[AuthEnvelopeType::Auth as u8, 0]).await?;
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&payload).await?;
    Ok(())
}

pub async fn flush_frame_writer<W>(writer: &mut W) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    writer.flush().await?;
    Ok(())
}

fn encode_auth_frame(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    let mut payload = Vec::new();
    match frame {
        Frame::Auth(AuthFrame::Spake2Start(start)) => {
            payload.push(AuthFrameType::Start as u8);
            payload.extend_from_slice(&start.protocol_version.to_be_bytes());
            write_peer_role(&mut payload, start.role);
            write_bytes(&mut payload, &start.nonce)?;
            write_bytes(&mut payload, &start.message)?;
            payload.push(u8::from(start.remember_consent));
        }
        Frame::Auth(AuthFrame::Spake2Message(message)) => {
            payload.push(AuthFrameType::Message as u8);
            write_bytes(&mut payload, &message.nonce)?;
            write_bytes(&mut payload, &message.message)?;
            payload.push(u8::from(message.remember_consent));
        }
        Frame::Auth(AuthFrame::Spake2Confirm(confirm)) => {
            payload.push(AuthFrameType::Confirm as u8);
            write_bytes(&mut payload, &confirm.proof)?;
            payload.push(u8::from(confirm.remember_contribution.is_some()));
            if let Some(contribution) = &confirm.remember_contribution {
                write_bytes(&mut payload, contribution)?;
            }
        }
    }
    Ok(payload)
}

fn decode_auth_frame(payload: &[u8]) -> Result<Frame, ProtocolError> {
    let mut reader = PayloadReader::new(payload);
    let frame_type = AuthFrameType::try_from(reader.read_u8()?)
        .map_err(|error| CoreError::Protocol(error.to_string()))?;
    let auth = match frame_type {
        AuthFrameType::Start => AuthFrame::Spake2Start(Spake2Start {
            protocol_version: reader.read_u32()?,
            role: reader.read_peer_role()?,
            nonce: reader.read_bytes()?,
            message: reader.read_bytes()?,
            remember_consent: reader.read_bool()?,
        }),
        AuthFrameType::Message => AuthFrame::Spake2Message(Spake2Message {
            nonce: reader.read_bytes()?,
            message: reader.read_bytes()?,
            remember_consent: reader.read_bool()?,
        }),
        AuthFrameType::Confirm => AuthFrame::Spake2Confirm(Spake2Confirm {
            proof: reader.read_bytes()?,
            remember_contribution: if reader.read_bool()? {
                Some(reader.read_bytes()?)
            } else {
                None
            },
        }),
    };
    reader.finish()?;
    Ok(Frame::Auth(auth))
}

fn write_peer_role(output: &mut Vec<u8>, role: PeerRole) {
    output.push(match role {
        PeerRole::Sender => 1,
        PeerRole::Receiver => 2,
    });
}

fn write_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), ProtocolError> {
    let length = u32::try_from(value.len())
        .map_err(|_| CoreError::Protocol("authentication field exceeds u32".into()))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

struct PayloadReader<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self { payload, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("fixed u32"),
        ))
    }

    fn read_peer_role(&mut self) -> Result<PeerRole, ProtocolError> {
        match self.read_u8()? {
            1 => Ok(PeerRole::Sender),
            2 => Ok(PeerRole::Receiver),
            role => Err(CoreError::Protocol(format!("unknown peer role {role}"))),
        }
    }

    fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(CoreError::Protocol(format!(
                "invalid authentication boolean {value}"
            ))),
        }
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>, ProtocolError> {
        let length = self.read_u32()? as usize;
        Ok(self.take(length)?.to_vec())
    }

    fn finish(&self) -> Result<(), ProtocolError> {
        if self.offset == self.payload.len() {
            Ok(())
        } else {
            Err(CoreError::Protocol(
                "authentication frame has trailing bytes".into(),
            ))
        }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| CoreError::Protocol("authentication field length overflow".into()))?;
        let value = self
            .payload
            .get(self.offset..end)
            .ok_or_else(|| CoreError::Protocol("truncated authentication frame".into()))?;
        self.offset = end;
        Ok(value)
    }
}
