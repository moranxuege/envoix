//! QR-based pairing invite payload - serialization, encoding, and validation.
//!
//! Invite strings have the form `envoix:<base64url>` where the base64url payload
//! is a JSON-encoded [`QrInvitePayload`].  The `envoix:` prefix makes the string
//! recognisable and leaves room for future format versions.
//!
//! # Security
//!
//! The invite payload is **unauthenticated and unencrypted**.  It contains the
//! plaintext SPAKE2 token, which must be treated like a password: share it only
//! over a trusted channel (e.g. scan the QR from the same screen, or paste it
//! over an already-secure session).  Anyone who obtains the invite string before
//! it expires can impersonate the receiver.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use envoix_protocol::PeerDescriptor;
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use qrcode::QrCode;
use qrcode::types::Color;
use serde::{Deserialize, Serialize};

use envoix_types::{MIN_SHARED_TOKEN_LEN, PROTOCOL_VERSION, is_valid_shared_token};

/// Prefix prepended to every encoded invite string.
pub const INVITE_PREFIX: &str = "envoix:";

/// Current payload schema version.  Increment when the schema changes in a
/// backward-incompatible way.
pub const PAYLOAD_VERSION: u32 = 2;

/// Versioned invite payload carried inside a QR code or pasted as plain text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QrInvitePayload {
    /// Payload schema version - must equal [`PAYLOAD_VERSION`].
    pub version: u32,
    /// Wire protocol version the receiver is running.
    pub protocol_version: u32,
    /// SPAKE2 shared token (at least MIN_SHARED_TOKEN_LEN ASCII bytes).
    pub token: String,
    /// Direct iroh endpoint descriptor the sender should dial.
    pub peer: PeerDescriptor,
    /// Optional relay home URLs for the endpoint. Older clients ignore this
    /// field and keep dialing direct addresses; newer clients combine it with
    /// `peer` into a full iroh endpoint address for relay fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_urls: Vec<String>,
    /// Expiry as a Unix timestamp in seconds.  Senders reject payloads where
    /// `expires_at <= now`.
    pub expires_at: u64,
    /// Reserved feature flags - set to 0 for this version.
    pub flags: u32,
}

/// Errors returned by QR payload operations.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum QrError {
    #[error("unsupported payload schema version {found} (expected {expected})")]
    VersionMismatch { found: u32, expected: u32 },

    #[error("unsupported protocol version {found} (expected {expected})")]
    ProtocolVersionMismatch { found: u32, expected: u32 },

    #[error("invite has expired")]
    Expired,

    #[error("invite contains no direct peer addresses")]
    NoDirectAddresses,

    #[error("token is too short or not ASCII (minimum {MIN_SHARED_TOKEN_LEN} ASCII bytes)")]
    WeakToken,

    #[error("malformed endpoint id: {0}")]
    MalformedEndpointId(String),

    #[error("malformed relay url: {0}")]
    MalformedRelayUrl(String),

    #[error("decode error: {0}")]
    DecodeError(String),

    #[error("entropy source unavailable: {0}")]
    Entropy(String),

    #[error(
        "unsupported feature flags 0x{0:08x}; sender and receiver versions may be incompatible"
    )]
    UnsupportedFlags(u32),
}

