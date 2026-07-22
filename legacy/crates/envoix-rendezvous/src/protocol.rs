//! The small control protocol between a peer and the broker: a peer sends
//! [`Join`], the broker replies with [`Paired`]. Everything after that is the
//! opaque end-to-end pairing traffic the broker relays without parsing.

use serde::{Deserialize, Serialize};

/// SPAKE2 role assigned by the broker, decided by join order (first = initiator).
/// The handshake role (who speaks first in SPAKE2), orthogonal to the transfer
/// direction (`envoix_types::PeerRole`, who sends the file): a sender may be
/// either the initiator or the responder, depending on who joined the room first.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Initiator,
    Responder,
}

/// A peer's opening message: which room it wants to pair in.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Join {
    pub room_id: String,
}

/// The broker's reply once a partner is present: the peer's assigned role.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Paired {
    pub role: Role,
}

/// The broker's reply to a [`Join`]: either a partner arrived (pairing traffic
/// follows) or the room's wait window elapsed with no partner.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Reply {
    /// A partner is present; here is the peer's assigned role.
    Paired(Paired),
    /// No partner joined within the room's TTL; the broker is closing.
    Expired,
}
