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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash, Serialize)]
pub enum PeerRole {
    Sender,
    Receiver,
}
