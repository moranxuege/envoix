//! Blind, transport-agnostic rendezvous over a versioned control dialect.
//!
//! Peers present a semi-public [`NamespacedRoomKey`]. The first peer is assigned
//! the initiator role, the second the responder role, and all bytes after the
//! paired replies are relayed without interpretation.

#![forbid(unsafe_code)]

mod client;
mod config;
mod error;
mod peer;
mod registry;
mod wire;

pub mod identifiers;

pub use client::join_room;
pub use config::{ClientConfig, ConfigError, ConfigField, ControlLimits, RegistryConfig};
pub use envoix_invite::NamespacedRoomKey;
pub use error::{IoOperation, RendezvousError, WaitKind};
pub use peer::{CloseWaiter, PeerConn};
pub use registry::RoomRegistry;
pub use wire::{
    CONTROL_HEADER_LEN, ControlError, ControlFrame, Join, Paired, RejectionReason, Reply, Role,
    decode_control, encode_control, read_control, write_control,
};

#[cfg(test)]
mod tests;
