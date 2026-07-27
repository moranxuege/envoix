use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use envoix_deployment::{DeploymentCatalogue, Service as DeployedService};
use envoix_rendezvous::{ControlLimits, RegistryConfig};
use envoix_rendezvous_iroh::IrohServerConfig;
use iroh::RelayUrl;

use crate::ServerError;
use crate::budget::{BudgetPlan, Service};

// The 95xx block, which the deployment catalogue reserves as UNALLOCATED. A
// flagless run is a bootstrap or a local experiment and must not silently
// occupy an environment's number: these used to be 94xx, which the owner's
// allocation made PROD's, so `envoix-server` with no arguments quietly sat on
// the production rendezvous port. Defaulting into the reserved block keeps the
// bootstrap path working while making that collision unspellable.
pub const DEFAULT_BIND: &str = "0.0.0.0:9545";
pub const DEFAULT_MAILBOX_BIND: &str = "0.0.0.0:9560";
pub const DEFAULT_DIAGNOSTICS_BIND: &str = "127.0.0.1:9562";
pub const DEFAULT_NODE_KEY_PATH: &str = "rendezvous-node.key";
pub const DEFAULT_ROOM_TTL_SECS: u64 = 300;
pub const DEFAULT_RELAY_TTL_SECS: u64 = 120;
pub const DEFAULT_JOIN_DEADLINE_SECS: u64 = 10;
pub const DEFAULT_CLOSE_GRACE_SECS: u64 = 10;
pub const DEFAULT_MAX_ROOM_KEY_LENGTH: usize = 64;
pub const DEFAULT_MAX_WAITING_ROOMS: usize = 4096;
pub const DEFAULT_MAX_CONNECTIONS: usize = 256;
pub const DEFAULT_MAILBOX_MAX_CONNECTIONS: usize = 64;
pub const DEFAULT_DIAGNOSTICS_MAX_CONNECTIONS: usize = 4;
pub const DEFAULT_PAIRING_WORKERS: usize = 2;
pub const DEFAULT_MAILBOX_WORKERS: usize = 2;
pub const DEFAULT_DIAGNOSTICS_WORKERS: usize = 1;
pub const DEFAULT_HANDSHAKE_DEADLINE_SECS: u64 = 10;
pub const DEFAULT_MAILBOX_TTL_SECS: u64 = 300;
pub const DEFAULT_MAILBOX_MAX_BLOB_SIZE: usize = 8 * 1024;
pub const DEFAULT_MAILBOX_MAX_KEY_LENGTH: usize = 64;
pub const DEFAULT_MAILBOX_MAX_ENTRIES: usize = 4096;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub mailbox_bind: SocketAddr,
    pub diagnostics_bind: SocketAddr,
    pub node_key_path: PathBuf,
    pub relay: Option<RelayUrl>,
    pub room_ttl: Duration,
    pub relay_ttl: Duration,
    pub join_deadline: Duration,
    pub close_grace: Duration,
    pub max_room_key_length: usize,
    pub max_waiting_rooms: usize,
    pub max_connections: usize,
    pub mailbox_max_connections: usize,
    pub diagnostics_max_connections: usize,
    pub pairing_workers: usize,
    pub mailbox_workers: usize,
    pub diagnostics_workers: usize,
    pub handshake_deadline: Duration,
    pub bind_deadline: Duration,
    pub mailbox_ttl: Duration,
    pub mailbox_max_blob_size: usize,
    pub mailbox_max_key_length: usize,
    pub mailbox_max_entries: usize,
}

