use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use envoix_client::api;
use envoix_client::{
    BindAddrs, IdentityConfig, PeerDescriptor, SPAKE2_EXPERIMENTAL_WARNING, TransferDirection,
    TransferSummary,
};
use envoix_qr::render_terminal_qr;

const IPV4_RECEIVE_ADDR: &str = "0.0.0.0:0";
const IPV6_RECEIVE_ADDR: &str = "[::]:0";
const PROGRESS_RENDER_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Parser)]
#[command(
    name = "envoix",
    version,
    about = "Secure file transfer CLI",
    after_help = "Manual flow:
    envoix receive --output ./received --token <token>
    envoix send --peer <endpoint-id>@<receiver-ip>:<port> --token <token> <file>

QR flow (no manual token or address needed):
    envoix receive --enable-mdns --output ./received
    envoix send --invite <invite-string> <file>
"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send one file to a receiver address printed by `envoix receive`.
    Send {
        /// Receiver peer descriptor (manual mode). Cannot be combined with --invite.
        #[arg(long, conflicts_with = "invite")]
        peer: Option<PeerDescriptor>,
        /// Explicit TOML config file path.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Use iroh mDNS discovery when available. Cannot be combined with --invite.
        #[arg(long, conflicts_with = "invite")]
        enable_mdns: bool,
        /// Persistent iroh identity file. Created if missing.
        #[arg(long, conflicts_with = "ephemeral_identity")]
        identity: Option<PathBuf>,
        /// Use a fresh iroh identity for this run.
        #[arg(long)]
        ephemeral_identity: bool,
        /// Start a new transfer and ignore compatible receiver-side resume state.
        #[arg(long = "fresh", action = ArgAction::SetFalse, default_value_t = true)]
        resume: bool,
        /// Shared ASCII pairing token (>=12 bytes). Required unless --invite or --room is set.
        #[arg(long, required_unless_present_any = ["invite", "room"], conflicts_with = "invite")]
        token: Option<String>,
        /// Invite string printed by `envoix receive --enable-mdns`; sets peer and token automatically.
        #[arg(long, conflicts_with_all = ["peer", "enable_mdns", "token"])]
        invite: Option<String>,
        /// Pairing code for a rendezvous-room transfer, e.g. 123456-amber-comet.
        #[arg(long, requires = "rendezvous", conflicts_with_all = ["peer", "invite", "enable_mdns", "token"])]
        room: Option<String>,
        /// Rendezvous broker address, <endpoint-id>@<ip:port> (used with --room).
        #[arg(long, requires = "room")]
        rendezvous: Option<String>,
        /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
        #[arg(long, requires = "room")]
        relay: Option<String>,
        /// Force the data path through the relay (no direct/holepunch). For A/B
        /// testing relay vs direct; requires --relay.
        #[arg(long, requires = "relay")]
        relay_only: bool,
        /// Force a direct data path: no relay fallback for the transfer (the
        /// relay is still used to reach the broker). Direct-or-fail.
        #[arg(long, requires = "room", conflicts_with = "relay_only")]
        direct_only: bool,
        /// File to send.
        file: PathBuf,
    },
    /// Receive one file into an output directory.
    Receive {
        /// Directory where the received file and resume state are stored.
        #[arg(long)]
        output: PathBuf,
        /// Explicit TOML config file path.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Enable iroh mDNS/address discovery. When used without
        /// --token, generates a random token and prints a QR invite.
        #[arg(long, visible_alias = "auto")]
        enable_mdns: bool,
        /// Persistent iroh identity file. Created if missing.
        #[arg(long, conflicts_with = "ephemeral_identity")]
        identity: Option<PathBuf>,
        /// Use a fresh iroh identity for this run.
        #[arg(long)]
        ephemeral_identity: bool,
        /// Shared ASCII pairing token (>=12 bytes). Required unless --enable-mdns or --room is set.
        #[arg(long, required_unless_present_any = ["enable_mdns", "room"])]
        token: Option<String>,
        /// Pairing code for a rendezvous-room transfer, e.g. 123456-amber-comet.
        #[arg(long, requires = "rendezvous", conflicts_with_all = ["token", "enable_mdns"])]
        room: Option<String>,
        /// Rendezvous broker address, <endpoint-id>@<ip:port> (used with --room).
        #[arg(long, requires = "room")]
        rendezvous: Option<String>,
        /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
        #[arg(long, requires = "room")]
        relay: Option<String>,
        /// Force the data path through the relay (no direct/holepunch). For A/B
        /// testing relay vs direct; requires --relay.
        #[arg(long, requires = "relay")]
        relay_only: bool,
        /// Force a direct data path: no relay fallback for the transfer (the
        /// relay is still used to reach the broker). Direct-or-fail.
        #[arg(long, requires = "room", conflicts_with = "relay_only")]
        direct_only: bool,
        /// Address family to bind for receiving.
        #[arg(long, value_enum, default_value_t = IpVersion::Dual)]
        ip_version: IpVersion,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum IpVersion {
    Dual,
    Ipv4,
    Ipv6,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Initialize the tracing subscriber.  Honors `RUST_LOG`, defaulting to `info`
/// for the `envoix` target and `warn` for everything else, so library warnings
/// reach the terminal and iroh internals stay quiet without flooding it. The
/// per-transfer "data path" line is rendered from Connected/PathChanged events,
/// not tracing. Output goes to stderr to keep stdout clean for machine-readable
/// formats.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("envoix=info,warn"));
    fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_target(false)
        .init();
}

async fn run(cli: Cli) -> Result<(), envoix_client::PublicError> {
    match cli.command {
        Command::Send {
            peer,
            config,
            enable_mdns,
            identity,
            ephemeral_identity: _,
            resume,
            token,
            invite,
            file,
            room,
            rendezvous,
            relay,
            relay_only,
            direct_only,
        } => {
            let summary = if let Some(code) = room {
                let rendezvous = rendezvous.expect("clap requires --rendezvous with --room");
                let client = api_client(config.as_deref(), identity_config(identity))?;
                eprintln!("pairing in room via {rendezvous}...");
                let mut options = send_options(resume);
                options.relay = relay;
                options.path = path_policy(relay_only, direct_only);
                let transfer = client.send(
                    file,
                    api::PeerSource::Room {
                        code,
                        broker: rendezvous,
                    },
                    options,
                )?;
                run_transfer(transfer).await?
            } else if let Some(invite_str) = invite {
                let client = api_client(config.as_deref(), identity_config(identity))?;
                let transfer = client.send(
                    file,
                    api::PeerSource::Invite { invite: invite_str },
                    send_options(resume),
                )?;
                run_transfer(transfer).await?
            } else if enable_mdns {
                if peer.is_some() {
                    return Err(envoix_client::PublicError::InvalidInput(
                        "use either --enable-mdns or --peer, not both".into(),
                    ));
                }
                let token = token.expect("clap ensures --token is present with --enable-mdns");
                let client = api_client(config.as_deref(), identity_config(identity))?;
                eprintln!("discovering receiver over mDNS...");
                let transfer = client.send(
                    file,
                    api::PeerSource::Mdns { token: Some(token) },
                    send_options(resume),
                )?;
                run_transfer(transfer).await?
            } else {
                let peer = peer.ok_or_else(|| {
                    envoix_client::PublicError::InvalidInput(
                        "send requires --peer unless --enable-mdns or --invite is set".into(),
                    )
                })?;
                let token = token.expect("clap ensures --token is present without --invite");
                let client = api_client(config.as_deref(), identity_config(identity))?;
                let transfer = client.send(
                    file,
                    api::PeerSource::Manual { peer, token },
                    send_options(resume),
                )?;
                run_transfer(transfer).await?
            };
            eprintln!(
                "sent {} bytes from {}",
                summary.bytes_transferred, summary.file_name
            );
        }
        Command::Receive {
            output,
            config,
            enable_mdns,
            identity,
            ephemeral_identity: _,
            token,
            ip_version,
            room,
            rendezvous,
            relay,
            relay_only,
            direct_only,
        } => {
            let listen_addrs = receive_addrs_for(ip_version);
            let identity = identity_config(identity);
            let summary = if let Some(code) = room {
                let rendezvous = rendezvous.expect("clap requires --rendezvous with --room");
                let client = api_client(config.as_deref(), identity)?;
                eprintln!("waiting for sender via rendezvous {rendezvous}...");
                let mut options = receive_options(listen_addrs);
                options.relay = relay;
                options.path = path_policy(relay_only, direct_only);
                let transfer = client.receive(
                    output,
                    api::PeerSource::Room {
                        code,
                        broker: rendezvous,
                    },
                    options,
                )?;
                run_transfer(transfer).await?
            } else if enable_mdns {
                let client = api_client(config.as_deref(), identity)?;
                eprintln!("waiting for sender...");
                let transfer = client.receive(
                    output,
                    api::PeerSource::Mdns { token },
                    receive_options(listen_addrs),
                )?;
                run_transfer(transfer).await?
            } else {
                let token = token.expect("clap requires --token unless --enable-mdns is set");
                let client = api_client(config.as_deref(), identity)?;
                let transfer = client.receive(
                    output,
                    api::PeerSource::ShowManual { token: Some(token) },
                    receive_options(listen_addrs),
                )?;
                run_transfer(transfer).await?
            };
            eprintln!(
                "received {} bytes into {}",
                summary.bytes_transferred, summary.file_name
            );
        }
    }

    Ok(())
}

/// How long a first Ctrl-C waits for a clean shutdown before forcing exit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Error used when an interrupt forces exit before the operation finished.
fn interrupted_error() -> envoix_client::PublicError {
    envoix_client::PublicError::Transfer("interrupted before completion".into())
}

/// Builds the new-API client from the CLI's config/identity arguments.
fn api_client(
    config_path: Option<&std::path::Path>,
    identity: IdentityConfig,
) -> Result<api::Client, envoix_client::PublicError> {
    eprintln!("{SPAKE2_EXPERIMENTAL_WARNING}");
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = identity;
    Ok(client)
}

fn send_options(resume: bool) -> api::TransferOptions {
    let mut options = api::TransferOptions::default();
    options.resume = resume;
    options
}

fn receive_options(listen_addrs: BindAddrs) -> api::TransferOptions {
    let mut options = api::TransferOptions::default();
    options.listen_addrs = Some(listen_addrs);
    options
}

fn path_policy(relay_only: bool, direct_only: bool) -> api::PathPolicy {
    if relay_only {
        api::PathPolicy::RelayOnly
    } else if direct_only {
        api::PathPolicy::DirectOnly
    } else {
        api::PathPolicy::Auto
    }
}

/// Drives a new-API transfer to completion: renders its event stream and
/// handles Ctrl-C (first press cancels gracefully; a second press or the
/// grace period elapsing forces exit).
async fn run_transfer(
    mut transfer: api::Transfer,
) -> Result<TransferSummary, envoix_client::PublicError> {
    let mut renderer = Renderer::default();
    let interrupted = tokio::select! {
        _ = drain_events(&mut transfer, &mut renderer) => false,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| {
                envoix_client::PublicError::Transfer(format!(
                    "failed to listen for interrupt signal: {error}"
                ))
            })?;
            true
        }
    };
    if interrupted {
        eprintln!("interrupt received; shutting down (Ctrl-C again to force)...");
        transfer.cancel();
        tokio::select! {
            _ = drain_events(&mut transfer, &mut renderer) => {}
            _ = tokio::signal::ctrl_c() => return Err(interrupted_error()),
            _ = tokio::time::sleep(SHUTDOWN_GRACE) => return Err(interrupted_error()),
        }
    }
    transfer.wait().await
}

