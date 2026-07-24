use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use envoix_types::{ByteCount, TransferId};
use zeroize::Zeroizing;

use crate::ContentHash;

use super::identifiers::{
    RECEIPT_AAD_PREFIX, RECEIPT_KIND, RECEIPT_PAYLOAD_SCHEMA_ID, RECEIPT_SEAL_KDF_CONTEXT,
    RECEIPT_SLOT_KDF_CONTEXT, encode_length_prefixed,
};

pub const MAX_SEALED_RECEIPT_SIZE: usize =
    8 * 4 + RECEIPT_PAYLOAD_SCHEMA_ID.len() + RECEIPT_KIND.len() + 32 + 8 + 16;

const TAG_SIZE: usize = 16;
// The seal key is unique to one transfer. The honest receiver seals one canonical
// receipt and retries that same blob, so zero is used only once under each key.
const ZERO_NONCE: [u8; 12] = [0; 12];

/// The receiver's proof of receipt. It is a pure function of the transfer's
/// committed facts — content hash + size — with no mutable field, so a receiver
/// can only ever seal one canonical blob per transfer (see `seal_receipt`: this
/// is what makes the fixed zero nonce safe). The landed filename is deliberately
/// NOT sealed: it is never identity (the sender verifies hash + size only), and
/// keeping it would both reintroduce a nonce-reuse hazard and leak it to the
/// untrusted mailbox.
#[derive(Clone, Eq, PartialEq)]
pub struct ReceiptPayload {
    file_hash: ContentHash,
    file_size: ByteCount,
}

impl fmt::Debug for ReceiptPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReceiptPayload([redacted])")
    }
}

impl ReceiptPayload {
    pub const fn new(file_hash: ContentHash, file_size: ByteCount) -> Self {
        Self {
            file_hash,
            file_size,
        }
    }

    pub const fn file_hash(&self) -> ContentHash {
        self.file_hash
    }

    pub const fn file_size(&self) -> ByteCount {
        self.file_size
    }

    fn encode(&self) -> Vec<u8> {
        let size = self.file_size.get().to_be_bytes();
        encode_length_prefixed(&[
            RECEIPT_PAYLOAD_SCHEMA_ID.as_bytes(),
            RECEIPT_KIND.as_bytes(),
            self.file_hash.as_bytes(),
            &size,
        ])
    }

    fn decode(encoded: &[u8]) -> Result<Self, MailboxProtocolError> {
        let parts = decode_parts(encoded, 4)?;
        if parts[0] != RECEIPT_PAYLOAD_SCHEMA_ID.as_bytes()
            || parts[1] != RECEIPT_KIND.as_bytes()
            || parts[2].len() != 32
            || parts[3].len() != 8
        {
            return Err(MailboxProtocolError::InvalidPayload);
        }

        let file_hash = ContentHash::from_bytes(
            parts[2]
                .try_into()
                .map_err(|_| MailboxProtocolError::InvalidPayload)?,
        );
        let file_size = ByteCount::new(u64::from_be_bytes(
            parts[3]
                .try_into()
                .map_err(|_| MailboxProtocolError::InvalidPayload)?,
        ));
        Ok(Self::new(file_hash, file_size))
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ReceiptSlot([u8; 32]);

impl ReceiptSlot {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn path_component(&self) -> String {
        let mut encoded = String::with_capacity(64);
        for byte in self.0 {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
        }
        encoded
    }
}

impl fmt::Debug for ReceiptSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReceiptSlot([opaque])")
    }
}

impl fmt::Display for ReceiptSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.path_component())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SealedReceipt(Vec<u8>);

impl SealedReceipt {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, MailboxProtocolError> {
        if bytes.len() < TAG_SIZE || bytes.len() > MAX_SEALED_RECEIPT_SIZE {
            return Err(MailboxProtocolError::InvalidSealedReceipt);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for SealedReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealedReceipt([opaque])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxProtocolError {
    InvalidPayload,
    InvalidSealedReceipt,
    SealFailed,
    AuthenticationFailed,
}

impl fmt::Display for MailboxProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload => formatter.write_str("invalid receipt payload"),
            Self::InvalidSealedReceipt => formatter.write_str("invalid sealed receipt"),
            Self::SealFailed => formatter.write_str("receipt sealing failed"),
            Self::AuthenticationFailed => formatter.write_str("receipt authentication failed"),
        }
    }
}

impl std::error::Error for MailboxProtocolError {}

pub fn receipt_slot(transfer_id: TransferId) -> ReceiptSlot {
    let material = encode_length_prefixed(&[&transfer_id.to_bytes()]);
    ReceiptSlot(blake3::derive_key(RECEIPT_SLOT_KDF_CONTEXT, &material))
}

pub fn seal_receipt(
    transfer_id: TransferId,
    mailbox_secret: &[u8; 32],
    receipt: &ReceiptPayload,
) -> Result<SealedReceipt, MailboxProtocolError> {
    let key = derive_seal_key(transfer_id, mailbox_secret);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key[..]));
    let aad = receipt_aad(transfer_id);
    let plaintext = Zeroizing::new(receipt.encode());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&ZERO_NONCE),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| MailboxProtocolError::SealFailed)?;
    SealedReceipt::from_bytes(ciphertext)
}

