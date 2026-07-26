//! Room rendezvous broker.
//!
//! Two peers connect to the broker and present the same room locator. The broker
//! matches an invitation creator to a joiner with complementary transfer roles,
//! tells each its fixed SPAKE2 role
//! ([`Role::Initiator`] / [`Role::Responder`]), then **blindly relays raw bytes**
//! between them. The end-to-end pairing (SPAKE2 + sealed peer descriptors, see
//! `envoix-pairing`) runs *through* this relay, so the broker never sees
//! plaintext and cannot forge or swap a descriptor - it is an untrusted mailbox.
//!
//! Transport-agnostic: a peer connection is a [`PeerConn`] over any
//! `AsyncRead`/`AsyncWrite` halves (iroh streams in production, an in-memory
//! duplex in tests).

mod broker;
mod io;
mod peer;
mod protocol;

pub use broker::RoomRegistry;
pub use envoix_invite::{BootstrapKind, InvitationSide, TransferRole};
pub use io::{read_framed, write_framed};
pub use peer::{CloseWaiter, PeerConn};
pub use protocol::{Join, Paired, RENDEZVOUS_PROTOCOL_VERSION, Reply, Role};

/// Errors from the rendezvous broker.
#[derive(Debug, thiserror::Error)]
pub enum RendezvousError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed control message: {0}")]
    BadMessage(String),
    #[error("control frame exceeds the size limit")]
    FrameTooLarge,
    #[error("pairing window expired before a partner joined the room")]
    Expired,
    #[error("join rejected: {0}")]
    Rejected(&'static str),
}