async fn drain_events(transfer: &mut api::Transfer, renderer: &mut Renderer) {
    while let Some(event) = transfer.next_event().await {
        renderer.render(event);
    }
}

/// Renders the unified event stream to the terminal. Single-task use, so no
/// locking - unlike the legacy sink, which was called from library threads.
#[derive(Debug, Default)]
struct Renderer {
    progress: Option<ProgressState>,
}

impl Renderer {
    fn render(&mut self, event: api::TransferEvent) {
        use api::TransferEvent as E;
        match event {
            // Contextual lines (which mode, which broker) are printed by the
            // dispatch site that knows the arguments.
            E::Binding { .. } | E::Pairing => {}
            E::Connected { path } | E::PathChanged { path } => {
                eprintln!("data path: {path}");
            }
            E::Advertised {
                peer,
                token,
                invite,
            } => {
                eprintln!("peer: {peer}");
                if let Some(invite) = invite {
                    eprintln!("\ninvite: {invite}");
                    if let Some(qr) = render_terminal_qr(&invite) {
                        eprintln!("{qr}");
                    }
                } else if let Some(token) = token {
                    eprintln!("token: {token}");
                }
            }
            E::Started {
                direction,
                file_name,
                total_bytes,
                bytes_resumed,
                ..
            } => {
                let state = ProgressState {
                    file_name,
                    direction,
                    total_bytes,
                    bytes_resumed,
                    started_at: Instant::now(),
                    last_rendered_at: Instant::now(),
                };
                render_progress_line(&state, bytes_resumed, false);
                self.progress = Some(state);
            }
            E::Progress {
                bytes_transferred, ..
            } => {
                if let Some(state) = self.progress.as_mut()
                    && state.last_rendered_at.elapsed() >= PROGRESS_RENDER_INTERVAL
                {
                    render_progress_line(state, bytes_transferred, false);
                    state.last_rendered_at = Instant::now();
                }
            }
            E::Verifying {
                direction,
                file_name,
                bytes_to_hash,
                ..
            } => render_hash_line(direction, &file_name, bytes_to_hash, false),
            E::Verified {
                direction,
                file_name,
                bytes_hashed,
                ..
            } => render_hash_line(direction, &file_name, bytes_hashed, true),
            E::Completed {
                bytes_transferred, ..
            } => match self.progress.take() {
                Some(state) => render_progress_line(&state, bytes_transferred, true),
                None => eprintln!("completed {bytes_transferred} bytes"),
            },
            E::Failed { direction, reason } => match self.progress.take() {
                Some(state) => render_transfer_failure_line(&state, &reason),
                None => render_attempt_failure_line(direction, &reason),
            },
            // The event enum is non_exhaustive; render nothing for variants
            // this build does not know.
            _ => {}
        }
    }
}

