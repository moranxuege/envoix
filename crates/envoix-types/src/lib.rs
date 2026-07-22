//! Shared domain types.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Wire protocol version used by the resumable transfer flow.
pub const PROTOCOL_VERSION: u32 = 1;

/// Minimum byte length for a SPAKE2 shared pairing token.
pub const MIN_SHARED_TOKEN_LEN: usize = 12;

/// Returns whether a SPAKE2 shared pairing token satisfies the shared policy.
pub fn is_valid_shared_token(token: &str) -> bool {
    token.is_ascii() && token.len() >= MIN_SHARED_TOKEN_LEN
}

/// The network path a transfer's connection is currently using.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DataPath {
    /// A direct (possibly hole-punched) UDP path to the peer.
    Direct {
        /// The peer's remote socket address.
        addr: std::net::SocketAddr,
    },
    /// Forwarded through a relay server.
    Relay {
        /// The relay's URL.
        url: String,
    },
    /// A transport this build cannot classify.
    Other {
        /// Debug description of the transport address.
        description: String,
    },
}

impl fmt::Display for DataPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct { addr } => write!(formatter, "direct ({addr})"),
            Self::Relay { url } => write!(formatter, "relay ({url})"),
            Self::Other { description } => formatter.write_str(description),
        }
    }
}

/// Progress of a rendezvous-room pairing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStep {
    /// Joining the room at the broker (parks until a partner arrives).
    Joining,
    /// The broker matched us with a partner; key exchange starting.
    Matched,
    /// SPAKE2 completed; a 6-digit SAS is displayed for user comparison before
    /// descriptors are exchanged. Both devices must show the same code.
    Confirming,
    /// SPAKE2 completed and descriptors were exchanged.
    Exchanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub struct TransferId(pub String);

impl TransferId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub struct FileId(pub String);

impl FileId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub struct ChunkId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub struct ChunkSize(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub struct ByteCount(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub enum TransferDirection {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub enum ConnectionMode {
    QuicDirect,
    Relay,
    ServerFallback,
}

/// Which end of a transfer a peer is - the data-plane transfer direction,
/// wire-encoded in the protocol. Mirrored at the invite layer by
/// `envoix_client::api::Role` (`Send`/`Receive`); orthogonal to the broker's
/// SPAKE2 handshake role (`envoix_rendezvous::Role`, `Initiator`/`Responder`),
/// which is decided by join order, not by who sends the file. The three look
/// alike but are deliberately separate - do not merge them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub enum PeerRole {
    Sender,
    Receiver,
}
