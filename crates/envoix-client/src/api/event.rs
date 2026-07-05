//! The unified transfer event stream.
//!
//! One stream tells the whole story of a transfer; anything a UI might show
//! must be an event here, never a log line. Every variant is emitted by the
//! current implementation - no speculative vocabulary.

use envoix_protocol::PeerDescriptor;
use envoix_session::TransferDirection;
use envoix_types::{DataPath, TransferId};
use serde::Serialize;

use super::TransferMode;

/// A lifecycle event plus when it was emitted.
///
/// Stamped at emission (not receipt), so a slow consumer cannot skew the
/// timeline; serializes flat: `{"ts_ms": ..., "event": ..., ...fields}`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StampedEvent {
    /// Emission time, milliseconds since the Unix epoch.
    pub ts_ms: u64,
    /// The event itself.
    #[serde(flatten)]
    pub event: TransferEvent,
}

/// One step in the life of a transfer, in emission order.
///
/// Serializes as one JSON object per event with an `"event"` tag
/// (the CLI's `--json` output; stable shape for tooling).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TransferEvent {
    /// The local endpoint is being set up.
    Binding {
        /// Direction of this local operation.
        direction: TransferDirection,
        /// The rendezvous mode in use.
        mode: TransferMode,
    },
    /// We are listening; share these with the peer so it can dial us.
    Advertised {
        /// Our descriptor for the peer to dial.
        peer: PeerDescriptor,
        /// The pairing token in use, when one should be displayed.
        token: Option<String>,
        /// An encoded invite (QR payload), when this source produces one.
        invite: Option<String>,
    },
    /// Rendezvous pairing through the broker is running.
    Pairing,
    /// Establishing the peer connection (dialing, or accepting after a room
    /// pairing).
    Connecting,
    /// A data path to the peer was selected.
    Connected {
        /// The selected path (direct or relay).
        path: DataPath,
    },
    /// The selected data path changed (e.g. a relay -> direct upgrade once
    /// hole-punching succeeds).
    PathChanged {
        /// The newly selected path.
        path: DataPath,
    },
    /// The data transfer has started.
    Started {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name being transferred.
        file_name: String,
        /// Total expected plaintext bytes.
        total_bytes: u64,
        /// Plaintext bytes already present before this attempt started.
        bytes_resumed: u64,
    },
    /// More plaintext bytes have been sent or persisted.
    Progress {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Plaintext bytes transferred so far.
        bytes_transferred: u64,
        /// Total expected plaintext bytes.
        total_bytes: u64,
    },
    /// A hash verification phase has started.
    Verifying {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name being verified.
        file_name: String,
        /// Number of plaintext bytes being hashed.
        bytes_to_hash: u64,
    },
    /// A hash verification phase completed.
    Verified {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name that was verified.
        file_name: String,
        /// Number of plaintext bytes hashed.
        bytes_hashed: u64,
    },
    /// Transfer completed and, on receive, the file was finalized.
    Completed {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Plaintext bytes transferred in total.
        bytes_transferred: u64,
    },
    /// The transfer failed; the operation's result carries the same error.
    Failed {
        /// Direction of this local operation.
        direction: TransferDirection,
        /// Human-readable failure reason.
        reason: String,
    },
}
