//! Versioned, transport-neutral room invites.

#![forbid(unsafe_code)]

mod code;
mod error;
mod invite;

pub mod identifiers;

pub use code::{
    EntropyError, EntropySource, MAX_ROOM_CODE_LENGTH, NamespacedRoomKey, RoomCode,
    generate_room_code,
};
pub use error::{InviteError, InviteField, RecognizedInvalid};
pub use invite::{
    Invite, MAX_INVITE_INPUT_LENGTH, MAX_INVITE_LINK_LENGTH, Role, encode_deep_link, encode_qr,
    route_invite,
};

#[cfg(test)]
mod tests;
