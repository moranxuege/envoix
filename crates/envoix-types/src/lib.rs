//! Shared domain types.

use std::fmt;

/// Wire protocol version used by the v0 walking skeleton.
pub const PROTOCOL_VERSION: u32 = 0;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
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

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FileId(pub String);

impl FileId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ChunkId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ChunkSize(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ByteCount(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TransferDirection {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConnectionMode {
    TcpIpv6,
    QuicDirect,
    Relay,
    ServerFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PeerRole {
    Sender,
    Receiver,
}
