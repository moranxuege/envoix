//! The unified transfer event stream.
//!
//! One stream tells the whole story of a transfer; anything a UI might show
//! must be an event here, never a log line. Every variant is emitted by the
//! current implementation - no speculative vocabulary.

use envoix_protocol::PeerDescriptor;
use envoix_session::TransferDirection;
use envoix_types::{DataPath, PairingStep, TransferId};
use serde::{Deserialize, Serialize};

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
    Pairing {
        /// Which pairing step was reached.
        step: PairingStep,
    },
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
    /// SEND only: every byte and the Complete frame are sent; awaiting the
    /// receiver's CompleteAck (the final round trip). A failure in this phase
    /// means the file very likely arrived - see `FailureCode::ConnectionLost`
    /// handling in the state machine.
    Confirming {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// BLAKE3 hash of the bytes actually sent (the `Complete` frame's
        /// hash) - the committed proof basis mailbox receipts are verified
        /// against.
        file_hash: String,
    },
    /// Transfer completed and, on receive, the file was finalized.
    Completed {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// File name that completed.
        file_name: String,
        /// Plaintext bytes transferred in total.
        bytes_transferred: u64,
    },
    /// The transfer failed; the operation's result carries the same error.
    Failed {
        /// Direction of this local operation.
        direction: TransferDirection,
        /// Human-readable failure reason.
        reason: String,
        /// Typed classification of the failure, so frontends branch on an enum
        /// instead of matching the prose in `reason`.
        reason_code: FailureCode,
    },
}

/// Typed classification of a transfer failure. The peer-reported codes ride the
/// same best-effort error frame as the message — a degraded path can drop them,
/// in which case the failure surfaces as [`ConnectionLost`](Self::ConnectionLost).
/// Frontends should treat these as a hint and keep durable facts (a partial on
/// disk) as the fallback signal for resumability.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// The local user cancelled the transfer.
    Cancelled,
    /// The local user paused the transfer (resumable intent).
    Paused,
    /// The peer reported cancelling the transfer.
    PeerCancelled,
    /// The peer reported pausing the transfer (resumable intent).
    PeerPaused,
    /// The connection dropped without a peer-reported reason.
    ConnectionLost,
    /// Any other failure; `reason` carries the detail.
    Other,
}

impl FailureCode {
    /// Classify a failure reason string. This is the ONE place the canonical
    /// interrupt messages (and the connection-drop phrasings the session layer
    /// produces) are matched — frontends must branch on the resulting enum,
    /// never on the prose.
    pub(crate) fn classify(reason: &str) -> Self {
        use envoix_session::{
            PEER_INTERRUPT_MESSAGE, PEER_PAUSE_MESSAGE, USER_INTERRUPT_MESSAGE, USER_PAUSE_MESSAGE,
        };
        match reason {
            r if r.contains(USER_PAUSE_MESSAGE) => Self::Paused,
            r if r.contains(USER_INTERRUPT_MESSAGE) => Self::Cancelled,
            r if r.contains(PEER_PAUSE_MESSAGE) => Self::PeerPaused,
            r if r.contains(PEER_INTERRUPT_MESSAGE) => Self::PeerCancelled,
            r if r.contains("connection lost") || r.contains("connection closed by peer") => {
                Self::ConnectionLost
            }
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FailureCode;

    #[test]
    fn classify_maps_canonical_messages_to_codes() {
        // The session layer prefixes/wraps these, so classify uses contains.
        assert_eq!(
            FailureCode::classify("transfer paused by user"),
            FailureCode::Paused
        );
        assert_eq!(
            FailureCode::classify("transfer interrupted by user"),
            FailureCode::Cancelled
        );
        assert_eq!(
            FailureCode::classify("transfer paused by peer"),
            FailureCode::PeerPaused
        );
        assert_eq!(
            FailureCode::classify("transfer interrupted by peer"),
            FailureCode::PeerCancelled
        );
        assert_eq!(
            FailureCode::classify("io error: connection lost"),
            FailureCode::ConnectionLost
        );
        assert_eq!(
            FailureCode::classify("connection closed by peer"),
            FailureCode::ConnectionLost
        );
        assert_eq!(FailureCode::classify("hash mismatch"), FailureCode::Other);
    }

    #[test]
    fn reason_code_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&FailureCode::PeerPaused).unwrap(),
            r#""peer_paused""#
        );
    }
}
