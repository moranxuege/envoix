//! Public application-facing facade for envoix clients.
//!
//! The API lives in [`api`]: build an [`api::Client`], start transfers with
//! [`api::Client::send`] / [`api::Client::receive`], or use the additive
//! [`api::Client::send_manifest`] / [`api::Client::receive_transfer`] surface
//! for multi-item transfers. Both are observed through the unified
//! [`api::TransferEvent`] stream.

pub mod api;

use std::fs;
use std::path::Path;

pub use envoix_auth::SPAKE2_EXPERIMENTAL_WARNING;
use envoix_error::CoreError;
pub use envoix_protocol::{
    ManifestEntryKind, ManifestEntryV1, ManifestHashAlgorithm, ManifestId, ManifestV1,
    PeerDescriptor,
};
pub use envoix_session::{
    BindAddrs, EndpointAddr, IdentityConfig, ManifestSendRequest, ManifestTransferSummary,
    MemoryIdentity, PairingConfig, PendingManifestV2Receive, ReceiverManifestV2SessionSummary,
    SenderManifestV2SessionSummary, SessionConfig, SessionError, SessionTransferSummary,
    TransferCancelToken, TransferDirection, TransferSummary,
};
pub use envoix_storage::TransferReceipt;
pub use envoix_types::PROTOCOL_VERSION;
// Chunk-size bounds + validation are a transfer-engine constraint; they live in
// envoix-transfer next to DEFAULT_CHUNK_SIZE and are reached through session.
// The flow-control window bounds live in envoix-session (a transport tuning).
use envoix_session::{MAX_DATA_STREAM_WINDOW, MIN_DATA_STREAM_WINDOW, validate_chunk_size};
use serde::Deserialize;

/// Environment variable overriding the runtime transfer chunk size.
pub const ENVOIX_CHUNK_SIZE: &str = "ENVOIX_CHUNK_SIZE";

/// Internal alias for the error type shared with the lower layers; the
/// public API surfaces [`api::TransferError`] instead.
pub(crate) type PublicError = CoreError;

/// The recognized contents of the optional TOML config file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfig {
    pub(crate) chunk_size: Option<String>,
    pub(crate) data_stream_window: Option<String>,
    pub(crate) candidates: Option<CandidatesConfig>,
}

/// `[candidates]` table: CIDR allow/deny lists scoping advertised addresses.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidatesConfig {
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    #[serde(default)]
    pub(crate) deny: Vec<String>,
}

impl RuntimeConfig {
    pub(crate) fn read(path: &Path) -> Result<Self, PublicError> {
        let text = fs::read_to_string(path).map_err(|error| {
            CoreError::InvalidInput(format!("failed to read config {}: {error}", path.display()))
        })?;
        toml::from_str(&text).map_err(|error| {
            CoreError::InvalidInput(format!("invalid config {}: {error}", path.display()))
        })
    }
}

/// Parse a human byte size like `64KB`, `16MB`, or `1048576B` into bytes.
/// `label` names the value in error messages (e.g. `"chunk size"`, `"window"`).
fn parse_size_bytes(value: &str, label: &str) -> Result<usize, PublicError> {
    let value = value.trim();
    let (number, unit) = if let Some(number) = value.strip_suffix("KB") {
        (number, 1024_usize)
    } else if let Some(number) = value.strip_suffix('K') {
        (number, 1024_usize)
    } else if let Some(number) = value.strip_suffix("MB") {
        (number, 1024_usize * 1024)
    } else if let Some(number) = value.strip_suffix('M') {
        (number, 1024_usize * 1024)
    } else if let Some(number) = value.strip_suffix('B') {
        (number, 1_usize)
    } else {
        return Err(CoreError::InvalidInput(format!(
            "{label} {value:?} must include B, K, KB, M, or MB"
        )));
    };

    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(CoreError::InvalidInput(format!(
            "invalid {label} {value:?}"
        )));
    }

    let count = number
        .parse::<usize>()
        .map_err(|error| CoreError::InvalidInput(format!("invalid {label} {value:?}: {error}")))?;
    count.checked_mul(unit).ok_or_else(|| {
        CoreError::InvalidInput(format!("{label} {value:?} exceeds supported range"))
    })
}

fn parse_chunk_size(value: &str) -> Result<usize, PublicError> {
    let bytes = parse_size_bytes(value, "chunk size")?;
    validate_chunk_size(bytes)?;
    Ok(bytes)
}

/// Parse a per-stream flow-control window (same human units as the chunk size),
/// requiring it to fall within the transport's accepted `[MIN, MAX]` bounds.
fn parse_window(value: &str) -> Result<u32, PublicError> {
    let bytes = parse_size_bytes(value, "window")?;
    let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
    if !(MIN_DATA_STREAM_WINDOW..=MAX_DATA_STREAM_WINDOW).contains(&bytes) {
        return Err(CoreError::InvalidInput(format!(
            "window {value:?} must be between {MIN_DATA_STREAM_WINDOW} and \
             {MAX_DATA_STREAM_WINDOW} bytes"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
