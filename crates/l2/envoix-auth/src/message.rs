use std::fmt;

use envoix_protocol::HEADER_LEN;
use envoix_protocol::identifiers::{DATA_MAGIC, DATA_WIRE_VERSION};

use crate::AuthCodecError;
use crate::error::AuthField;
use crate::handshake::PeerRole;

pub const AUTH_WIRE_ID: u8 = 1;
pub const MAX_AUTH_PAYLOAD: usize = 4 * 1024;
pub const NONCE_SIZE: usize = 32;
pub const START_MESSAGE_SIZE: usize = 33;
pub const RESPONSE_MESSAGE_SIZE: usize = 33;
pub const CONFIRMATION_SIZE: usize = 32;

const _: () = assert!(MAX_AUTH_PAYLOAD <= u32::MAX as usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AuthMessageKind {
    Start = 1,
    Response = 2,
    Confirm = 3,
}

impl AuthMessageKind {
    pub const fn wire_id(self) -> u8 {
        self as u8
    }

    fn from_wire_id(wire_id: u8) -> Option<Self> {
        match wire_id {
            1 => Some(Self::Start),
            2 => Some(Self::Response),
            3 => Some(Self::Confirm),
            _ => None,
        }
    }
}

impl fmt::Display for AuthMessageKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start => formatter.write_str("start"),
            Self::Response => formatter.write_str("response"),
            Self::Confirm => formatter.write_str("confirm"),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Start {
    role: PeerRole,
    nonce: [u8; NONCE_SIZE],
    message: [u8; START_MESSAGE_SIZE],
}

impl Start {
    pub const fn role(&self) -> PeerRole {
        self.role
    }

    pub const fn nonce(&self) -> &[u8; NONCE_SIZE] {
        &self.nonce
    }

    pub const fn message(&self) -> &[u8; START_MESSAGE_SIZE] {
        &self.message
    }

    pub(crate) const fn new(
        role: PeerRole,
        nonce: [u8; NONCE_SIZE],
        message: [u8; START_MESSAGE_SIZE],
    ) -> Self {
        Self {
            role,
            nonce,
            message,
        }
    }
}

impl fmt::Debug for Start {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Start")
            .field("role", &self.role)
            .field("nonce", &"[redacted]")
            .field("message", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Response {
    nonce: [u8; NONCE_SIZE],
    message: [u8; RESPONSE_MESSAGE_SIZE],
}

impl Response {
    pub const fn nonce(&self) -> &[u8; NONCE_SIZE] {
        &self.nonce
    }

    pub const fn message(&self) -> &[u8; RESPONSE_MESSAGE_SIZE] {
        &self.message
    }

    pub(crate) const fn new(nonce: [u8; NONCE_SIZE], message: [u8; RESPONSE_MESSAGE_SIZE]) -> Self {
        Self { nonce, message }
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Response")
            .field("nonce", &"[redacted]")
            .field("message", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Confirmation {
    proof: [u8; CONFIRMATION_SIZE],
}

impl Confirmation {
    pub const fn proof(&self) -> &[u8; CONFIRMATION_SIZE] {
        &self.proof
    }

    pub(crate) const fn new(proof: [u8; CONFIRMATION_SIZE]) -> Self {
        Self { proof }
    }
}

impl fmt::Debug for Confirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Confirmation")
            .field("proof", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum AuthMessage {
    Start(Start),
    Response(Response),
    Confirm(Confirmation),
}

impl AuthMessage {
    pub const fn kind(&self) -> AuthMessageKind {
        match self {
            Self::Start(_) => AuthMessageKind::Start,
            Self::Response(_) => AuthMessageKind::Response,
            Self::Confirm(_) => AuthMessageKind::Confirm,
        }
    }
}

impl fmt::Debug for AuthMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("AuthMessage")
            .field(&self.kind())
            .finish()
    }
}

pub fn encode_auth_message(message: &AuthMessage) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(HEADER_LEN + 80);
    encoded.extend_from_slice(DATA_MAGIC);
    encoded.extend_from_slice(&DATA_WIRE_VERSION.to_be_bytes());
    encoded.extend_from_slice(&[AUTH_WIRE_ID, 0]);
    encoded.extend_from_slice(&[0; 4]);
    encoded.push(message.kind().wire_id());

    match message {
        AuthMessage::Start(start) => {
            encoded.push(start.role.wire_id());
            write_sized(&mut encoded, start.nonce());
            write_sized(&mut encoded, start.message());
        }
        AuthMessage::Response(response) => {
            write_sized(&mut encoded, response.nonce());
            write_sized(&mut encoded, response.message());
        }
        AuthMessage::Confirm(confirmation) => {
            write_sized(&mut encoded, confirmation.proof());
        }
    }

    let payload_length = encoded.len() - HEADER_LEN;
    encoded[8..12].copy_from_slice(&(payload_length as u32).to_be_bytes());
    encoded
}

pub fn decode_auth_message(input: &[u8]) -> Result<AuthMessage, AuthCodecError> {
    if input.len() < HEADER_LEN {
        return Err(AuthCodecError::TruncatedHeader {
            actual: input.len(),
        });
    }
    if &input[..4] != DATA_MAGIC {
        return Err(AuthCodecError::WrongMagic);
    }
    let version = u16::from_be_bytes([input[4], input[5]]);
    if version != DATA_WIRE_VERSION {
        return Err(AuthCodecError::UnsupportedVersion { actual: version });
    }
    if input[6] != AUTH_WIRE_ID {
        return Err(AuthCodecError::WrongWireId { actual: input[6] });
    }
    if input[7] != 0 {
        return Err(AuthCodecError::NonZeroReservedByte);
    }

    let declared = u32::from_be_bytes([input[8], input[9], input[10], input[11]]) as usize;
    if declared > MAX_AUTH_PAYLOAD {
        return Err(AuthCodecError::PayloadTooLarge {
            declared,
            maximum: MAX_AUTH_PAYLOAD,
        });
    }
    let actual = input.len() - HEADER_LEN;
    if actual < declared {
        return Err(AuthCodecError::TruncatedFrame { declared, actual });
    }
    if actual > declared {
        return Err(AuthCodecError::TrailingFrameBytes {
            count: actual - declared,
        });
    }

    let mut reader = PayloadReader::new(&input[HEADER_LEN..]);
    let kind_id = reader.read_u8()?;
    let kind = AuthMessageKind::from_wire_id(kind_id)
        .ok_or(AuthCodecError::UnknownMessageKind { wire_id: kind_id })?;
    let message = match kind {
        AuthMessageKind::Start => {
            let role_id = reader.read_u8()?;
            let role = PeerRole::from_wire_id(role_id)?;
            AuthMessage::Start(Start::new(
                role,
                reader.read_sized(AuthField::Nonce)?,
                reader.read_sized(AuthField::SpakeMessage)?,
            ))
        }
        AuthMessageKind::Response => AuthMessage::Response(Response::new(
            reader.read_sized(AuthField::Nonce)?,
            reader.read_sized(AuthField::SpakeMessage)?,
        )),
        AuthMessageKind::Confirm => AuthMessage::Confirm(Confirmation::new(
            reader.read_sized(AuthField::Confirmation)?,
        )),
    };
    reader.finish()?;
    Ok(message)
}

fn write_sized(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
}

struct PayloadReader<'a> {
    remaining: &'a [u8],
}

impl<'a> PayloadReader<'a> {
    const fn new(payload: &'a [u8]) -> Self {
        Self { remaining: payload }
    }

    fn read_u8(&mut self) -> Result<u8, AuthCodecError> {
        let Some((&value, rest)) = self.remaining.split_first() else {
            return Err(AuthCodecError::TruncatedPayload {
                needed: 1,
                remaining: 0,
            });
        };
        self.remaining = rest;
        Ok(value)
    }

    fn read_sized<const N: usize>(&mut self, field: AuthField) -> Result<[u8; N], AuthCodecError> {
        let length = self.read_u32()? as usize;
        if length != N {
            return Err(AuthCodecError::InvalidFieldLength {
                field,
                actual: length,
                expected: N,
            });
        }
        if self.remaining.len() < N {
            return Err(AuthCodecError::TruncatedPayload {
                needed: N,
                remaining: self.remaining.len(),
            });
        }
        let (value, rest) = self.remaining.split_at(N);
        self.remaining = rest;
        Ok(value.try_into().expect("slice length was checked"))
    }

    fn read_u32(&mut self) -> Result<u32, AuthCodecError> {
        if self.remaining.len() < 4 {
            return Err(AuthCodecError::TruncatedPayload {
                needed: 4,
                remaining: self.remaining.len(),
            });
        }
        let (value, rest) = self.remaining.split_at(4);
        self.remaining = rest;
        Ok(u32::from_be_bytes(
            value.try_into().expect("slice length was checked"),
        ))
    }

    fn finish(self) -> Result<(), AuthCodecError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(AuthCodecError::TrailingPayloadBytes {
                count: self.remaining.len(),
            })
        }
    }
}
