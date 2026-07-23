//! Minimal composition root for the blind Envoix rendezvous server.

#![forbid(unsafe_code)]

mod config;
mod error;
mod key;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use envoix_rendezvous::{RendezvousError, RoomRegistry};
use envoix_rendezvous_iroh::{
    EndpointConfig, IrohRendezvousError, bind_endpoint, endpoint_addr, serve_endpoint,
};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub use config::{
    DEFAULT_BIND, DEFAULT_CLOSE_GRACE_SECS, DEFAULT_HANDSHAKE_DEADLINE_SECS,
    DEFAULT_JOIN_DEADLINE_SECS, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_ROOM_KEY_LENGTH,
    DEFAULT_MAX_WAITING_ROOMS, DEFAULT_NODE_KEY_PATH, DEFAULT_RELAY_TTL_SECS,
    DEFAULT_ROOM_TTL_SECS, ServerConfig,
};
pub use error::{KeyError, KeyOperation, ServerError};

pub struct ServerHandle {
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
    requested_bind: SocketAddr,
    bound_addr: SocketAddr,
    shutdown_deadline: Duration,
    task: JoinHandle<Result<(), IrohRendezvousError>>,
}

impl ServerHandle {
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint_addr.id
    }

    pub const fn endpoint_addr(&self) -> &EndpointAddr {
        &self.endpoint_addr
    }

    pub const fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    pub fn connect_string(&self) -> String {
        if self.requested_bind.ip().is_unspecified() {
            format!(
                "{}@<this-host-ip>:{}",
                self.endpoint_id(),
                self.bound_addr.port()
            )
        } else {
            format!("{}@{}", self.endpoint_id(), self.bound_addr)
        }
    }

    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    pub async fn shutdown(mut self) -> Result<(), ServerError> {
        let endpoint = self.endpoint.clone();
        match timeout(self.shutdown_deadline, async {
            endpoint.close().await;
            (&mut self.task).await
        })
        .await
        {
            Ok(completed) => map_server_task(completed),
            Err(_) => {
                self.task.abort();
                let _ = self.task.await;
                Err(ServerError::ShutdownDeadline)
            }
        }
    }

    pub async fn wait_for_ctrl_c(mut self) -> Result<(), ServerError> {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(ServerError::Signal)?;
                self.shutdown().await
            }
            completed = &mut self.task => {
                timeout(self.shutdown_deadline, self.endpoint.close())
                    .await
                    .map_err(|_| ServerError::ShutdownDeadline)?;
                map_server_task(completed)
            }
        }
    }
}

pub async fn run(config: ServerConfig) -> Result<ServerHandle, ServerError> {
    let (registry_config, server_config) = config.mechanism_configs()?;
    let secret_key = key::load_or_create_node_key(&config.node_key_path)?;
    let endpoint_config =
        EndpointConfig::new(config.bind, config.relay, secret_key, config.bind_deadline)
            .map_err(ServerError::IrohConfig)?;
    let endpoint = bind_endpoint(endpoint_config).await?;
    let address = endpoint_addr(&endpoint);
    let bound_addr = endpoint
        .bound_sockets()
        .into_iter()
        .find(|address| address.is_ipv4() == config.bind.is_ipv4())
        .unwrap_or(config.bind);
    let shutdown_endpoint = endpoint.clone();
    let task = tokio::spawn(serve_endpoint(
        endpoint,
        Arc::new(RoomRegistry::new(registry_config)),
        server_config,
        observe_connection,
    ));

    Ok(ServerHandle {
        endpoint: shutdown_endpoint,
        endpoint_addr: address,
        requested_bind: config.bind,
        bound_addr,
        shutdown_deadline: config.close_grace,
        task,
    })
}

fn observe_connection(result: &Result<(), IrohRendezvousError>) {
    match result {
        Ok(()) => tracing::info!(outcome = "paired", "rendezvous connection completed"),
        Err(IrohRendezvousError::Core(RendezvousError::Expired)) => {
            tracing::info!(outcome = "expired", "rendezvous connection completed");
        }
        Err(IrohRendezvousError::Core(RendezvousError::Rejected(reason))) => {
            tracing::warn!(
                outcome = "rejected",
                reason = ?reason,
                "rendezvous connection completed"
            );
        }
        Err(IrohRendezvousError::Transport { operation }) => {
            tracing::warn!(
                outcome = "transport",
                operation = ?operation,
                "rendezvous connection completed"
            );
        }
        Err(IrohRendezvousError::Deadline { wait }) => {
            tracing::warn!(
                outcome = "transport",
                wait = ?wait,
                "rendezvous connection deadline"
            );
        }
        Err(IrohRendezvousError::Core(RendezvousError::Io { operation })) => {
            tracing::warn!(
                outcome = "transport",
                operation = ?operation,
                "rendezvous connection completed"
            );
        }
        Err(IrohRendezvousError::Core(RendezvousError::Deadline { wait })) => {
            tracing::warn!(
                outcome = "transport",
                wait = ?wait,
                "rendezvous connection deadline"
            );
        }
        Err(IrohRendezvousError::Core(RendezvousError::PeerClosed)) => {
            tracing::info!(outcome = "peer_closed", "rendezvous connection completed");
        }
        Err(IrohRendezvousError::Core(RendezvousError::Control(error))) => {
            tracing::warn!(
                outcome = "invalid_control",
                error = ?error,
                "rendezvous connection completed"
            );
        }
        Err(
            IrohRendezvousError::InvalidConfig(_)
            | IrohRendezvousError::Core(
                RendezvousError::InvalidConfig(_)
                | RendezvousError::RegistryUnavailable
                | RendezvousError::WaiterIdExhausted,
            )
            | IrohRendezvousError::ConnectionTaskFailed,
        ) => {
            tracing::error!(outcome = "internal", "rendezvous connection failed");
        }
    }
}

fn map_server_task(
    completed: Result<Result<(), IrohRendezvousError>, tokio::task::JoinError>,
) -> Result<(), ServerError> {
    completed
        .map_err(|_| ServerError::ServerTaskFailed)?
        .map_err(ServerError::Iroh)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use envoix_rendezvous::{ConfigError as RendezvousConfigError, ConfigField};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn secret_key_is_created_then_reused() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("nested").join("node.key");

        let first = key::load_or_create_node_key(&path).unwrap();
        let second = key::load_or_create_node_key(&path).unwrap();

        assert_eq!(first.public(), second.public());
        assert_eq!(fs::read(&path).unwrap().len(), 32);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn wrong_length_key_file_errors() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("bad.key");
        fs::write(&path, b"too short").unwrap();

        assert!(matches!(
            key::load_or_create_node_key(&path),
            Err(KeyError::InvalidLength { actual: 9 })
        ));
    }

    #[test]
    fn config_validation_preserves_typed_origin() {
        let mut config = ServerConfig::operational_defaults();
        config.max_waiting_rooms = 0;

        assert!(matches!(
            config.mechanism_configs(),
            Err(ServerError::RendezvousConfig(
                RendezvousConfigError::ZeroLimit {
                    field: ConfigField::WaitingRooms
                }
            ))
        ));
    }

    #[test]
    fn node_key_default_matches_identifier_manifest() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../server-identifiers.toml")).unwrap();
        assert_eq!(
            manifest["rendezvous"]["node_key_path"].as_str(),
            Some(DEFAULT_NODE_KEY_PATH)
        );
    }
}
