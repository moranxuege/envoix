use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use envoix_server::{
    DEFAULT_BIND, DEFAULT_CLOSE_GRACE_SECS, DEFAULT_DIAGNOSTICS_BIND,
    DEFAULT_DIAGNOSTICS_MAX_CONNECTIONS, DEFAULT_DIAGNOSTICS_WORKERS,
    DEFAULT_HANDSHAKE_DEADLINE_SECS, DEFAULT_JOIN_DEADLINE_SECS, DEFAULT_MAILBOX_BIND,
    DEFAULT_MAILBOX_MAX_BLOB_SIZE, DEFAULT_MAILBOX_MAX_CONNECTIONS, DEFAULT_MAILBOX_MAX_ENTRIES,
    DEFAULT_MAILBOX_MAX_KEY_LENGTH, DEFAULT_MAILBOX_TTL_SECS, DEFAULT_MAILBOX_WORKERS,
    DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_ROOM_KEY_LENGTH, DEFAULT_MAX_WAITING_ROOMS,
    DEFAULT_NODE_KEY_PATH, DEFAULT_PAIRING_WORKERS, DEFAULT_RELAY_TTL_SECS, DEFAULT_ROOM_TTL_SECS,
    ServerConfig, ServerError, ServerHandle, run,
};
use iroh::RelayUrl;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "envoix-server", about = "Envoix blind rendezvous server")]
struct Cli {
    /// Serve as a named environment: every port comes from
    /// `deploy/environments.toml`, and an environment that is not fully
    /// provisioned is refused rather than half-served.
    #[arg(long, conflicts_with_all = ["bind", "mailbox_bind", "diagnostics_bind"])]
    environment: Option<String>,
    #[arg(long, default_value = DEFAULT_BIND)]
    bind: SocketAddr,
    #[arg(long, default_value = DEFAULT_MAILBOX_BIND)]
    mailbox_bind: SocketAddr,
    #[arg(long, default_value = DEFAULT_DIAGNOSTICS_BIND)]
    diagnostics_bind: SocketAddr,
    #[arg(long, default_value = DEFAULT_NODE_KEY_PATH)]
    secret_key: PathBuf,
    #[arg(long)]
    relay: Option<RelayUrl>,
    #[arg(long, default_value_t = DEFAULT_ROOM_TTL_SECS)]
    room_ttl: u64,
    #[arg(long, default_value_t = DEFAULT_RELAY_TTL_SECS)]
    relay_ttl: u64,
    #[arg(long, default_value_t = DEFAULT_JOIN_DEADLINE_SECS)]
    join_deadline: u64,
    #[arg(long, default_value_t = DEFAULT_CLOSE_GRACE_SECS)]
    close_grace: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_ROOM_KEY_LENGTH)]
    max_room_key_len: usize,
    #[arg(long, default_value_t = DEFAULT_MAX_WAITING_ROOMS)]
    max_waiting_rooms: usize,
    #[arg(long, default_value_t = DEFAULT_MAX_CONNECTIONS)]
    max_connections: usize,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_MAX_CONNECTIONS)]
    mailbox_max_connections: usize,
    #[arg(long, default_value_t = DEFAULT_DIAGNOSTICS_MAX_CONNECTIONS)]
    diagnostics_max_connections: usize,
    #[arg(long, default_value_t = DEFAULT_PAIRING_WORKERS)]
    pairing_workers: usize,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_WORKERS)]
    mailbox_workers: usize,
    #[arg(long, default_value_t = DEFAULT_DIAGNOSTICS_WORKERS)]
    diagnostics_workers: usize,
    #[arg(long, default_value_t = DEFAULT_HANDSHAKE_DEADLINE_SECS)]
    handshake_deadline: u64,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_TTL_SECS)]
    mailbox_ttl: u64,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_MAX_BLOB_SIZE)]
    mailbox_max_blob_size: usize,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_MAX_KEY_LENGTH)]
    mailbox_max_key_length: usize,
    #[arg(long, default_value_t = DEFAULT_MAILBOX_MAX_ENTRIES)]
    mailbox_max_entries: usize,
}

impl Cli {
    fn into_config(self) -> Result<ServerConfig, ServerError> {
        let mut config = ServerConfig::operational_defaults();
        config.bind = self.bind;
        config.mailbox_bind = self.mailbox_bind;
        config.diagnostics_bind = self.diagnostics_bind;
        config.node_key_path = self.secret_key;
        config.relay = self.relay;
        config.room_ttl = Duration::from_secs(self.room_ttl);
        config.relay_ttl = Duration::from_secs(self.relay_ttl);
        config.join_deadline = Duration::from_secs(self.join_deadline);
        config.close_grace = Duration::from_secs(self.close_grace);
        config.max_room_key_length = self.max_room_key_len;
        config.max_waiting_rooms = self.max_waiting_rooms;
        config.max_connections = self.max_connections;
        config.mailbox_max_connections = self.mailbox_max_connections;
        config.diagnostics_max_connections = self.diagnostics_max_connections;
        config.pairing_workers = self.pairing_workers;
        config.mailbox_workers = self.mailbox_workers;
        config.diagnostics_workers = self.diagnostics_workers;
        config.handshake_deadline = Duration::from_secs(self.handshake_deadline);
        config.mailbox_ttl = Duration::from_secs(self.mailbox_ttl);
        config.mailbox_max_blob_size = self.mailbox_max_blob_size;
        config.mailbox_max_key_length = self.mailbox_max_key_length;
        config.mailbox_max_entries = self.mailbox_max_entries;
        match self.environment {
            Some(environment) => config.for_environment(&environment),
            None => Ok(config),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("envoix_server=info,warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let handle = run(Cli::parse().into_config()?).await?;
    announce(&handle);
    handle.wait_for_ctrl_c().await
}

fn announce(handle: &ServerHandle) {
    tracing::info!(
        endpoint_id = %handle.endpoint_id(),
        bind = %handle.bound_addr(),
        mailbox_bind = %handle.mailbox_bound_addr(),
        diagnostics_bind = %handle.diagnostics_bound_addr(),
        connect = %handle.connect_string(),
        "rendezvous server listening"
    );
    println!("rendezvous endpoint id: {}", handle.endpoint_id());
    println!("listening on {}", handle.bound_addr());
    println!("mailbox listening on {}", handle.mailbox_bound_addr());
    println!(
        "diagnostics listening on {} (loopback only)",
        handle.diagnostics_bound_addr()
    );
    for meter in handle.meters() {
        println!(
            "budget {}: {} concurrent callers",
            meter.service().as_str(),
            meter.capacity()
        );
    }
    println!("connect with: {}", handle.connect_string());
}