fn receive_addrs_for(ip_version: IpVersion) -> BindAddrs {
    match ip_version {
        IpVersion::Dual => BindAddrs::dual_stack(0),
        IpVersion::Ipv4 => BindAddrs::single(
            IPV4_RECEIVE_ADDR
                .parse()
                .expect("default IPv4 address is valid"),
        ),
        IpVersion::Ipv6 => BindAddrs::single(
            IPV6_RECEIVE_ADDR
                .parse()
                .expect("default IPv6 address is valid"),
        ),
    }
}

fn identity_config(path: Option<PathBuf>) -> IdentityConfig {
    path.map(IdentityConfig::Persistent)
        .unwrap_or(IdentityConfig::Ephemeral)
}

#[derive(Debug)]
struct ProgressState {
    file_name: String,
    direction: TransferDirection,
    total_bytes: u64,
    bytes_resumed: u64,
    started_at: Instant,
    last_rendered_at: Instant,
}

fn render_hash_line(direction: TransferDirection, file_name: &str, bytes_hashed: u64, done: bool) {
    let verb = match direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let status = if done { "verified" } else { "verifying" };
    let line = format!(
        "{:<24} {:>9} {}",
        format!("{verb} {}", display_file_name(file_name)),
        format_bytes(bytes_hashed),
        status,
    );

    let mut stderr = io::stderr().lock();
    if done {
        let _ = writeln!(stderr, "\r{line:<80}");
    } else {
        let _ = write!(stderr, "\r{line:<80}");
        let _ = stderr.flush();
    }
}

