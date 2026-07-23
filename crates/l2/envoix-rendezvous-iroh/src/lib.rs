//! iroh adaptation for the blind rendezvous mechanism.

#![forbid(unsafe_code)]

mod config;
mod error;
mod transport;

pub use config::{ConfigError, ConfigField, EndpointConfig, IrohClientConfig, IrohServerConfig};
pub use error::{IrohOperation, IrohRendezvousError, IrohWait};
pub use transport::{BrokerSession, bind_endpoint, endpoint_addr, join_room, serve_endpoint};

#[cfg(test)]
mod tests;