impl ServerConfig {
    pub fn operational_defaults() -> Self {
        Self {
            bind: DEFAULT_BIND.parse().expect("DEFAULT_BIND must be valid"),
            mailbox_bind: DEFAULT_MAILBOX_BIND
                .parse()
                .expect("DEFAULT_MAILBOX_BIND must be valid"),
            diagnostics_bind: DEFAULT_DIAGNOSTICS_BIND
                .parse()
                .expect("DEFAULT_DIAGNOSTICS_BIND must be valid"),
            node_key_path: PathBuf::from(DEFAULT_NODE_KEY_PATH),
            relay: None,
            room_ttl: Duration::from_secs(DEFAULT_ROOM_TTL_SECS),
            relay_ttl: Duration::from_secs(DEFAULT_RELAY_TTL_SECS),
            join_deadline: Duration::from_secs(DEFAULT_JOIN_DEADLINE_SECS),
            close_grace: Duration::from_secs(DEFAULT_CLOSE_GRACE_SECS),
            max_room_key_length: DEFAULT_MAX_ROOM_KEY_LENGTH,
            max_waiting_rooms: DEFAULT_MAX_WAITING_ROOMS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            mailbox_max_connections: DEFAULT_MAILBOX_MAX_CONNECTIONS,
            diagnostics_max_connections: DEFAULT_DIAGNOSTICS_MAX_CONNECTIONS,
            pairing_workers: DEFAULT_PAIRING_WORKERS,
            mailbox_workers: DEFAULT_MAILBOX_WORKERS,
            diagnostics_workers: DEFAULT_DIAGNOSTICS_WORKERS,
            handshake_deadline: Duration::from_secs(DEFAULT_HANDSHAKE_DEADLINE_SECS),
            bind_deadline: Duration::from_secs(DEFAULT_HANDSHAKE_DEADLINE_SECS),
            mailbox_ttl: Duration::from_secs(DEFAULT_MAILBOX_TTL_SECS),
            mailbox_max_blob_size: DEFAULT_MAILBOX_MAX_BLOB_SIZE,
            mailbox_max_key_length: DEFAULT_MAILBOX_MAX_KEY_LENGTH,
            mailbox_max_entries: DEFAULT_MAILBOX_MAX_ENTRIES,
        }
    }

    /// Takes every port from the catalogue's entry for `environment`, and only
    /// if that environment is deployable. A half-provisioned environment has no
    /// identity to serve under, so it does not get to be served under one.
    pub fn for_environment(self, environment: &str) -> Result<Self, ServerError> {
        let catalogue = DeploymentCatalogue::compiled()
            .map_err(|error| ServerError::Catalogue(error.to_string()))?;
        self.for_declared(&catalogue, environment)
    }

    fn for_declared(
        mut self,
        catalogue: &DeploymentCatalogue,
        environment: &str,
    ) -> Result<Self, ServerError> {
        let blockers = catalogue.blockers(environment);
        if !blockers.is_empty() {
            return Err(ServerError::EnvironmentNotDeployable {
                environment: environment.to_owned(),
                blockers: blockers.iter().map(ToString::to_string).collect(),
            });
        }
        let declared = catalogue
            .environment(environment)
            .expect("a deployable environment is declared");
        // The catalogue names the public host; a server binds every interface
        // on the port that host is reached at. Diagnostics keeps its declared
        // loopback bind, which is the point of declaring it.
        self.bind = SocketAddr::new(self.bind.ip(), declared.port(DeployedService::Rendezvous));
        self.mailbox_bind = SocketAddr::new(
            self.mailbox_bind.ip(),
            declared.port(DeployedService::Mailbox),
        );
        self.diagnostics_bind = SocketAddr::new(
            declared
                .diagnostics
                .bind
                .parse()
                .map_err(|_| ServerError::Catalogue("diagnostics bind is not an IP".into()))?,
            declared.port(DeployedService::Diagnostics),
        );
        Ok(self)
    }

