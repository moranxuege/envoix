//! Length-prefixed framing for pairing messages, shared by both peers so they
//! serialize a message identically. Each frame is a 4-byte big-endian body
//! length followed by the JSON body. The caller performs the actual stream I/O
//! (read the length, read that many bytes, then `unframe`).

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::PairingError;

/// Upper bound on a single frame body. Pairing messages are tiny (a SPAKE2
/// message + nonce, a MAC, or a small sealed descriptor); this just bounds a
/// read.
pub const MAX_FRAME_BODY: usize = 64 * 1024;

/// Encode `value` as `len(4, big-endian) || json` for the wire.
pub fn frame<T: Serialize>(value: &T) -> Result<Vec<u8>, PairingError> {
    let body = serde_json::to_vec(value).map_err(|e| PairingError::BadJson(e.to_string()))?;
    if body.len() > MAX_FRAME_BODY {
        return Err(PairingError::BadMessage("frame body too large".into()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a frame `body` (the bytes after the length prefix) into `T`.
pub fn unframe<T: DeserializeOwned>(body: &[u8]) -> Result<T, PairingError> {
    serde_json::from_slice(body).map_err(|e| PairingError::BadJson(e.to_string()))
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
