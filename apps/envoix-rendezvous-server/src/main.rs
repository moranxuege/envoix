//! Rendezvous server binary: bind an iroh endpoint, print a usable rendezvous
//! address (`<endpoint-id>@<ip:port>`), and serve room pairing until terminated.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use iroh::SecretKey;

use envoix_rendezvous::{BrokerConfig, RateLimitConfig, RoomRegistry};
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
    /// How long (seconds) expired/exhausted Room tombstones remain unavailable.
    #[arg(long, default_value_t = 300)]
    room_tombstone_ttl: u64,
    /// Cumulative matched-attempt budget for a short Room Code.
    #[arg(long, default_value_t = 6)]
    room_attempt_limit: u32,
    /// Per-Room matched-attempt token refill count.
    #[arg(long, default_value_t = 6)]
    room_rate_events: u32,
    /// Per-Room matched-attempt refill period in seconds.
    #[arg(long, default_value_t = 300)]
    room_rate_period: u64,
    /// Per-Room matched-attempt burst.
    #[arg(long, default_value_t = 2)]
    room_rate_burst: u32,
    /// Per-EndpointId Join token refill count.
    #[arg(long, default_value_t = 10)]
    endpoint_rate_events: u32,
    /// Per-EndpointId Join refill period in seconds.
    #[arg(long, default_value_t = 60)]
    endpoint_rate_period: u64,
    /// Per-EndpointId Join burst.
    #[arg(long, default_value_t = 20)]
    endpoint_rate_burst: u32,
    /// Per-IP Join token refill count when a direct address is observed.
    #[arg(long, default_value_t = 30)]
    ip_rate_events: u32,
    /// Per-IP Join refill period in seconds.
    #[arg(long, default_value_t = 60)]
    ip_rate_period: u64,
    /// Per-IP Join burst.
    #[arg(long, default_value_t = 60)]
    ip_rate_burst: u32,
    /// Per-/24 (IPv4) or /64 (IPv6) Join token refill count.
    #[arg(long, default_value_t = 120)]
    subnet_rate_events: u32,
    /// Per-subnet Join refill period in seconds.
    #[arg(long, default_value_t = 60)]
    subnet_rate_period: u64,
    /// Per-subnet Join burst.
    #[arg(long, default_value_t = 240)]
    subnet_rate_burst: u32,
    /// Maximum live iroh connections.
    #[arg(long, default_value_t = 256)]
    max_connections: usize,
    /// Maximum live connections for one authenticated EndpointId.
    #[arg(long, default_value_t = 8)]
    max_connections_per_endpoint: usize,
    /// Maximum live creator/joiner connections for one Room.
    #[arg(long, default_value_t = 2)]
    max_connections_per_room: usize,
    /// Maximum live and tombstoned Room states.
    #[arg(long, default_value_t = 8192)]
    max_room_states: usize,
    /// Maximum parked creators.
    #[arg(long, default_value_t = 4096)]
    max_waiting_creators: usize,
    /// Maximum EndpointId, IP, and subnet limiter records combined.
    #[arg(long, default_value_t = 8192)]
    max_source_states: usize,
    /// Idle lifetime (seconds) for an unused source limiter record.
    #[arg(long, default_value_t = 600)]
    source_state_ttl: u64,
    /// Handshake and stream-open deadline in seconds.
    #[arg(long, default_value_t = 10)]
    handshake_timeout: u64,
    /// First Join-frame deadline in seconds.
    #[arg(long, default_value_t = 10)]
    join_timeout: u64,
    /// Hard lifetime of a matched relay in seconds.
    #[arg(long, default_value_t = 120)]
    relay_ttl: u64,
    /// Maximum idle time between post-match frames in seconds.
    #[arg(long, default_value_t = 30)]
    relay_idle_timeout: u64,
    /// Maximum time to read or write one post-match frame in seconds.
    #[arg(long, default_value_t = 10)]
    slow_frame_timeout: u64,
    /// Graceful transport close deadline in seconds.
    #[arg(long, default_value_t = 10)]
    close_grace: u64,
    /// Maximum Join or post-match frame body in bytes.
    #[arg(long, default_value_t = 64 * 1024)]
    max_frame_body: usize,
    /// Maximum retry_after value returned by the broker in seconds.
    #[arg(long, default_value_t = 300)]
    max_retry_after: u64,
    /// Retry guidance for temporarily unavailable creator/Room slots, in seconds.
    #[arg(long, default_value_t = 1)]
    unavailable_retry_after: u64,
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
    /// A non-loopback bind requires `--tls-cert` and `--tls-key`; plain HTTP is
    /// accepted only on loopback for local development or TLS proxying.
    #[arg(long)]
    log_bind: Option<SocketAddr>,
    /// How long (seconds) collected logs are kept after their last update.
    #[arg(long, default_value_t = 3600)]
    log_ttl: u64,
    /// File holding the bearer token required for diagnostic uploads
    /// (`POST /logs/<room>`). Without this option uploads are disabled. File
    /// input keeps the token out of argv and process listings.
    #[arg(long)]
    log_upload_token_file: Option<PathBuf>,
    /// File holding the operator bearer token that gates report RETRIEVAL
    /// (`GET /logs/<room>`). A room id is a low-entropy correlation key, not
    /// authorization. File (not argv) so the secret never leaks via `ps`.
    /// Without this AND without `--unsafe-open-log-view`, report retrieval is
    /// DISABLED (fail-closed).
    #[arg(long)]
    log_view_token_file: Option<PathBuf>,
    /// Explicitly allow ANONYMOUS report retrieval (no token). Reads become
    /// readable by anyone who can guess a room id — a deliberate, visible opt-in
    /// for a trusted/private deployment; never use for broad rollout.
    #[arg(long)]
    unsafe_open_log_view: bool,
    /// TLS certificate chain (PEM) for the log endpoint. With
    /// `--tls-key`, `--log-bind` serves HTTPS instead of plain HTTP. The PEM
    /// pair is re-read periodically, so ACME renewals that replace the files
    /// take effect without a restart (live rooms survive).
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// TLS private key (PEM); see `--tls-cert`.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
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
    let broker_config = cli.broker_config()?;
    cli.validate_log_transport()?;

    // Pin the process-level rustls provider before anything touches TLS; with
    // both iroh and axum-server in the tree the automatic choice is ambiguous.
    // (Err = already installed, which is fine.)
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Shared per-room log store: fed by the capture layer below, served by the
    // optional HTTP endpoint.
    let log_store = Arc::new(logs::RoomLogs::new(Duration::from_secs(cli.log_ttl)));

    // Include the broker crate (`envoix_rendezvous`) at info, not just the iroh
    // wiring - otherwise pairings/expiries (its target) fall to the global warn
    // default and never show.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "envoix_rendezvous=info,envoix_rendezvous_iroh=info,warn",
        )
    });
    // Only capture per-room events when the log endpoint is enabled.
    let capture = cli
        .log_bind
        .map(|_| logs::RoomCapture::new(log_store.clone()));
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    let registry = tracing_subscriber::registry().with(filter).with(capture);
    match cli.log_format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
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

    // Optional per-room log-collection endpoint on its own HTTP(S) port. Off
    // unless --log-bind is given; a separate task that never touches the
    // pairing endpoint. With --tls-cert/--tls-key it terminates TLS itself
    // itself. A loopback-only HTTP listener may instead sit behind a TLS proxy;
    // source rate limits then see the proxy as the peer and must also be applied
    // at that proxy.
    if let Some(addr) = cli.log_bind {
        let upload_auth = match &cli.log_upload_token_file {
            Some(path) => {
                logs::UploadAuth::Token(load_bearer_token(path, "--log-upload-token-file")?)
            }
            None => {
                tracing::warn!(
                    "diagnostic uploads (POST /logs/<room>) are DISABLED (fail-closed) — set \
                     --log-upload-token-file to enable authenticated uploads"
                );
                logs::UploadAuth::Closed
            }
        };
        // How report retrieval (GET) is gated. A token file wins; else the
        // explicit --unsafe-open-log-view opens it; else fail-CLOSED (default).
        let view_auth = match (&cli.log_view_token_file, cli.unsafe_open_log_view) {
            (Some(path), _) => {
                logs::ViewAuth::Token(load_bearer_token(path, "--log-view-token-file")?)
            }
            (None, true) => {
                tracing::warn!(
                    "report retrieval (GET /logs/<room>) is ANONYMOUS via \
                     --unsafe-open-log-view — a room id is enumerable; anyone can read logs"
                );
                logs::ViewAuth::Open
            }
            (None, false) => {
                tracing::warn!(
                    "report retrieval (GET /logs/<room>) is DISABLED (fail-closed) — set \
                     --log-view-token-file to enable authenticated reads, or \
                     --unsafe-open-log-view to allow anonymous reads"
                );
                logs::ViewAuth::Closed
            }
        };
        let router = logs::router(log_store.clone(), upload_auth, view_auth);
        if let (Some(cert), Some(key)) = (cli.tls_cert, cli.tls_key) {
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| format!("TLS cert {} / key {}", cert.display(), key.display()))?;
            // Re-read the PEM pair on a slow cadence: certbot replaces the
            // files on renewal, and reloading in place keeps live rooms up.
            {
                let config = config.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(12 * 3600));
                    tick.tick().await; // consume the immediate first tick
                    loop {
                        tick.tick().await;
                        if let Err(error) = config.reload_from_pem_file(&cert, &key).await {
                            tracing::warn!(%error, "TLS reload failed; serving previous cert");
                        }
                    }
                });
            }
            tokio::spawn(async move {
                tracing::info!(%addr, "log endpoint listening (https)");
                if let Err(error) = axum_server::bind_rustls(addr, config)
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                    .await
                {
                    tracing::error!(%error, "log endpoint failed");
                }
            });
        } else {
            tokio::spawn(async move {
                match tokio::net::TcpListener::bind(addr).await {
                    Ok(listener) => {
                        tracing::info!(%addr, "log endpoint listening (http)");
                        if let Err(error) = axum::serve(
                            listener,
                            router.into_make_service_with_connect_info::<SocketAddr>(),
                        )
                        .await
                        {
                            tracing::error!(%error, "log endpoint failed");
                        }
                    }
                    Err(error) => tracing::error!(%error, %addr, "log endpoint bind failed"),
                }
            });
        }
    }

    serve_endpoint(
        endpoint,
        Arc::new(
            RoomRegistry::with_config(broker_config)
                .map_err(|error| anyhow::anyhow!("invalid broker configuration: {error}"))?,
        ),
        locate,
    )
    .await
}

