//! Versioned, transport-neutral room invites.

#![forbid(unsafe_code)]

mod code;
mod error;
mod invite;

pub mod identifiers;

pub use code::{EntropyError, EntropySource, NamespacedRoomKey, RoomCode, generate_room_code};
pub use error::{InviteError, InviteField, RecognizedInvalid};
pub use invite::{Invite, Role, encode_deep_link, encode_qr, route_invite};

#[cfg(test)]
mod tests;
