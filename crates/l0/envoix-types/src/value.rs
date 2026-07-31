use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{OfferedName, TransferId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

impl fmt::Display for Direction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Send => formatter.write_str("send"),
            Self::Receive => formatter.write_str("receive"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ByteCount(u64);

impl ByteCount {
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Which bytes, identified by their digest.
///
/// A VALUE, and here rather than in the protocol crate because that is what it
/// is. It names the content of a file the same way whether that file is on a
/// wire, in an app-private artifact, or being read through a provider — and a
/// digest that lived in the protocol vocabulary could not be spoken by anything
/// that stores bytes without also depending on how they are transmitted.
///
/// The protocol crate re-exports it, so every existing spelling still resolves.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A prefix of some bytes that has been promised durable, and which bytes it is.
///
/// L0 because it is a pair of L0 values with a meaning, and because two crates
/// on different sides need to speak it: the transfer engine, which promises a
/// prefix it has accepted, and the bulk store, which makes that promise durable.
/// Putting it in either one would make the other depend on it for a two-field
/// value type.
///
/// No chunk index. It is `length.div_ceil(chunk_size)` and the chunk size is
/// negotiated per transfer, so keeping it here would store a conclusion beside
/// its premise — and let the two disagree.
///
/// The digest is LOCAL evidence: it says the durable prefix is the one that was
/// promised, which is a different question from whether it matches what a remote
/// peer holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurablePrefix {
    pub length: ByteCount,
    pub digest: ContentHash,
}

/// What a peer says it is sending: a name and a byte count, and the transfer
/// they are about.
///
/// L0 for the same reason as [`DurablePrefix`]: the transfer machine reads it
/// off the wire and the product authority decides what to do with it, and
/// neither should depend on the other for three fields it already owns.
///
/// A CLAIM, never a measurement. A sender establishes its total by counting
/// bytes it read; a receiver is told. The absence of a digest here is that
/// difference made structural — there is nothing in this type a receiver could
/// mistake for proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerContentDeclaration {
    pub transfer: TransferId,
    pub offered_name: OfferedName,
    pub file_size: ByteCount,
}
