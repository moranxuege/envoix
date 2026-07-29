//! Whether this endpoint MINTED the room or JOINED one.
//!
//! The other half of the 2x2. `TransferRecord::direction` says which side
//! sends; this says who created the room, and the two are independent — every
//! combination is reachable, which is what `MintRoom`/`JoinRoom` made true.
//!
//! It is durable rather than derived because it has an observable consequence:
//! the read projection publishes an invite for a card with a pairing channel,
//! and a JOINED card holds the channel it adopted. Without this fact a joiner
//! republishes the room secret it was given as though it had minted it — an
//! invite names a one-peer rendezvous, so a third party acting on the
//! republished one races the two who were already pairing.

use envoix_invite::Role;
use envoix_types::Direction;
use serde::{Deserialize, Serialize};

/// How this endpoint came to be in its room.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomParticipation {
    /// This endpoint created the room and published its invite.
    Minted,
    /// This endpoint adopted an invite someone else published.
    Joined,
}

impl RoomParticipation {
    /// Whether this endpoint may PUBLISH the channel's invite.
    ///
    /// Only a minter. Named here rather than at the projection so "may I share
    /// this?" has one answer instead of a condition each caller re-derives from
    /// whatever it happens to hold.
    pub const fn publishes_the_invite(self) -> bool {
        matches!(self, Self::Minted)
    }

    /// Whether `local` is the direction this participation implies, given the
    /// role the invite DECLARES.
    ///
    /// A minter is on the side its own invite declares; a joiner takes the
    /// opposite. Anything else is a record disagreeing with its own channel,
    /// which the decoder refuses rather than making live.
    pub fn agrees(self, local: Direction, creator: Role) -> bool {
        let creator_direction = match creator {
            Role::Send => Direction::Send,
            Role::Receive => Direction::Receive,
        };
        match self {
            Self::Minted => local == creator_direction,
            Self::Joined => local != creator_direction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this fact is durable. A joined card holds the channel
    /// it ADOPTED, so publishing on channel presence alone made a joiner
    /// republish the secret it was given — and an invite names a one-peer
    /// rendezvous, so a third party acting on the republished one races the two
    /// already pairing.
    #[test]
    fn only_a_minter_publishes_its_invite() {
        assert!(RoomParticipation::Minted.publishes_the_invite());
        assert!(!RoomParticipation::Joined.publishes_the_invite());
    }

    /// Participation and direction are independent, and the invite's declared
    /// role is what ties them: a minter is on the side its own invite declares,
    /// a joiner takes the opposite. All four cells, and the four that disagree
    /// with their own channel.
    #[test]
    fn each_participation_implies_its_direction() {
        for (participation, creator, local) in [
            (RoomParticipation::Minted, Role::Send, Direction::Send),
            (RoomParticipation::Minted, Role::Receive, Direction::Receive),
            (RoomParticipation::Joined, Role::Send, Direction::Receive),
            (RoomParticipation::Joined, Role::Receive, Direction::Send),
        ] {
            assert!(
                participation.agrees(local, creator),
                "{participation:?} of a {creator:?} invite is {local:?}"
            );
            let contradiction = match local {
                Direction::Send => Direction::Receive,
                Direction::Receive => Direction::Send,
            };
            assert!(
                !participation.agrees(contradiction, creator),
                "{participation:?} cannot also be {contradiction:?}"
            );
        }
    }
}
