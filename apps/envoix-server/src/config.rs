use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use envoix_rendezvous::{ControlLimits, RegistryConfig};
use envoix_rendezvous_iroh::IrohServerConfig;
use iroh::RelayUrl;

use crate::ServerError;

pub const DEFAULT_BIND: &str = "0.0.0.0:9445";
pub const DEFAULT_NODE_KEY_PATH: &str = "rendezvous-node.key";
pub const DEFAULT_ROOM_TTL_SECS: u64 = 300;
pub const DEFAULT_RELAY_TTL_SECS: u64 = 120;
pub const DEFAULT_JOIN_DEADLINE_SECS: u64 = 10;
pub const DEFAULT_CLOSE_GRACE_SECS: u64 = 10;
pub const DEFAULT_MAX_ROOM_KEY_LENGTH: usize = 64;
pub const DEFAULT_MAX_WAITING_ROOMS: usize = 4096;
pub const DEFAULT_MAX_CONNECTIONS: usize = 256;
pub const DEFAULT_HANDSHAKE_DEADLINE_SECS: u64 = 10;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub node_key_path: PathBuf,
    pub relay: Option<RelayUrl>,
    pub room_ttl: Duration,
    pub relay_ttl: Duration,
    pub join_deadline: Duration,
    pub close_grace: Duration,
    pub max_room_key_length: usize,
    pub max_waiting_rooms: usize,
    pub max_connections: usize,
    pub handshake_deadline: Duration,
    pub bind_deadline: Duration,
}

impl ServerConfig {
    pub fn operational_defaults() -> Self {
        Self {
            bind: DEFAULT_BIND.parse().expect("DEFAULT_BIND must be valid"),
            node_key_path: PathBuf::from(DEFAULT_NODE_KEY_PATH),
            relay: None,
            room_ttl: Duration::from_secs(DEFAULT_ROOM_TTL_SECS),
            relay_ttl: Duration::from_secs(DEFAULT_RELAY_TTL_SECS),
            join_deadline: Duration::from_secs(DEFAULT_JOIN_DEADLINE_SECS),
            close_grace: Duration::from_secs(DEFAULT_CLOSE_GRACE_SECS),
            max_room_key_length: DEFAULT_MAX_ROOM_KEY_LENGTH,
            max_waiting_rooms: DEFAULT_MAX_WAITING_ROOMS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            handshake_deadline: Duration::from_secs(DEFAULT_HANDSHAKE_DEADLINE_SECS),
            bind_deadline: Duration::from_secs(DEFAULT_HANDSHAKE_DEADLINE_SECS),
        }
    }

    pub(crate) fn mechanism_configs(
        &self,
    ) -> Result<(RegistryConfig, IrohServerConfig), ServerError> {
        let limits =
            ControlLimits::new(self.max_room_key_length).map_err(ServerError::RendezvousConfig)?;
        let registry = RegistryConfig::new(
            self.room_ttl,
            self.relay_ttl,
            self.join_deadline,
            self.close_grace,
            limits,
            self.max_waiting_rooms,
        )
        .map_err(ServerError::RendezvousConfig)?;
        let adapter = IrohServerConfig::new(self.handshake_deadline, self.max_connections)
            .map_err(ServerError::IrohConfig)?;
        Ok((registry, adapter))
    }
}
