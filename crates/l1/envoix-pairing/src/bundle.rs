use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroizing;

use crate::PairingError;
use crate::handshake::Role;
use crate::message::{
    AEAD_NONCE_SIZE, MAX_DESCRIPTOR_SIZE, MessageKind, PairingMessage, SealedDescriptor,
    decode_message, encode_message,
};
use crate::secret::DataPlaneToken;

#[derive(Eq, PartialEq)]
pub struct DescriptorPayload(Vec<u8>);

impl DescriptorPayload {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, PairingError> {
        let bytes = bytes.into();
        if bytes.len() > MAX_DESCRIPTOR_SIZE {
            return Err(PairingError::DescriptorTooLarge {
                actual: bytes.len(),
                maximum: MAX_DESCRIPTOR_SIZE,
            });
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for DescriptorPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescriptorPayload")
            .field("length", &self.0.len())
            .finish()
    }
}

#[derive(Eq, PartialEq)]
pub struct PeerDescriptor {
    payload: DescriptorPayload,
    data_token: DataPlaneToken,
}

impl PeerDescriptor {
    pub fn payload(&self) -> &DescriptorPayload {
        &self.payload
    }

    pub fn data_token(&self) -> &DataPlaneToken {
        &self.data_token
    }
}

impl fmt::Debug for PeerDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerDescriptor")
            .field("payload", &self.payload)
            .field("data_token", &self.data_token)
            .finish()
    }
}

pub(crate) fn seal_descriptor(
    bundle_key: &[u8; 32],
    role: Role,
    nonce: [u8; AEAD_NONCE_SIZE],
    descriptor: &DescriptorPayload,
    data_token: &[u8; 32],
) -> Result<Vec<u8>, PairingError> {
    let mut plaintext = Zeroizing::new(Vec::with_capacity(
        4 + descriptor.as_bytes().len() + data_token.len(),
    ));
    plaintext.extend_from_slice(&(descriptor.as_bytes().len() as u32).to_be_bytes());
    plaintext.extend_from_slice(descriptor.as_bytes());
    plaintext.extend_from_slice(data_token);
    let sealed = seal_with_nonce(bundle_key, role.seal_aad(), nonce, &plaintext)?;
    encode_message(&PairingMessage::SealedDescriptor(sealed))
}

pub(crate) fn open_descriptor(
    bundle_key: &[u8; 32],
    peer_role: Role,
    encoded: &[u8],
    expected_token: &[u8; 32],
) -> Result<PeerDescriptor, PairingError> {
    let sealed = match decode_message(encoded)? {
        PairingMessage::SealedDescriptor(sealed) => sealed,
        other => {
            return Err(PairingError::UnexpectedMessage {
                expected: MessageKind::SealedDescriptor,
                actual: other.kind(),
            });
        }
    };
    if sealed.nonce()[..4] != peer_role.nonce_prefix() {
        return Err(PairingError::AuthenticationFailed);
    }
    let plaintext = open_sealed(bundle_key, peer_role.seal_aad(), &sealed)?;
    decode_descriptor(&plaintext, expected_token)
}

pub(crate) fn seal_with_nonce(
    bundle_key: &[u8; 32],
    aad: &[u8],
    nonce: [u8; AEAD_NONCE_SIZE],
    plaintext: &[u8],
) -> Result<SealedDescriptor, PairingError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(bundle_key));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| PairingError::AuthenticationFailed)?;
    SealedDescriptor::new(nonce, ciphertext)
}

pub(crate) fn open_sealed(
    bundle_key: &[u8; 32],
    aad: &[u8],
    sealed: &SealedDescriptor,
) -> Result<Zeroizing<Vec<u8>>, PairingError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(bundle_key));
    cipher
        .decrypt(
            Nonce::from_slice(sealed.nonce()),
            Payload {
                msg: sealed.ciphertext(),
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| PairingError::AuthenticationFailed)
}

fn decode_descriptor(
    plaintext: &[u8],
    expected_token: &[u8; 32],
) -> Result<PeerDescriptor, PairingError> {
    if plaintext.len() < 4 + expected_token.len() {
        return Err(PairingError::InvalidDescriptor);
    }
    let descriptor_len =
        u32::from_be_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]) as usize;
    if descriptor_len > MAX_DESCRIPTOR_SIZE {
        return Err(PairingError::InvalidDescriptor);
    }
    let expected_len = 4 + descriptor_len + expected_token.len();
    if plaintext.len() != expected_len {
        return Err(PairingError::InvalidDescriptor);
    }
    let token_offset = 4 + descriptor_len;
    let mut received_token = Zeroizing::new([0; 32]);
    received_token.copy_from_slice(&plaintext[token_offset..]);
    // BLAKE3's fixed-size Hash equality is constant-time.
    if blake3::Hash::from_bytes(*received_token) != blake3::Hash::from_bytes(*expected_token) {
        return Err(PairingError::DataTokenMismatch);
    }
    Ok(PeerDescriptor {
        payload: DescriptorPayload(plaintext[4..token_offset].to_vec()),
        data_token: DataPlaneToken::from_zeroizing(received_token),
    })
}
