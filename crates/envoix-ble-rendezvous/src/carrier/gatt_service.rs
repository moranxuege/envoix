//! BLE GATT service and characteristic UUIDs for the Envoix rendezvous carrier.
//!
//! Uses a custom service UUID derived from the envoix namespace. Characteristics
//! are sized for the envelope exchange (max 512-byte MTU-friendly payloads).

/// Base UUID for the Envoix BLE rendezvous service, derived as
/// `SHA-256("envoix-ble-rendezvous")[..4]` as the vendor-assigned portion.
///
/// Full 128-bit UUID: `7e4e76e6-0001-1000-8000-00805f9b34fb`
pub const RENDEZVOUS_SERVICE_UUID: &str = "7e4e76e6-0001-1000-8000-00805f9b34fb";

// ---------------------------------------------------------------------------
// Characteristic UUIDs (16-bit suffixes under the base)
// ---------------------------------------------------------------------------

/// Ephemeral public key exchange (write / notify, 32 bytes).
/// - Initiator writes its 32-byte X25519 public key here.
/// - Responder notifies with its 32-byte X25519 public key.
pub const EPHEMERAL_PUBLIC_KEY_CHAR: &str = "7e4e76e6-0002-1000-8000-00805f9b34fb";

/// SAS confirmation MAC (write / notify, 32 bytes).
/// - After user confirms the 6-digit codes match, each side writes its
///   keyed-BLAKE3 confirmation MAC here.
pub const SAS_CONFIRM_CHAR: &str = "7e4e76e6-0003-1000-8000-00805f9b34fb";

/// Invitation envelope data (write / notify, up to 512 bytes per fragment).
/// Carries the authenticated `envoix://pair/` invitation or encrypted exchange
/// data after the SAS handshake completes.
pub const ENVELOPE_DATA_CHAR: &str = "7e4e76e6-0004-1000-8000-00805f9b34fb";

/// Carrier control point (write with response, 1 byte).
/// Used for flow control: fragment acknowledgment, abort, etc.
pub const CONTROL_POINT_CHAR: &str = "7e4e76e6-0005-1000-8000-00805f9b34fb";

// ---------------------------------------------------------------------------
// Control point commands
// ---------------------------------------------------------------------------

/// Fragment received OK — ready for next fragment.
pub const CP_ACK: u8 = 0x01;
/// Abort the current exchange with an error reason.
pub const CP_ABORT: u8 = 0x02;
/// Ready for envelope exchange (after SAS confirmation succeeds).
pub const CP_READY: u8 = 0x03;

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

/// Maximum BLE ATT MTU we request.
pub const REQUESTED_MTU: u16 = 512;

/// Maximum envelope fragment payload size (MTU minus 3-byte ATT header).
pub const MAX_FRAGMENT_SIZE: u16 = REQUESTED_MTU - 3;

/// Maximum total envelope size (256 KiB).
pub const MAX_ENVELOPE_SIZE: u32 = 256 * 1024;

/// Advertisement interval for presence (milliseconds).
pub const ADVERTISEMENT_INTERVAL_MS: u32 = 200;

/// Temporary presence identifier rotation interval (seconds).
pub const PRESENCE_ROTATION_SECS: u64 = 120;

/// Connection timeout (seconds).
pub const CONNECTION_TIMEOUT_SECS: u64 = 30;

/// Maximum number of fragments per envelope.
pub const MAX_FRAGMENTS: u16 = 512;

/// Replay cache size (number of confirmed MACs to remember).
pub const REPLAY_CACHE_SIZE: usize = 64;