    pub(crate) const fn budget(&self, service: Service) -> BudgetPlan {
        match service {
            Service::Pairing => BudgetPlan {
                max_concurrent: self.max_connections,
                worker_threads: self.pairing_workers,
            },
            Service::Mailbox => BudgetPlan {
                max_concurrent: self.mailbox_max_connections,
                worker_threads: self.mailbox_workers,
            },
            Service::Diagnostics => BudgetPlan {
                max_concurrent: self.diagnostics_max_connections,
                worker_threads: self.diagnostics_workers,
            },
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
        let adapter =
            IrohServerConfig::new(self.handshake_deadline).map_err(ServerError::IrohConfig)?;
        Ok((registry, adapter))
    }

    pub(crate) fn validate_mailbox(&self) -> Result<(), ServerError> {
        if self.mailbox_ttl.is_zero() {
            return Err(ServerError::InvalidMailboxConfig("TTL must be nonzero"));
        }
        if self.mailbox_max_blob_size == 0 {
            return Err(ServerError::InvalidMailboxConfig(
                "maximum blob size must be nonzero",
            ));
        }
        if self.mailbox_max_key_length == 0 {
            return Err(ServerError::InvalidMailboxConfig(
                "maximum key length must be nonzero",
            ));
        }
        if self.mailbox_max_entries == 0 {
            return Err(ServerError::InvalidMailboxConfig(
                "maximum entry count must be nonzero",
            ));
        }
        Ok(())
    }

    /// This process may not take a port that belongs to somebody else's live
    /// service, whatever the command line says. The list is the catalogue's,
    /// not a constant hidden in here.
    pub(crate) fn validate_binds(&self) -> Result<(), ServerError> {
        let catalogue = DeploymentCatalogue::compiled()
            .map_err(|error| ServerError::Catalogue(error.to_string()))?;
        for bind in [self.bind, self.mailbox_bind, self.diagnostics_bind] {
            if let Some(reserved) = catalogue.reserved_port(bind.port()) {
                return Err(ServerError::ReservedPort {
                    port: bind.port(),
                    owner: reserved.owner.clone(),
                });
            }
        }
        if !self.diagnostics_bind.ip().is_loopback() {
            return Err(ServerError::DiagnosticsNotLoopback(self.diagnostics_bind));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use envoix_deployment::CATALOGUE_TOML;

    use super::*;

    /// dev is the environment one provision away from deployable: it holds the
    /// node id promoted from test and lacks only a trust root, which needs
    /// hostnames that do not resolve yet.
    fn provisioned_dev_catalogue() -> DeploymentCatalogue {
        let text = CATALOGUE_TOML.replace(
            "root_sha256 = \"TBD_PROVISION_DEV_TRUST_ROOT_SHA256\"\nprovisioning_status = \"tbd\"",
            "root_sha256 = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\"\nprovisioning_status = \"provisioned\"",
        );
        DeploymentCatalogue::parse(&text).unwrap()
    }

    /// Naming an environment takes its ports from the catalogue, so the file and
    /// the process cannot disagree about where a service listens.
    #[test]
    fn a_named_environment_binds_the_ports_it_is_declared_with() {
        let catalogue = provisioned_dev_catalogue();
        let config = ServerConfig::operational_defaults()
            .for_declared(&catalogue, "dev")
            .unwrap();

        let declared = catalogue.environment("dev").unwrap();
        assert_eq!(config.bind.port(), declared.rendezvous.port);
        assert_eq!(config.mailbox_bind.port(), declared.mailbox.port);
        assert_eq!(config.diagnostics_bind.port(), declared.diagnostics.port);
        assert!(config.diagnostics_bind.ip().is_loopback());
        assert!(
            config.bind.ip().is_unspecified(),
            "a server serves the host"
        );
        config
            .validate_binds()
            .expect("no reserved port is claimed");
    }

    /// The promotion gate, from the process side: an environment missing either
    /// provisioned value cannot be served under its own name.
    #[test]
    fn an_unprovisioned_environment_cannot_be_served() {
        let shipped = DeploymentCatalogue::compiled().unwrap();
        let refused = ServerConfig::operational_defaults().for_declared(&shipped, "test");
        assert!(matches!(
            refused,
            Err(ServerError::EnvironmentNotDeployable { ref blockers, .. })
                if blockers.iter().any(|blocker| blocker.contains("trust.root_sha256"))
        ));

        let unknown = ServerConfig::operational_defaults().for_declared(&shipped, "staging");
        assert!(matches!(
            unknown,
            Err(ServerError::EnvironmentNotDeployable { .. })
        ));
    }

    /// The hard boundary, from the process side.
    #[test]
    fn a_port_belonging_to_another_service_is_never_bound() {
        let mut config = ServerConfig::operational_defaults();
        config.bind = "0.0.0.0:8445".parse().unwrap();
        assert!(matches!(
            config.validate_binds(),
            Err(ServerError::ReservedPort { port: 8445, .. })
        ));

        let mut exposed = ServerConfig::operational_defaults();
        exposed.diagnostics_bind = "0.0.0.0:9462".parse().unwrap();
        assert!(matches!(
            exposed.validate_binds(),
            Err(ServerError::DiagnosticsNotLoopback(_))
        ));
    }
}