fn render_transfer_failure_line(state: &ProgressState, reason: &str) {
    let verb = match state.direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let line = format!(
        "{:<24} failed: {}",
        format!("{verb} {}", display_file_name(&state.file_name)),
        reason
    );
    eprintln!("\r{line:<80}");
}

fn render_attempt_failure_line(direction: TransferDirection, reason: &str) {
    let verb = match direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    eprintln!("{verb} attempt failed: {reason}");
}

fn render_progress_line(state: &ProgressState, bytes_transferred: u64, done: bool) {
    let percent = bytes_transferred
        .saturating_mul(100)
        .checked_div(state.total_bytes)
        .unwrap_or(100);
    let elapsed = state.started_at.elapsed();
    let bytes_this_attempt = bytes_transferred.saturating_sub(state.bytes_resumed);
    let bytes_per_second = if elapsed.is_zero() {
        0.0
    } else {
        bytes_this_attempt as f64 / elapsed.as_secs_f64()
    };
    let eta = eta(
        bytes_transferred,
        state.total_bytes,
        bytes_this_attempt,
        bytes_per_second,
    );
    let verb = match state.direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let line = format!(
        "{:<24} {:>4}% {:>9}/{:<9} {:>10}/s {:>5}",
        format!("{verb} {}", display_file_name(&state.file_name)),
        percent.min(100),
        format_bytes(bytes_transferred),
        format_bytes(state.total_bytes),
        format_bytes(bytes_per_second as u64),
        eta,
    );

    let mut stderr = io::stderr().lock();
    if done {
        let _ = writeln!(stderr, "\r{line:<80}");
    } else {
        let _ = write!(stderr, "\r{line:<80}");
        let _ = stderr.flush();
    }
}

