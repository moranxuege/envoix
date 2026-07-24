//! Pure SPAKE2 pairing, confirmation, and sealed-descriptor protocol.

#![forbid(unsafe_code)]

mod bundle;
mod error;
mod handshake;
mod message;
mod random;
mod secret;

pub mod identifiers;

pub use bundle::{DescriptorPayload, PeerDescriptor};
pub use error::PairingError;
pub use handshake::{
    InitiatorAwaitResponse, InitiatorConfirming, Paired, ResponderAwaitConfirm, Role,
    initiator_start, responder_respond,
};
pub use message::{
    Confirmation, MAX_MESSAGE_BODY, MessageKind, PairingMessage, PakeResponse, PakeStart,
    SealedDescriptor, WIRE_HEADER_LEN, decode_message, encode_message,
};
pub use random::{EntropyError, EntropySource, SystemEntropy};
pub use secret::{DataPlaneToken, MailboxSecret, PairingCode};

#[cfg(test)]
mod tests;
