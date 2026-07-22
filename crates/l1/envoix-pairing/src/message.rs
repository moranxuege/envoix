use crate::PairingError;

pub const WIRE_HEADER_LEN: usize = 5;
pub const MAX_MESSAGE_BODY: usize = 64 * 1024;
pub const SPAKE_MESSAGE_SIZE: usize = 33;
pub const CONFIRMATION_SIZE: usize = 32;
pub const AEAD_NONCE_SIZE: usize = 12;
pub const AEAD_TAG_SIZE: usize = 16;
pub const MAX_DESCRIPTOR_SIZE: usize = 32 * 1024;
pub const MAX_SEALED_CIPHERTEXT_SIZE: usize = MAX_DESCRIPTOR_SIZE + 4 + 32 + AEAD_TAG_SIZE;

const _: () = assert!(MAX_MESSAGE_BODY <= u32::MAX as usize);
const _: () = assert!(MAX_SEALED_CIPHERTEXT_SIZE + AEAD_NONCE_SIZE <= MAX_MESSAGE_BODY);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MessageKind {
    Start = 1,
    Response = 2,
    Confirm = 3,
    SealedDescriptor = 4,
}

impl MessageKind {
    pub const fn wire_id(self) -> u8 {
        self as u8
    }

    const fn from_wire_id(wire_id: u8) -> Option<Self> {
        match wire_id {
            1 => Some(Self::Start),
            2 => Some(Self::Response),
            3 => Some(Self::Confirm),
            4 => Some(Self::SealedDescriptor),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PakeStart {
    message: Vec<u8>,
}

impl PakeStart {
    pub fn message(&self) -> &[u8] {
        &self.message
    }

    pub(crate) fn new(message: Vec<u8>) -> Result<Self, PairingError> {
        validate_exact_length(MessageKind::Start, message.len(), SPAKE_MESSAGE_SIZE)?;
        Ok(Self { message })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PakeResponse {
    message: Vec<u8>,
}

impl PakeResponse {
    pub fn message(&self) -> &[u8] {
        &self.message
    }

    pub(crate) fn new(message: Vec<u8>) -> Result<Self, PairingError> {
        validate_exact_length(MessageKind::Response, message.len(), SPAKE_MESSAGE_SIZE)?;
        Ok(Self { message })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Confirmation {
    tag: [u8; CONFIRMATION_SIZE],
}

impl Confirmation {
    pub const fn tag(&self) -> &[u8; CONFIRMATION_SIZE] {
        &self.tag
    }

    pub(crate) const fn new(tag: [u8; CONFIRMATION_SIZE]) -> Self {
        Self { tag }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedDescriptor {
    nonce: [u8; AEAD_NONCE_SIZE],
    ciphertext: Vec<u8>,
}

impl SealedDescriptor {
    pub const fn nonce(&self) -> &[u8; AEAD_NONCE_SIZE] {
        &self.nonce
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub(crate) fn new(
        nonce: [u8; AEAD_NONCE_SIZE],
        ciphertext: Vec<u8>,
    ) -> Result<Self, PairingError> {
        if !(AEAD_TAG_SIZE..=MAX_SEALED_CIPHERTEXT_SIZE).contains(&ciphertext.len()) {
            return Err(PairingError::InvalidMessageLength {
                kind: MessageKind::SealedDescriptor,
                actual: AEAD_NONCE_SIZE + ciphertext.len(),
            });
        }
        Ok(Self { nonce, ciphertext })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingMessage {
    Start(PakeStart),
    Response(PakeResponse),
    Confirm(Confirmation),
    SealedDescriptor(SealedDescriptor),
}

impl PairingMessage {
    pub const fn kind(&self) -> MessageKind {
        match self {
            Self::Start(_) => MessageKind::Start,
            Self::Response(_) => MessageKind::Response,
            Self::Confirm(_) => MessageKind::Confirm,
            Self::SealedDescriptor(_) => MessageKind::SealedDescriptor,
        }
    }
}

pub fn encode_message(message: &PairingMessage) -> Result<Vec<u8>, PairingError> {
    let mut encoded = Vec::new();
    encoded.push(message.kind().wire_id());
    encoded.extend_from_slice(&[0; 4]);
    match message {
        PairingMessage::Start(start) => encoded.extend_from_slice(start.message()),
        PairingMessage::Response(response) => encoded.extend_from_slice(response.message()),
        PairingMessage::Confirm(confirm) => encoded.extend_from_slice(confirm.tag()),
        PairingMessage::SealedDescriptor(sealed) => {
            encoded.extend_from_slice(sealed.nonce());
            encoded.extend_from_slice(sealed.ciphertext());
        }
    }
    let body_len = encoded.len() - WIRE_HEADER_LEN;
    if body_len > MAX_MESSAGE_BODY {
        return Err(PairingError::MessageTooLarge {
            declared: body_len,
            maximum: MAX_MESSAGE_BODY,
        });
    }
    encoded[1..5].copy_from_slice(&(body_len as u32).to_be_bytes());
    Ok(encoded)
}

pub fn decode_message(encoded: &[u8]) -> Result<PairingMessage, PairingError> {
    if encoded.len() < WIRE_HEADER_LEN {
        return Err(PairingError::TruncatedMessageHeader {
            actual: encoded.len(),
        });
    }
    let kind = MessageKind::from_wire_id(encoded[0]).ok_or(PairingError::UnknownMessageType {
        wire_id: encoded[0],
    })?;
    let body_len = u32::from_be_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]) as usize;
    if body_len > MAX_MESSAGE_BODY {
        return Err(PairingError::MessageTooLarge {
            declared: body_len,
            maximum: MAX_MESSAGE_BODY,
        });
    }
    let actual = encoded.len() - WIRE_HEADER_LEN;
    if actual < body_len {
        return Err(PairingError::TruncatedMessage {
            declared: body_len,
            actual,
        });
    }
    if actual > body_len {
        return Err(PairingError::TrailingMessageBytes {
            count: actual - body_len,
        });
    }
    let body = &encoded[WIRE_HEADER_LEN..];
    match kind {
        MessageKind::Start => {
            validate_exact_length(kind, body.len(), SPAKE_MESSAGE_SIZE)?;
            Ok(PairingMessage::Start(PakeStart::new(body.to_vec())?))
        }
        MessageKind::Response => {
            validate_exact_length(kind, body.len(), SPAKE_MESSAGE_SIZE)?;
            Ok(PairingMessage::Response(PakeResponse::new(body.to_vec())?))
        }
        MessageKind::Confirm => {
            validate_exact_length(kind, body.len(), CONFIRMATION_SIZE)?;
            let mut tag = [0; CONFIRMATION_SIZE];
            tag.copy_from_slice(body);
            Ok(PairingMessage::Confirm(Confirmation::new(tag)))
        }
        MessageKind::SealedDescriptor => {
            if !(AEAD_NONCE_SIZE + AEAD_TAG_SIZE..=AEAD_NONCE_SIZE + MAX_SEALED_CIPHERTEXT_SIZE)
                .contains(&body.len())
            {
                return Err(PairingError::InvalidMessageLength {
                    kind,
                    actual: body.len(),
                });
            }
            let mut nonce = [0; AEAD_NONCE_SIZE];
            nonce.copy_from_slice(&body[..AEAD_NONCE_SIZE]);
            Ok(PairingMessage::SealedDescriptor(SealedDescriptor::new(
                nonce,
                body[AEAD_NONCE_SIZE..].to_vec(),
            )?))
        }
    }
}

fn validate_exact_length(
    kind: MessageKind,
    actual: usize,
    expected: usize,
) -> Result<(), PairingError> {
    if actual == expected {
        Ok(())
    } else {
        Err(PairingError::InvalidMessageLength { kind, actual })
    }
}
