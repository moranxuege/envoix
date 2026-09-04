//! Deployment defaults and optional transport-only runtime configuration.

use std::fs;
use std::path::Path;

use envoix_error::CoreError;
use envoix_session::{MAX_DATA_STREAM_WINDOW, MIN_DATA_STREAM_WINDOW};
use serde::Deserialize;

/// Deployed defaults shared by native apps, the CLI Agent, and FFI adapters.
/// A configured relay still allows iroh to select a direct LAN path.
pub const DEFAULT_RENDEZVOUS_BROKER: &str =
    "6de87065a13b786177e37cd039ad8ff2b32ac9a78fb8f248ac919a9fcbe67b92@47.237.15.48:8445";
pub const DEFAULT_RELAY_URL: &str = "https://relay.envoix.cc:8444";

/// Optional transport tuning loaded by the compatibility client.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfig {
    pub(crate) data_stream_window: Option<String>,
    pub(crate) candidates: Option<CandidatesConfig>,
    pub(crate) rendezvous_pairing_attempts: Option<usize>,
    pub(crate) rendezvous_server_retries: Option<usize>,
    pub(crate) rendezvous_max_retry_after_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidatesConfig {
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    #[serde(default)]
    pub(crate) deny: Vec<String>,
}

impl RuntimeConfig {
    pub(crate) fn read(path: &Path) -> Result<Self, CoreError> {
        let text = fs::read_to_string(path).map_err(|error| {
            CoreError::InvalidInput(format!("failed to read runtime config: {error}"))
        })?;
        toml::from_str(&text)
            .map_err(|error| CoreError::InvalidInput(format!("invalid runtime config: {error}")))
    }
}

fn parse_size_bytes(value: &str, label: &str) -> Result<usize, CoreError> {
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

pub(crate) fn parse_window(value: &str) -> Result<u32, CoreError> {
    let bytes = u32::try_from(parse_size_bytes(value, "window")?).unwrap_or(u32::MAX);
    if !(MIN_DATA_STREAM_WINDOW..=MAX_DATA_STREAM_WINDOW).contains(&bytes) {
        return Err(CoreError::InvalidInput(format!(
            "window {value:?} must be between {MIN_DATA_STREAM_WINDOW} and {MAX_DATA_STREAM_WINDOW} bytes"
        )));
    }
    Ok(bytes)
}
