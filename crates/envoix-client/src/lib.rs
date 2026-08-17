//! Public application-facing facade for canonical Manifest v2 transfers.

pub mod api;
pub mod product;

use std::fs;
use std::path::Path;

pub use envoix_auth::SPAKE2_EXPERIMENTAL_WARNING;
use envoix_error::CoreError;
pub use envoix_protocol::PeerDescriptor;
pub use envoix_session::{BindAddrs, EndpointAddr, IdentityConfig, MemoryIdentity};
use envoix_session::{MAX_DATA_STREAM_WINDOW, MIN_DATA_STREAM_WINDOW};
pub use envoix_session::{TransferCancelToken, TransferDirection};
pub use envoix_types::PROTOCOL_VERSION;
use serde::Deserialize;

/// Deployed defaults shared by the native apps, CLI agent, and FFI facade.
/// A configured relay still allows iroh to select a direct LAN path.
pub const DEFAULT_RENDEZVOUS_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
pub const DEFAULT_RELAY_URL: &str = "https://envoix.chkxwlyh.us:8444";

type PublicError = CoreError;

/// Optional transport-only runtime configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfig {
    data_stream_window: Option<String>,
    candidates: Option<CandidatesConfig>,
    rendezvous_pairing_attempts: Option<usize>,
    rendezvous_server_retries: Option<usize>,
    rendezvous_max_retry_after_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatesConfig {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

impl RuntimeConfig {
    fn read(path: &Path) -> Result<Self, PublicError> {
        let text = fs::read_to_string(path).map_err(|error| {
            CoreError::InvalidInput(format!("failed to read runtime config: {error}"))
        })?;
        toml::from_str(&text)
            .map_err(|error| CoreError::InvalidInput(format!("invalid runtime config: {error}")))
    }
}

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

fn parse_window(value: &str) -> Result<u32, PublicError> {
    let bytes = u32::try_from(parse_size_bytes(value, "window")?).unwrap_or(u32::MAX);
    if !(MIN_DATA_STREAM_WINDOW..=MAX_DATA_STREAM_WINDOW).contains(&bytes) {
        return Err(CoreError::InvalidInput(format!(
            "window {value:?} must be between {MIN_DATA_STREAM_WINDOW} and {MAX_DATA_STREAM_WINDOW} bytes"
        )));
    }
    Ok(bytes)
}
