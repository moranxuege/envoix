//! Rendezvous server binary: bind an iroh endpoint, print a usable rendezvous
//! address (`<endpoint-id>@<ip:port>`), and serve room pairing until terminated.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use iroh::SecretKey;

use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{PeerLocator, build_endpoint, relay_mode_from_url, serve_endpoint};

mod geoip;
mod logs;

#[derive(Parser)]
#[command(
    name = "envoix-rendezvous-server",
    about = "Envoix room rendezvous (iroh node)"
)]
struct Cli {
    /// UDP address to bind the iroh endpoint to. Defaults to a fixed port so the
    /// advertised `<endpoint-id>@<ip:port>` stays stable across restarts; use a
    /// random `:0` port only for throwaway or relay-only setups.
    #[arg(long, default_value = "0.0.0.0:8445")]
    bind: SocketAddr,
    /// File holding the server's persistent secret key (created with owner-only
    /// permissions if missing), so the endpoint id stays stable across restarts.
    #[arg(long, default_value = "rendezvous-secret.key")]
    secret_key: PathBuf,
    /// Relay URL to register with for WAN reachability (e.g.
    /// https://relay.example.com:8444). Omit for no relay (LAN/direct only).
    #[arg(long)]
    relay: Option<String>,
    /// How long (seconds) a parked peer waits for a partner before the room
    /// expires. The first peer is dropped with an expiry notice after this.
    #[arg(long, default_value_t = 300)]
    room_ttl: u64,
    /// Log output format: `pretty` human lines, or `json` (one object per line)
    /// for log aggregators and campaign correlation.
    #[arg(long, value_enum, default_value_t = LogFormat::Pretty)]
    log_format: LogFormat,
    /// Optional MaxMind-format City database (GeoLite2 or DB-IP Lite `.mmdb`);
    /// when set, peer log lines are annotated with the peer's city/country.
    #[arg(long)]
    geoip_city: Option<PathBuf>,
    /// Optional MaxMind-format ASN database; when set, peer log lines are
    /// annotated with the peer's ISP/carrier.
    #[arg(long)]
    geoip_asn: Option<PathBuf>,
    /// HTTP address for the per-room log-collection endpoint
    /// (`POST /logs/<room_id>?side=…`, `GET /logs/<room_id>`). Omit to disable.
    #[arg(long)]
    log_bind: Option<SocketAddr>,
    /// How long (seconds) collected logs are kept after their last update.
    #[arg(long, default_value_t = 3600)]
    log_ttl: u64,
}

/// How server logs are rendered.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum LogFormat {
    Pretty,
    Json,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Include the broker crate (`envoix_rendezvous`) at info, not just the iroh
    // wiring - otherwise pairings/expiries (its target) fall to the global warn
    // default and never show.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "envoix_rendezvous=info,envoix_rendezvous_iroh=info,warn",
        )
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match cli.log_format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.init(),
    }

    let secret_key = load_or_create_secret_key(&cli.secret_key)
        .with_context(|| format!("secret key {}", cli.secret_key.display()))?;
    let relay = relay_mode_from_url(cli.relay.as_deref())?;
    let endpoint = build_endpoint(cli.bind, secret_key, relay).await?;
    tracing::info!(endpoint_id = %endpoint.id(), bind = %cli.bind, "rendezvous server listening");
    // Human copy-paste convenience (endpoint id + a ready-to-use --rendezvous
    // value). Suppressed under `--log-format json` so the stream stays pure
    // JSON; the same facts are in the structured "listening" log above.
    if matches!(cli.log_format, LogFormat::Pretty) {
        println!("rendezvous endpoint id: {}", endpoint.id());
        println!("listening on {}", cli.bind);
        // When bound to an unspecified address (0.0.0.0/::) the reachable host is
        // unknown to the process, so show the (fixed) port and let the operator
        // fill in the public IP.
        if cli.bind.ip().is_unspecified() {
            println!(
                "connect with: --rendezvous {}@<this-host-ip>:{}",
                endpoint.id(),
                cli.bind.port()
            );
        } else {
            println!("connect with: --rendezvous {}@{}", endpoint.id(), cli.bind);
        }
    }

    // Build the optional GeoIP annotator from the operator-supplied databases.
    let locate: Option<PeerLocator> =
        match geoip::GeoIp::load(cli.geoip_city.as_deref(), cli.geoip_asn.as_deref())? {
            Some(geo) => {
                let geo = Arc::new(geo);
                tracing::info!("GeoIP annotation enabled");
                Some(Arc::new(move |ip| geo.describe(ip)))
            }
            None => None,
        };

    // Optional per-room log-collection endpoint on its own HTTP port. Off unless
    // --log-bind is given; a separate task that never touches the pairing endpoint.
    if let Some(addr) = cli.log_bind {
        let store = Arc::new(logs::RoomLogs::new(Duration::from_secs(cli.log_ttl)));
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    tracing::info!(%addr, "log endpoint listening");
                    if let Err(error) = axum::serve(listener, logs::router(store)).await {
                        tracing::error!(%error, "log endpoint failed");
                    }
                }
                Err(error) => tracing::error!(%error, %addr, "log endpoint bind failed"),
            }
        });
    }

    serve_endpoint(
        endpoint,
        Arc::new(RoomRegistry::with_ttl(Duration::from_secs(cli.room_ttl))),
        locate,
    )
    .await
}

/// Load the server's secret key from `path`, creating a fresh one if the file
/// does not exist, so the endpoint id is stable across restarts. A newly
/// created file is written with owner-only permissions on Unix.
fn load_or_create_secret_key(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key file must be exactly 32 bytes"))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    write_secret_key(path, &key.to_bytes())?;
    Ok(key)
}

#[cfg(unix)]
fn write_secret_key(path: &Path, bytes: &[u8; 32]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(not(unix))]
fn write_secret_key(path: &Path, bytes: &[u8; 32]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn secret_key_is_created_then_reused() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("secret.key");

        let first = load_or_create_secret_key(&path).expect("create");
        assert!(path.exists(), "key file should be created");
        let second = load_or_create_secret_key(&path).expect("reuse");

        assert_eq!(first.public(), second.public(), "key must be stable");
    }

    #[test]
    fn wrong_length_key_file_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.key");
        std::fs::write(&path, b"too short").unwrap();
        assert!(load_or_create_secret_key(&path).is_err());
    }
}