impl QrInvitePayload {
    /// Encodes the payload into an invite string: `envoix:<base64url>`.
    ///
    /// Serialization is infallible for this struct (only primitives, `String`,
    /// and `Vec<String>`), so this does not return a `Result`.
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).expect("QrInvitePayload always serializes to JSON");
        let b64 = URL_SAFE_NO_PAD.encode(json.as_bytes());
        format!("{INVITE_PREFIX}{b64}")
    }

    /// Decodes an invite string produced by [`encode`](Self::encode).
    ///
    /// Returns [`QrError::DecodeError`] for any parse failure.  Call
    /// [`validate`](Self::validate) separately to check semantic constraints.
    pub fn decode(s: &str) -> Result<Self, QrError> {
        let b64 = s
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or_else(|| QrError::DecodeError(format!("missing '{INVITE_PREFIX}' prefix")))?;

        let bytes = URL_SAFE_NO_PAD
            .decode(b64)
            .map_err(|e| QrError::DecodeError(format!("base64 decode failed: {e}")))?;

        serde_json::from_slice(&bytes)
            .map_err(|e| QrError::DecodeError(format!("JSON parse failed: {e}")))
    }

    /// Validates semantic constraints on the payload.
    ///
    /// `now` is the current Unix timestamp in seconds.  Pass
    /// `std::time::SystemTime::now()` converted to seconds, or a fixed value
    /// in tests.
    pub fn validate(&self, now: u64) -> Result<(), QrError> {
        self.validate_versions()?;
        if self.expires_at <= now {
            return Err(QrError::Expired);
        }
        self.validate_body()
    }

    /// Validates a previously accepted invite for the continuation of that
    /// same transfer. Expiry prevents new pairing attempts; it must not destroy
    /// an established transfer's ability to resume after a long pause.
    pub fn validate_for_resume(&self) -> Result<(), QrError> {
        self.validate_versions()?;
        self.validate_body()
    }

    fn validate_versions(&self) -> Result<(), QrError> {
        if self.version != PAYLOAD_VERSION {
            return Err(QrError::VersionMismatch {
                found: self.version,
                expected: PAYLOAD_VERSION,
            });
        }

        if self.protocol_version != PROTOCOL_VERSION {
            return Err(QrError::ProtocolVersionMismatch {
                found: self.protocol_version,
                expected: PROTOCOL_VERSION,
            });
        }

        Ok(())
    }

    fn validate_body(&self) -> Result<(), QrError> {
        if self.peer.direct_addrs.is_empty() {
            return Err(QrError::NoDirectAddresses);
        }

        if !is_valid_shared_token(&self.token) {
            return Err(QrError::WeakToken);
        }

        if let Err(error) = self.peer.endpoint_id.parse::<EndpointId>() {
            return Err(QrError::MalformedEndpointId(error.to_string()));
        }

        for relay_url in &self.relay_urls {
            relay_url
                .parse::<RelayUrl>()
                .map_err(|error| QrError::MalformedRelayUrl(error.to_string()))?;
        }

        if self.flags != 0 {
            return Err(QrError::UnsupportedFlags(self.flags));
        }

        Ok(())
    }

    /// Returns the peer descriptor.
    pub fn peer_descriptor(&self) -> Result<PeerDescriptor, QrError> {
        if self.peer.direct_addrs.is_empty() {
            return Err(QrError::NoDirectAddresses);
        }
        Ok(self.peer.clone())
    }

    /// Returns the full iroh endpoint address described by the invite,
    /// including relay URLs when present.
    pub fn endpoint_addr(&self) -> Result<EndpointAddr, QrError> {
        let peer = self.peer_descriptor()?;
        let id = peer
            .endpoint_id
            .parse::<EndpointId>()
            .map_err(|error| QrError::MalformedEndpointId(error.to_string()))?;
        let direct = peer.direct_addrs.iter().copied().map(TransportAddr::Ip);
        let relays = self
            .relay_urls
            .iter()
            .map(|url| {
                url.parse::<RelayUrl>()
                    .map(TransportAddr::Relay)
                    .map_err(|error| QrError::MalformedRelayUrl(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EndpointAddr::from_parts(id, direct.chain(relays)))
    }

    /// Constructs a new payload with the current protocol version and schema
    /// version pre-filled.
    pub fn new(token: String, peer: PeerDescriptor, expires_at: u64) -> Self {
        Self::new_with_relay_urls(token, peer, Vec::new(), expires_at)
    }

    /// Constructs a new payload with optional relay home URLs.
    pub fn new_with_relay_urls(
        token: String,
        peer: PeerDescriptor,
        relay_urls: Vec<String>,
        expires_at: u64,
    ) -> Self {
        Self {
            version: PAYLOAD_VERSION,
            protocol_version: PROTOCOL_VERSION,
            token,
            peer,
            relay_urls,
            expires_at,
            flags: 0,
        }
    }
}

/// Number of random bytes used when generating a pairing token.
/// 16 bytes = 128 bits of entropy, well above the MIN_SHARED_TOKEN_LEN minimum.
const TOKEN_RANDOM_BYTES: usize = 16;

/// Generates a random pairing token as a lowercase hex string.
///
/// Returns [`QrError::Entropy`] only if the OS entropy source is unavailable.
pub fn generate_token() -> Result<String, QrError> {
    let mut bytes = [0u8; TOKEN_RANDOM_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| QrError::Entropy(e.to_string()))?;

    let mut token = String::with_capacity(TOKEN_RANDOM_BYTES * 2);
    for b in bytes {
        use std::fmt::Write as _;
        write!(token, "{b:02x}").expect("writing to String is infallible");
    }
    Ok(token)
}

/// Renders `data` as a QR code and returns a UTF-8 string suitable for
/// printing directly to a terminal.
///
/// Each pair of QR rows is collapsed into one line of text using Unicode
/// half-block characters (`▀` `▄` `█` ` `), so the output is roughly square
/// in a fixed-width font.  A four-module quiet zone is added on every side
/// per the QR Code specification, which requires this minimum for reliable
/// finder-pattern detection.
///
/// Returns `None` if `data` is too long to encode at any QR error-correction
/// level.
pub fn render_terminal_qr(data: &str) -> Option<String> {
    const QUIET: usize = 4;

    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.into_colors();
    let padded = width + QUIET * 2;

    // Dark module lookup that treats the quiet zone as light.
    let is_dark = |row: usize, col: usize| -> bool {
        if row < QUIET || col < QUIET || row >= width + QUIET || col >= width + QUIET {
            return false;
        }
        colors[(row - QUIET) * width + (col - QUIET)] == Color::Dark
    };

    // Render two QR rows per output line using half-block characters.
    let mut output = String::new();
    for row in (0..padded).step_by(2) {
        for col in 0..padded {
            let top = is_dark(row, col);
            let bot = row + 1 < padded && is_dark(row + 1, col);
            output.push(match (top, bot) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        output.push('\n');
    }

    Some(output)
}

#[cfg(test)]
mod tests;
