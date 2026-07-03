//! The unified client API (new surface, being built alongside the legacy
//! methods; see `docs/design/client-api.md`).
//!
//! One entry point per operation: a transfer is described by *what* to move,
//! *who* to move it with ([`PeerSource`]), and *how* to connect
//! ([`TransferOptions`]); it is observed through one event stream
//! ([`TransferEvent`]) and controlled through a handle.
//!
//! Binding-friendly by construction: no generics, closures, or lifetimes in
//! public signatures, so the surface can be exposed through UniFFI later.

mod event;
mod options;
mod source;

pub use event::TransferEvent;
pub use options::{PathPolicy, TransferOptions};
pub use source::PeerSource;
