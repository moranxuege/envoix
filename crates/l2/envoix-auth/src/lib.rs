//! Sans-io, channel-bound data-plane authentication.

#![forbid(unsafe_code)]

pub mod identifiers;

mod error;
mod handshake;
mod message;
mod random;

pub use error::{AuthCodecError, AuthError, AuthField};
pub use handshake::{
    Authenticated, Deadline, ExportedKeyingMaterial, MonotonicMillis, PeerRole,
    ReceiverAwaitConfirm, ReceiverAwaitStart, SenderAwaitConfirm, SenderAwaitResponse,
    receiver_wait, sender_start,
};
pub use message::{
    AUTH_WIRE_ID, AuthMessage, AuthMessageKind, Confirmation, MAX_AUTH_PAYLOAD, NONCE_SIZE,
    RESPONSE_MESSAGE_SIZE, Response, START_MESSAGE_SIZE, Start, decode_auth_message,
    encode_auth_message,
};

#[cfg(test)]
mod tests;