fn load_bearer_token(path: &Path, option: &str) -> Result<Arc<str>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {option} {}", path.display()))?;
    let token = raw.trim();
    if token.is_empty()
        || token.len() > 1024
        || !token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        anyhow::bail!(
            "{option} {} must contain 1..=1024 visible ASCII bytes",
            path.display()
        );
    }
    Ok(Arc::from(token))
}

impl Cli {
    fn validate_log_transport(&self) -> Result<()> {
        let Some(address) = self.log_bind else {
            return Ok(());
        };
        if self.tls_cert.is_none() && !address.ip().is_loopback() {
            anyhow::bail!(
                "a non-loopback --log-bind requires --tls-cert and --tls-key; plain HTTP is local-only"
            );
        }
        Ok(())
    }

    fn broker_config(&self) -> Result<BrokerConfig> {
        let config = BrokerConfig {
            room_ttl: Duration::from_secs(self.room_ttl),
            room_tombstone_ttl: Duration::from_secs(self.room_tombstone_ttl),
            room_attempt_limit: self.room_attempt_limit,
            room_attempt_rate: RateLimitConfig {
                events: self.room_rate_events,
                period: Duration::from_secs(self.room_rate_period),
                burst: self.room_rate_burst,
            },
            endpoint_join_rate: RateLimitConfig {
                events: self.endpoint_rate_events,
                period: Duration::from_secs(self.endpoint_rate_period),
                burst: self.endpoint_rate_burst,
            },
            ip_join_rate: RateLimitConfig {
                events: self.ip_rate_events,
                period: Duration::from_secs(self.ip_rate_period),
                burst: self.ip_rate_burst,
            },
            subnet_join_rate: RateLimitConfig {
                events: self.subnet_rate_events,
                period: Duration::from_secs(self.subnet_rate_period),
                burst: self.subnet_rate_burst,
            },
            max_connections: self.max_connections,
            max_connections_per_endpoint: self.max_connections_per_endpoint,
            max_connections_per_room: self.max_connections_per_room,
            max_room_states: self.max_room_states,
            max_waiting_creators: self.max_waiting_creators,
            max_source_states: self.max_source_states,
            source_state_ttl: Duration::from_secs(self.source_state_ttl),
            handshake_timeout: Duration::from_secs(self.handshake_timeout),
            join_timeout: Duration::from_secs(self.join_timeout),
            relay_ttl: Duration::from_secs(self.relay_ttl),
            relay_idle_timeout: Duration::from_secs(self.relay_idle_timeout),
            slow_frame_timeout: Duration::from_secs(self.slow_frame_timeout),
            close_grace: Duration::from_secs(self.close_grace),
            max_frame_body: self.max_frame_body,
            max_retry_after: Duration::from_secs(self.max_retry_after),
            unavailable_retry_after: Duration::from_secs(self.unavailable_retry_after),
        };
        config
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid broker configuration: {error}"))?;
        Ok(config)
    }
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
mod tests;
