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
    Invite, MAX_BROKER_LENGTH, MAX_INVITE_INPUT_LENGTH, MAX_INVITE_LINK_LENGTH, MAX_RELAY_LENGTH,
    QrMatrix, Role, encode_deep_link, encode_qr, encode_qr_matrix, route_invite,
};

#[cfg(test)]
mod tests;
