use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use envoix_server::{
    DEFAULT_BIND, DEFAULT_CLOSE_GRACE_SECS, DEFAULT_HANDSHAKE_DEADLINE_SECS,
    DEFAULT_JOIN_DEADLINE_SECS, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_ROOM_KEY_LENGTH,
    DEFAULT_MAX_WAITING_ROOMS, DEFAULT_NODE_KEY_PATH, DEFAULT_RELAY_TTL_SECS,
    DEFAULT_ROOM_TTL_SECS, ServerConfig, ServerError, ServerHandle, run,
};
use iroh::RelayUrl;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "envoix-server", about = "Envoix blind rendezvous server")]
struct Cli {
    #[arg(long, default_value = DEFAULT_BIND)]
    bind: SocketAddr,
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
    #[arg(long, default_value_t = DEFAULT_HANDSHAKE_DEADLINE_SECS)]
    handshake_deadline: u64,
}

impl Cli {
    fn into_config(self) -> ServerConfig {
        let mut config = ServerConfig::operational_defaults();
        config.bind = self.bind;
        config.node_key_path = self.secret_key;
        config.relay = self.relay;
        config.room_ttl = Duration::from_secs(self.room_ttl);
        config.relay_ttl = Duration::from_secs(self.relay_ttl);
        config.join_deadline = Duration::from_secs(self.join_deadline);
        config.close_grace = Duration::from_secs(self.close_grace);
        config.max_room_key_length = self.max_room_key_len;
        config.max_waiting_rooms = self.max_waiting_rooms;
        config.max_connections = self.max_connections;
        config.handshake_deadline = Duration::from_secs(self.handshake_deadline);
        config
    }
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("envoix_server=info,warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let handle = run(Cli::parse().into_config()).await?;
    announce(&handle);
    handle.wait_for_ctrl_c().await
}

fn announce(handle: &ServerHandle) {
    tracing::info!(
        endpoint_id = %handle.endpoint_id(),
        bind = %handle.bound_addr(),
        connect = %handle.connect_string(),
        "rendezvous server listening"
    );
    println!("rendezvous endpoint id: {}", handle.endpoint_id());
    println!("listening on {}", handle.bound_addr());
    println!("connect with: {}", handle.connect_string());
}
