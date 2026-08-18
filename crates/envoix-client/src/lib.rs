//! Public application-facing facade for canonical Manifest v2 transfers.

pub mod api;
pub mod command;
pub mod configuration;
pub mod event;
pub mod model;
pub mod ports;
pub mod product;
mod reducers;
pub mod runtime;
pub mod snapshot;

pub use configuration::{DEFAULT_RELAY_URL, DEFAULT_RENDEZVOUS_BROKER};
pub use envoix_auth::SPAKE2_EXPERIMENTAL_WARNING;
pub use envoix_protocol::PeerDescriptor;
pub use envoix_session::{BindAddrs, EndpointAddr, IdentityConfig, MemoryIdentity};
pub use envoix_session::{TransferCancelToken, TransferDirection};
pub use envoix_types::PROTOCOL_VERSION;

/// Version of the typed v0.3 application command/event/snapshot contract.
pub const APPLICATION_CONTRACT_VERSION: u16 = 1;