fn display_file_name(file_name: &str) -> String {
    const MAX_LEN: usize = 19;

    if file_name.chars().count() <= MAX_LEN {
        return file_name.to_owned();
    }

    let suffix: String = file_name
        .chars()
        .rev()
        .take(MAX_LEN - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("~{suffix}")
}

fn eta(
    bytes_transferred: u64,
    total_bytes: u64,
    bytes_this_attempt: u64,
    bytes_per_second: f64,
) -> String {
    if bytes_transferred >= total_bytes {
        return "00:00".into();
    }
    if bytes_this_attempt == 0 || bytes_per_second <= 0.0 {
        return "--:--".into();
    }

    let remaining = total_bytes - bytes_transferred;
    format_duration(Duration::from_secs_f64(remaining as f64 / bytes_per_second))
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes}B")
    } else if value < 10.0 {
        format!("{value:.1}{unit}")
    } else {
        format!("{value:.0}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "peer@[::1]:9000";

    #[test]
    fn parses_send_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "send",
            "--peer",
            PEER,
            "--token",
            "abcdefghijkl",
            "hello.txt",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send {
                peer,
                config: None,
                enable_mdns,
                resume,
                ref token,
                invite: None,
                ref file,
                ..
            } if peer == Some(PEER.parse().unwrap())
                && !enable_mdns
                && resume
                && token.as_deref() == Some("abcdefghijkl")
                && file == std::path::Path::new("hello.txt")
        ));
    }

    #[test]
    fn parses_send_enable_mdns_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "send",
            "--enable-mdns",
            "--token",
            "abcdefghijkl",
            "hello.txt",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send {
                peer: None,
                config: None,
                enable_mdns: true,
                resume: true,
                ref token,
                invite: None,
                ref file,
                ..
            } if token.as_deref() == Some("abcdefghijkl")
                && file == std::path::Path::new("hello.txt")
        ));
    }

    #[test]
    fn parses_send_fresh_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "send",
            "--peer",
            PEER,
            "--fresh",
            "--token",
            "abcdefghijkl",
            "hello.txt",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::Send { resume: false, .. }));
    }

    #[test]
    fn parses_receive_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--output",
            "received",
            "--token",
            "abcdefghijkl",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                output,
                config: None,
                enable_mdns,
                token: Some(ref token),
                ip_version,
                ..
            } if output == std::path::Path::new("received")
                && !enable_mdns
                && token == "abcdefghijkl"
                && ip_version == IpVersion::Dual
        ));
    }

    #[test]
    fn parses_receive_ipv4() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--output",
            "received",
            "--token",
            "abcdefghijkl",
            "--ip-version",
            "ipv4",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                ip_version: IpVersion::Ipv4,
                ..
            }
        ));
    }

    #[test]
    fn parses_receive_enable_mdns_command() {
        let cli =
            Cli::try_parse_from(["envoix", "receive", "--enable-mdns", "--output", "received"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                output,
                enable_mdns: true,
                token: None,
                ..
            } if output == std::path::Path::new("received")
        ));
    }

    #[test]
    fn parses_receive_auto_alias() {
        let cli =
            Cli::try_parse_from(["envoix", "receive", "--auto", "--output", "received"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                enable_mdns: true,
                ..
            }
        ));
    }

    #[test]
    fn parses_receive_with_explicit_token() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--output",
            "received",
            "--token",
            "abcdefghijkl",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                enable_mdns: false,
                token: Some(ref t),
                ..
            } if t == "abcdefghijkl"
        ));
    }

    #[test]
    fn parses_send_room_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "send",
            "--room",
            "123456-amber-comet",
            "--rendezvous",
            "id@1.2.3.4:8445",
            "hello.txt",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send { room: Some(ref r), rendezvous: Some(ref rv), .. }
                if r == "123456-amber-comet" && rv == "id@1.2.3.4:8445"
        ));
    }

    #[test]
    fn send_room_requires_rendezvous() {
        let result = Cli::try_parse_from([
            "envoix",
            "send",
            "--room",
            "123456-amber-comet",
            "hello.txt",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn send_room_conflicts_with_token() {
        let result = Cli::try_parse_from([
            "envoix",
            "send",
            "--room",
            "123456-amber-comet",
            "--rendezvous",
            "id@1.2.3.4:8445",
            "--token",
            "abcdefghijkl",
            "hello.txt",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_receive_room_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--output",
            "received",
            "--room",
            "123456-amber-comet",
            "--rendezvous",
            "id@1.2.3.4:8445",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive { room: Some(ref r), .. } if r == "123456-amber-comet"
        ));
    }

    #[test]
    fn parses_receive_ipv6() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--output",
            "received",
            "--token",
            "abcdefghijkl",
            "--ip-version",
            "ipv6",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                ip_version: IpVersion::Ipv6,
                ..
            }
        ));
    }

    #[test]
    fn parses_send_invite_command() {
        let cli = Cli::try_parse_from(["envoix", "send", "--invite", "envoix:dGVzdA", "hello.txt"])
            .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send {
                peer: None,
                config: None,
                enable_mdns: false,
                resume: true,
                token: None,
                ref invite,
                ref file,
                ..
            } if invite.as_deref() == Some("envoix:dGVzdA")
                && file == std::path::Path::new("hello.txt")
        ));
    }

    #[test]
    fn parses_explicit_config_path() {
        let cli = Cli::try_parse_from([
            "envoix",
            "send",
            "--peer",
            PEER,
            "--config",
            "envoix.toml",
            "--token",
            "abcdefghijkl",
            "hello.txt",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send {
                config,
                ..
            } if config == Some(std::path::PathBuf::from("envoix.toml"))
        ));
    }

    #[test]
    fn rejects_missing_token() {
        let error =
            Cli::try_parse_from(["envoix", "send", "--peer", PEER, "hello.txt"]).unwrap_err();

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn receive_enable_mdns_with_token_is_valid() {
        assert!(
            Cli::try_parse_from([
                "envoix",
                "receive",
                "--enable-mdns",
                "--output",
                "recv",
                "--token",
                "abcdefghijkl",
            ])
            .is_ok()
        );
    }

    #[test]
    fn receive_enable_mdns_without_token_is_valid() {
        assert!(
            Cli::try_parse_from(["envoix", "receive", "--enable-mdns", "--output", "recv",])
                .is_ok()
        );
    }

    #[test]
    fn rejects_send_invite_with_peer() {
        assert!(
            Cli::try_parse_from([
                "envoix",
                "send",
                "--invite",
                "envoix:dGVzdA",
                "--peer",
                PEER,
                "f.txt",
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_send_invite_with_token() {
        assert!(
            Cli::try_parse_from([
                "envoix",
                "send",
                "--invite",
                "envoix:dGVzdA",
                "--token",
                "abcdefghijkl",
                "f.txt",
            ])
            .is_err()
        );
    }
}
