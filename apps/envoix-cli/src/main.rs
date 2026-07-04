use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

mod args;
mod render;

use args::{Cli, Command, IpVersion, ReceiveArgs, SendArgs};
use clap::Parser;
use envoix_client::api;
use envoix_client::api::TransferError;
use envoix_client::{BindAddrs, IdentityConfig, SPAKE2_EXPERIMENTAL_WARNING, TransferSummary};
use render::EventOutput;

const IPV4_RECEIVE_ADDR: &str = "0.0.0.0:0";
const IPV6_RECEIVE_ADDR: &str = "[::]:0";
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
        Command::Send(SendArgs {
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
        }) => {
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
        Command::Receive(ReceiveArgs {
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
        }) => {
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
