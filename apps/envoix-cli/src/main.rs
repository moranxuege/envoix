use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

mod render;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use envoix_client::api;
use envoix_client::api::TransferError;
use envoix_client::{
    BindAddrs, IdentityConfig, PeerDescriptor, SPAKE2_EXPERIMENTAL_WARNING, TransferSummary,
};
use render::EventOutput;

const IPV4_RECEIVE_ADDR: &str = "0.0.0.0:0";
const IPV6_RECEIVE_ADDR: &str = "[::]:0";
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
    /// Emit lifecycle events as JSON lines on stdout instead of human
    /// rendering (progress lines stay off; contextual notes stay on stderr).
    #[arg(long, global = true)]
    json: bool,
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

async fn run(cli: Cli) -> Result<(), TransferError> {
    let event_output = EventOutput::new(cli.json);
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
                run_transfer(transfer, event_output.clone()).await?
            } else if let Some(invite_str) = invite {
                let client = api_client(config.as_deref(), identity_config(identity))?;
                let transfer = client.send(
                    file,
                    api::PeerSource::Invite { invite: invite_str },
                    send_options(resume),
                )?;
                run_transfer(transfer, event_output.clone()).await?
            } else if enable_mdns {
                if peer.is_some() {
                    return Err(TransferError::input(
                        "use either --enable-mdns or --peer, not both",
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
                run_transfer(transfer, event_output.clone()).await?
            } else {
                let peer = peer.ok_or_else(|| {
                    TransferError::input(
                        "send requires --peer unless --enable-mdns or --invite is set",
                    )
                })?;
                let token = token.expect("clap ensures --token is present without --invite");
                let client = api_client(config.as_deref(), identity_config(identity))?;
                let transfer = client.send(
                    file,
                    api::PeerSource::Manual { peer, token },
                    send_options(resume),
                )?;
                run_transfer(transfer, event_output.clone()).await?
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
                run_transfer(transfer, event_output.clone()).await?
            } else if enable_mdns {
                let client = api_client(config.as_deref(), identity)?;
                eprintln!("waiting for sender...");
                let transfer = client.receive(
                    output,
                    api::PeerSource::Mdns { token },
                    receive_options(listen_addrs),
                )?;
                run_transfer(transfer, event_output.clone()).await?
            } else {
                let token = token.expect("clap requires --token unless --enable-mdns is set");
                let client = api_client(config.as_deref(), identity)?;
                let transfer = client.receive(
                    output,
                    api::PeerSource::ShowManual { token: Some(token) },
                    receive_options(listen_addrs),
                )?;
                run_transfer(transfer, event_output.clone()).await?
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

/// Builds the new-API client from the CLI's config/identity arguments.
fn api_client(
    config_path: Option<&std::path::Path>,
    identity: IdentityConfig,
) -> Result<api::Client, TransferError> {
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
    mut renderer: EventOutput,
) -> Result<TransferSummary, TransferError> {
    let interrupted = tokio::select! {
        _ = drain_events(&mut transfer, &mut renderer) => false,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| {
                TransferError::input(format!("failed to listen for interrupt signal: {error}"))
            })?;
            true
        }
    };
    if interrupted {
        eprintln!("interrupt received; shutting down (Ctrl-C again to force)...");
        transfer.cancel();
        tokio::select! {
            _ = drain_events(&mut transfer, &mut renderer) => {}
            _ = tokio::signal::ctrl_c() => return Err(TransferError::cancelled(transfer.phase())),
            _ = tokio::time::sleep(SHUTDOWN_GRACE) => return Err(TransferError::cancelled(transfer.phase())),
        }
    }
    transfer.wait().await
}

async fn drain_events(transfer: &mut api::Transfer, renderer: &mut EventOutput) {
    while let Some(event) = transfer.next_event().await {
        renderer.render(event);
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
