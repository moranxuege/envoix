//! Composition root for the Envoix rendezvous server.
//!
//! Three services run here — pairing, the receipt mailbox and diagnostics —
//! and the only thing they share is the process. Each has its own listener, its
//! own admission budget and its own worker threads, so a flood arriving at one
//! cannot take capacity from another: there is no shared pool to take it from.

#![forbid(unsafe_code)]

mod budget;
mod config;
mod diagnostics;
mod error;
mod key;
mod mailbox;
mod serve;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use envoix_rendezvous::{RendezvousError, RoomRegistry};
use envoix_rendezvous_iroh::{
    ConnectionAdmission, EndpointConfig, IrohRendezvousError, ServeOutcome, bind_endpoint,
    endpoint_addr, serve_endpoint,
};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use budget::{Admission, ServiceBudget, ServiceBudgets, ServiceRuntime};

pub use budget::{BudgetMeter, Service};
pub use config::{
    DEFAULT_BIND, DEFAULT_CLOSE_GRACE_SECS, DEFAULT_DIAGNOSTICS_BIND,
    DEFAULT_DIAGNOSTICS_MAX_CONNECTIONS, DEFAULT_DIAGNOSTICS_WORKERS,
    DEFAULT_HANDSHAKE_DEADLINE_SECS, DEFAULT_JOIN_DEADLINE_SECS, DEFAULT_MAILBOX_BIND,
    DEFAULT_MAILBOX_MAX_BLOB_SIZE, DEFAULT_MAILBOX_MAX_CONNECTIONS, DEFAULT_MAILBOX_MAX_ENTRIES,
    DEFAULT_MAILBOX_MAX_KEY_LENGTH, DEFAULT_MAILBOX_TTL_SECS, DEFAULT_MAILBOX_WORKERS,
    DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_ROOM_KEY_LENGTH, DEFAULT_MAX_WAITING_ROOMS,
    DEFAULT_NODE_KEY_PATH, DEFAULT_PAIRING_WORKERS, DEFAULT_RELAY_TTL_SECS, DEFAULT_ROOM_TTL_SECS,
    ServerConfig,
};
pub use diagnostics::BUDGET_HTTP_ROUTE;
pub use error::{KeyError, KeyOperation, ServerError};

/// The pairing budget, offered to the rendezvous adapter as its admission
/// policy. The adapter decides how a refusal is spoken; this decides who is
/// refused, and it can only ever spend pairing's own slots.
impl ConnectionAdmission for ServiceBudget {
    type Permit = Admission;

    fn try_admit(&self) -> Option<Self::Permit> {
        Self::try_admit(self)
    }
}

pub struct ServerHandle {
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
    requested_bind: SocketAddr,
    bound_addr: SocketAddr,
    mailbox_bound_addr: SocketAddr,
    diagnostics_bound_addr: SocketAddr,
    shutdown_deadline: Duration,
    meters: [BudgetMeter; 3],
    stopped: Arc<Notify>,
    iroh_task: JoinHandle<Result<(), IrohRendezvousError>>,
    mailbox_task: JoinHandle<Result<(), std::io::Error>>,
    diagnostics_task: JoinHandle<Result<(), std::io::Error>>,
    http_shutdown: Vec<oneshot::Sender<()>>,
    /// Held so the workers outlive the work. Dropping never blocks.
    _runtimes: [ServiceRuntime; 3],
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

    pub const fn mailbox_bound_addr(&self) -> SocketAddr {
        self.mailbox_bound_addr
    }

    pub const fn diagnostics_bound_addr(&self) -> SocketAddr {
        self.diagnostics_bound_addr
    }

    /// What each service may spend and what it has spent.
    pub fn meters(&self) -> &[BudgetMeter; 3] {
        &self.meters
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
        !self.iroh_task.is_finished()
            && !self.mailbox_task.is_finished()
            && !self.diagnostics_task.is_finished()
    }

    pub async fn shutdown(mut self) -> Result<(), ServerError> {
        let endpoint = self.endpoint.clone();
        for shutdown in self.http_shutdown.drain(..) {
            let _ = shutdown.send(());
        }
        match timeout(self.shutdown_deadline, async {
            endpoint.close().await;
            tokio::join!(
                &mut self.iroh_task,
                &mut self.mailbox_task,
                &mut self.diagnostics_task
            )
        })
        .await
        {
            Ok((iroh, mailbox, diagnostics)) => {
                map_iroh_task(iroh)?;
                map_http_task(
                    mailbox,
                    ServerError::Mailbox,
                    ServerError::MailboxTaskFailed,
                )?;
                map_http_task(
                    diagnostics,
                    ServerError::Diagnostics,
                    ServerError::DiagnosticsTaskFailed,
                )
            }
            Err(_) => {
                self.iroh_task.abort();
                self.mailbox_task.abort();
                self.diagnostics_task.abort();
                let _ = self.iroh_task.await;
                let _ = self.mailbox_task.await;
                let _ = self.diagnostics_task.await;
                Err(ServerError::ShutdownDeadline)
            }
        }
    }

    pub async fn wait_for_ctrl_c(self) -> Result<(), ServerError> {
        let stopped = self.stopped.clone();
        tokio::select! {
            signal = tokio::signal::ctrl_c() => signal.map_err(ServerError::Signal)?,
            () = stopped.notified() => {}
        }
        self.shutdown().await
    }
}