pub fn open_receipt(
    transfer_id: TransferId,
    mailbox_secret: &[u8; 32],
    sealed: &SealedReceipt,
) -> Result<ReceiptPayload, MailboxProtocolError> {
    let key = derive_seal_key(transfer_id, mailbox_secret);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key[..]));
    let aad = receipt_aad(transfer_id);
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&ZERO_NONCE),
                Payload {
                    msg: sealed.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| MailboxProtocolError::AuthenticationFailed)?,
    );
    ReceiptPayload::decode(&plaintext)
}

fn derive_seal_key(transfer_id: TransferId, mailbox_secret: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let material = Zeroizing::new(encode_length_prefixed(&[
        &transfer_id.to_bytes(),
        mailbox_secret,
    ]));
    Zeroizing::new(blake3::derive_key(RECEIPT_SEAL_KDF_CONTEXT, &material))
}

fn receipt_aad(transfer_id: TransferId) -> Vec<u8> {
    encode_length_prefixed(&[
        RECEIPT_AAD_PREFIX.as_bytes(),
        RECEIPT_KIND.as_bytes(),
        RECEIPT_PAYLOAD_SCHEMA_ID.as_bytes(),
        &transfer_id.to_bytes(),
    ])
}

fn decode_parts(encoded: &[u8], count: usize) -> Result<Vec<&[u8]>, MailboxProtocolError> {
    let mut remaining = encoded;
    let mut parts = Vec::with_capacity(count);
    for _ in 0..count {
        let length_bytes = remaining
            .get(..8)
            .ok_or(MailboxProtocolError::InvalidPayload)?;
        let length = u64::from_be_bytes(
            length_bytes
                .try_into()
                .map_err(|_| MailboxProtocolError::InvalidPayload)?,
        );
        let length = usize::try_from(length).map_err(|_| MailboxProtocolError::InvalidPayload)?;
        remaining = &remaining[8..];
        let part = remaining
            .get(..length)
            .ok_or(MailboxProtocolError::InvalidPayload)?;
        parts.push(part);
        remaining = &remaining[length..];
    }
    if !remaining.is_empty() {
        return Err(MailboxProtocolError::InvalidPayload);
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ReceiptPayload {
        ReceiptPayload::new(ContentHash::from_bytes([0x42; 32]), ByteCount::new(9_001))
    }

    #[test]
    fn slot_and_seal_are_domain_separated_and_deterministic() {
        let transfer = TransferId::from_bytes([0x11; 16]);
        let other_transfer = TransferId::from_bytes([0x12; 16]);
        let secret = [0x22; 32];
        assert_eq!(receipt_slot(transfer), receipt_slot(transfer));
        assert_ne!(receipt_slot(transfer), receipt_slot(other_transfer));

        let sealed = seal_receipt(transfer, &secret, &payload()).unwrap();
        assert_eq!(open_receipt(transfer, &secret, &sealed).unwrap(), payload());
        assert_eq!(
            open_receipt(other_transfer, &secret, &sealed),
            Err(MailboxProtocolError::AuthenticationFailed)
        );
    }

    #[test]
    fn tamper_and_wrong_secret_fail_authentication() {
        let transfer = TransferId::from_bytes([0x31; 16]);
        let secret = [0x32; 32];
        let sealed = seal_receipt(transfer, &secret, &payload()).unwrap();

        for index in 0..sealed.as_bytes().len() {
            let mut tampered = sealed.as_bytes().to_vec();
            tampered[index] ^= 1;
            let tampered = SealedReceipt::from_bytes(tampered).unwrap();
            assert_eq!(
                open_receipt(transfer, &secret, &tampered),
                Err(MailboxProtocolError::AuthenticationFailed)
            );
        }
        assert_eq!(
            open_receipt(transfer, &[0x33; 32], &sealed),
            Err(MailboxProtocolError::AuthenticationFailed)
        );
    }

    #[test]
    fn payload_binds_hash_and_size() {
        let encoded = payload().encode();
        assert_eq!(ReceiptPayload::decode(&encoded).unwrap(), payload());

        let mut wrong_schema = encoded;
        wrong_schema[8] ^= 1;
        assert_eq!(
            ReceiptPayload::decode(&wrong_schema),
            Err(MailboxProtocolError::InvalidPayload)
        );
    }

    #[test]
    fn reseal_is_byte_identical_so_the_zero_nonce_is_never_reused() {
        // The payload has no mutable field, so a receiver can seal only one
        // canonical blob per transfer. Re-sealing is byte-identical, hence the
        // fixed zero nonce is never reused across distinct plaintexts.
        let transfer = TransferId::from_bytes([0x61; 16]);
        let secret = [0x62; 32];
        let first = seal_receipt(transfer, &secret, &payload()).unwrap();
        let second = seal_receipt(transfer, &secret, &payload()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn receipt_debug_output_is_opaque() {
        let transfer = TransferId::from_bytes([0x51; 16]);
        let slot = receipt_slot(transfer);
        let sealed = seal_receipt(transfer, &[0x52; 32], &payload()).unwrap();
        assert_eq!(format!("{:?}", payload()), "ReceiptPayload([redacted])");
        assert_eq!(format!("{slot:?}"), "ReceiptSlot([opaque])");
        assert_eq!(format!("{sealed:?}"), "SealedReceipt([opaque])");
    }
}
