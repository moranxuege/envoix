//! The sealed bundle.
//!
//! Given the SPAKE2 shared key `K`, derive a one-shot AEAD key with the BLAKE3
//! KDF and seal a payload with ChaCha20-Poly1305. An attacker who cannot derive
//! `K` (no pairing code) can neither read nor forge a bundle. The `aad`
//! (associated data) binds each bundle to a context the caller chooses - the
//! sender's role - so a relay that only sees ciphertext cannot reflect one
//! peer's sealed payload back as the other's: opening with a different `aad`
//! fails the AEAD tag check.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::PairingError;

/// BLAKE3 KDF context separating this key from any other use of `K`.
const BUNDLE_KEY_CONTEXT: &str = "envoix-pairing bundle key v1";

/// ChaCha20-Poly1305 nonce length.
const NONCE_LEN: usize = 12;

/// Derive the one-shot AEAD key from the SPAKE2 shared key `k` (BLAKE3 KDF).
fn bundle_key(k: &[u8]) -> Key {
    Key::from(blake3::derive_key(BUNDLE_KEY_CONTEXT, k))
}

/// Seal `plaintext` under a key derived from `k`, bound to `aad`. The output is
/// `nonce(12) || ciphertext+tag`, safe to send over the cleartext mailbox; it
/// opens only with the same `k` and `aad`.
pub fn seal(k: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, PairingError> {
    let cipher = ChaCha20Poly1305::new(&bundle_key(k));
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| PairingError::Entropy)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| PairingError::Decrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open a bundle produced by [`seal`] with the same `k` and `aad`. Fails if `k`
/// or `aad` is wrong, or the bytes were tampered with.
pub fn open(k: &[u8], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, PairingError> {
    if sealed.len() < NONCE_LEN {
        return Err(PairingError::Malformed);
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(&bundle_key(k));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| PairingError::Decrypt)
}

/// Seal a serializable `value` (JSON) under `k`, bound to `aad`.
pub fn seal_json<T: Serialize>(k: &[u8], aad: &[u8], value: &T) -> Result<Vec<u8>, PairingError> {
    let json = serde_json::to_vec(value).map_err(|e| PairingError::BadJson(e.to_string()))?;
    seal(k, aad, &json)
}

/// Open a value sealed by [`seal_json`] with the same `k` and `aad`.
pub fn open_json<T: DeserializeOwned>(
    k: &[u8],
    aad: &[u8],
    sealed: &[u8],
) -> Result<T, PairingError> {
    let json = open(k, aad, sealed)?;
    serde_json::from_slice(&json).map_err(|e| PairingError::BadJson(e.to_string()))
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