pub async fn run(config: ServerConfig) -> Result<ServerHandle, ServerError> {
    let (registry_config, adapter_config) = config.mechanism_configs()?;
    config.validate_mailbox()?;
    config.validate_binds()?;
    let budgets = ServiceBudgets::build(|service| config.budget(service))?;
    let meters = budgets.meters();
    let stopped = Arc::new(Notify::new());

    // Bound synchronously so the caller learns the port before anything is
    // served, then handed to the service that owns it: a tokio socket belongs
    // to the reactor that registers it, and each service has its own.
    let mailbox_listener =
        std::net::TcpListener::bind(config.mailbox_bind).map_err(ServerError::MailboxBind)?;
    mailbox_listener
        .set_nonblocking(true)
        .map_err(ServerError::MailboxBind)?;
    let mailbox_bound_addr = mailbox_listener
        .local_addr()
        .map_err(ServerError::MailboxBind)?;
    let diagnostics_listener = std::net::TcpListener::bind(config.diagnostics_bind)
        .map_err(ServerError::DiagnosticsBind)?;
    diagnostics_listener
        .set_nonblocking(true)
        .map_err(ServerError::DiagnosticsBind)?;
    let diagnostics_bound_addr = diagnostics_listener
        .local_addr()
        .map_err(ServerError::DiagnosticsBind)?;

    let secret_key = key::load_or_create_node_key(&config.node_key_path)?;
    let endpoint_config =
        EndpointConfig::new(config.bind, config.relay, secret_key, config.bind_deadline)
            .map_err(ServerError::IrohConfig)?;

    let (pairing_budget, pairing_runtime) = budgets.pairing;
    let (mailbox_budget, mailbox_runtime) = budgets.mailbox;
    let (diagnostics_budget, diagnostics_runtime) = budgets.diagnostics;

    // The endpoint is bound on pairing's own runtime so that iroh's internal
    // tasks belong to pairing's workers too — otherwise the budget would cover
    // only the part of pairing this file can see.
    let (ready, bound) = oneshot::channel();
    let pairing_meter = meters[0].clone();
    let requested_bind = config.bind;
    let notify_stopped = stopped.clone();
    let mut iroh_task = pairing_runtime.spawn(async move {
        let endpoint = bind_endpoint(endpoint_config).await?;
        let bound_addr = endpoint
            .bound_sockets()
            .into_iter()
            .find(|address| address.is_ipv4() == requested_bind.is_ipv4())
            .unwrap_or(requested_bind);
        if ready
            .send((endpoint.clone(), endpoint_addr(&endpoint), bound_addr))
            .is_err()
        {
            return Ok(());
        }
        let observed = pairing_meter.clone();
        let result = serve_endpoint(
            endpoint,
            Arc::new(RoomRegistry::new(registry_config)),
            adapter_config,
            pairing_budget,
            move |outcome| observe_connection(&outcome, &observed),
        )
        .await;
        notify_stopped.notify_one();
        result
    });
    let Ok((endpoint, address, bound_addr)) = bound.await else {
        return Err(match (&mut iroh_task).await {
            Ok(Err(error)) => ServerError::Iroh(error),
            _ => ServerError::ServerTaskFailed,
        });
    };

    let (mailbox_shutdown, mailbox_shutdown_receiver) = oneshot::channel();
    let notify_stopped = stopped.clone();
    let mailbox_task = mailbox_runtime.spawn(async move {
        let result = mailbox::serve(
            mailbox_listener,
            mailbox::MailboxLimits {
                ttl: config.mailbox_ttl,
                max_blob_size: config.mailbox_max_blob_size,
                max_key_length: config.mailbox_max_key_length,
                max_entries: config.mailbox_max_entries,
            },
            mailbox_budget,
            mailbox_shutdown_receiver,
        )
        .await;
        notify_stopped.notify_one();
        result
    });

    let (diagnostics_shutdown, diagnostics_shutdown_receiver) = oneshot::channel();
    let notify_stopped = stopped.clone();
    let readout = meters.clone();
    let diagnostics_task = diagnostics_runtime.spawn(async move {
        let result = diagnostics::serve(
            diagnostics_listener,
            diagnostics_budget,
            readout,
            diagnostics_shutdown_receiver,
        )
        .await;
        notify_stopped.notify_one();
        result
    });

    Ok(ServerHandle {
        endpoint,
        endpoint_addr: address,
        requested_bind,
        bound_addr,
        mailbox_bound_addr,
        diagnostics_bound_addr,
        shutdown_deadline: config.close_grace,
        meters,
        stopped,
        iroh_task,
        mailbox_task,
        diagnostics_task,
        http_shutdown: vec![mailbox_shutdown, diagnostics_shutdown],
        _runtimes: [pairing_runtime, mailbox_runtime, diagnostics_runtime],
    })
}

fn observe_connection(outcome: &ServeOutcome<'_>, meter: &BudgetMeter) {
    meter.record_worker();
    let result = match outcome {
        ServeOutcome::Refused => {
            meter.record_refused();
            tracing::warn!(
                capacity = meter.capacity(),
                "refused: the pairing budget is full"
            );
            return;
        }
        ServeOutcome::Completed(result) => result,
    };
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
        Err(IrohRendezvousError::Refused) => {
            tracing::warn!(outcome = "refused", "rendezvous connection refused");
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

fn map_iroh_task(
    completed: Result<Result<(), IrohRendezvousError>, tokio::task::JoinError>,
) -> Result<(), ServerError> {
    completed
        .map_err(|_| ServerError::ServerTaskFailed)?
        .map_err(ServerError::Iroh)
}

fn map_http_task(
    completed: Result<Result<(), std::io::Error>, tokio::task::JoinError>,
    failed: fn(std::io::Error) -> ServerError,
    joined: ServerError,
) -> Result<(), ServerError> {
    completed.map_err(|_| joined)?.map_err(failed)
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
