//! Command-line arguments: clap definitions and parse tests.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use envoix_client::api;
use envoix_client::api::TransferError;
use envoix_client::{BindAddrs, IdentityConfig, PeerDescriptor};
use envoix_qr::render_terminal_qr;

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
pub(crate) struct Cli {
    /// Emit lifecycle events as JSON lines on stdout instead of human
    /// rendering (progress lines stay off; contextual notes stay on stderr).
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Increase log verbosity: -v shows envoix internals, -vv adds iroh
    /// internals (path selection, hole-punching). RUST_LOG overrides both.
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    pub(crate) verbose: u8,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Send one file to a receiver address printed by `envoix receive`.
    Send(SendArgs),
    /// Receive one file into an output directory.
    Receive(ReceiveArgs),
}

/// Arguments for `envoix send`.
#[derive(Args, Debug)]
pub(crate) struct SendArgs {
    /// Receiver peer descriptor (manual mode). Cannot be combined with --invite.
    #[arg(long, conflicts_with = "invite")]
    pub(crate) peer: Option<PeerDescriptor>,
    /// Explicit TOML config file path.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
    /// Use iroh mDNS discovery when available. Cannot be combined with --invite.
    #[arg(long, conflicts_with = "invite")]
    pub(crate) enable_mdns: bool,
    /// Persistent iroh identity file. Created if missing.
    #[arg(long, conflicts_with = "ephemeral_identity")]
    pub(crate) identity: Option<PathBuf>,
    /// Use a fresh iroh identity for this run.
    #[arg(long)]
    pub(crate) ephemeral_identity: bool,
    /// Start a new transfer and ignore compatible receiver-side resume state.
    #[arg(long = "fresh", action = ArgAction::SetFalse, default_value_t = true)]
    pub(crate) resume: bool,
    /// Shared ASCII pairing token (>=12 bytes). Required unless --invite or --room is set.
    #[arg(long, required_unless_present_any = ["invite", "room"], conflicts_with = "invite")]
    pub(crate) token: Option<String>,
    /// Invite string printed by `envoix receive --enable-mdns`; sets peer and token automatically.
    #[arg(long, conflicts_with_all = ["peer", "enable_mdns", "token"])]
    pub(crate) invite: Option<String>,
    /// Pairing code for a rendezvous-room transfer, e.g. 123456-amber-comet.
    /// Pass `--room` with no value to auto-generate a code (printed with a QR)
    /// for the other side to use.
    #[arg(long, num_args = 0..=1, requires = "rendezvous", conflicts_with_all = ["peer", "invite", "enable_mdns", "token"])]
    pub(crate) room: Option<Option<String>>,
    /// Rendezvous broker address, <endpoint-id>@<ip:port> (used with --room).
    #[arg(long, requires = "room")]
    pub(crate) rendezvous: Option<String>,
    /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
    #[arg(long, requires = "room")]
    pub(crate) relay: Option<String>,
    /// Force the data path through the relay (no direct/holepunch). For A/B
    /// testing relay vs direct; requires --relay.
    #[arg(long, requires = "relay")]
    pub(crate) relay_only: bool,
    /// (Temporarily disabled - see docs/design/client-api.md 5.5.) A relay-free
    /// direct path between two NATed peers is not achievable: iroh's
    /// hole-punching runs over a connection that must first be established via
    /// the relay, so removing the relay removes the punch itself.
    #[arg(long, requires = "room", conflicts_with = "relay_only", hide = true)]
    pub(crate) direct_only: bool,
    /// First file or folder to send.
    pub(crate) file: PathBuf,
    /// Additional files or folders. Every positional source becomes a root in
    /// the same canonical transfer job.
    #[arg(num_args = 0..)]
    pub(crate) additional_files: Vec<PathBuf>,
    /// Compression policy frozen into the job at Send.
    #[arg(long, value_enum, default_value_t = CompressionPolicyArg::Smart)]
    pub(crate) compression: CompressionPolicyArg,
    /// How to resolve unreadable descendants discovered before Send.
    #[arg(long, value_enum, default_value_t = SourceIssueActionArg::Fail)]
    pub(crate) source_issue_action: SourceIssueActionArg,
}

/// Arguments for `envoix receive`.
#[derive(Args, Debug)]
pub(crate) struct ReceiveArgs {
    /// Directory where the received file and resume state are stored.
    #[arg(long)]
    pub(crate) output: PathBuf,
    /// Explicit TOML config file path.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
    /// Enable iroh mDNS/address discovery. When used without
    /// --token, generates a random token and prints a QR invite.
    #[arg(long, visible_alias = "auto")]
    pub(crate) enable_mdns: bool,
    /// Persistent iroh identity file. Created if missing.
    #[arg(long, conflicts_with = "ephemeral_identity")]
    pub(crate) identity: Option<PathBuf>,
    /// Use a fresh iroh identity for this run.
    #[arg(long)]
    pub(crate) ephemeral_identity: bool,
    /// Shared ASCII pairing token (>=12 bytes). Required unless --enable-mdns or --room is set.
    #[arg(long, required_unless_present_any = ["enable_mdns", "room"])]
    pub(crate) token: Option<String>,
    /// Pairing code for a rendezvous-room transfer, e.g. 123456-amber-comet.
    /// Pass `--room` with no value to auto-generate a code (printed with a QR)
    /// for the other side to use.
    #[arg(long, num_args = 0..=1, requires = "rendezvous", conflicts_with_all = ["token", "enable_mdns"])]
    pub(crate) room: Option<Option<String>>,
    /// Rendezvous broker address, <endpoint-id>@<ip:port> (used with --room).
    #[arg(long, requires = "room")]
    pub(crate) rendezvous: Option<String>,
    /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
    #[arg(long, requires = "room")]
    pub(crate) relay: Option<String>,
    /// Force the data path through the relay (no direct/holepunch). For A/B
    /// testing relay vs direct; requires --relay.
    #[arg(long, requires = "relay")]
    pub(crate) relay_only: bool,
    /// (Temporarily disabled - see docs/design/client-api.md 5.5.) A relay-free
    /// direct path between two NATed peers is not achievable: iroh's
    /// hole-punching runs over a connection that must first be established via
    /// the relay, so removing the relay removes the punch itself.
    #[arg(long, requires = "room", conflicts_with = "relay_only", hide = true)]
    pub(crate) direct_only: bool,
    /// Address family to bind for receiving.
    #[arg(long, value_enum, default_value_t = IpVersion::Dual)]
    pub(crate) ip_version: IpVersion,
    /// Explicitly approve an offer larger than the automatic receive limit or
    /// more than half of currently allocatable destination space.
    #[arg(long)]
    pub(crate) approve_large_transfer: bool,
    /// Save directly on the destination storage, or explicitly accept a
    /// verified second copy and its additional peak-space cost.
    #[arg(long, value_enum, default_value_t = SaveModeArg::Direct)]
    pub(crate) save_mode: SaveModeArg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum IpVersion {
    Dual,
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CompressionPolicyArg {
    Never,
    Always,
    Smart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum SourceIssueActionArg {
    Fail,
    ApprovePartial,
    RemoveRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum SaveModeArg {
    Direct,
    CopyAfterVerify,
}

/// Everything `run` needs to start a transfer, derived purely from the
/// parsed arguments - no I/O, so the args -> behavior mapping is testable.
pub(crate) struct TransferPlan {
    /// File to send, or directory to receive into.
    pub(crate) path: PathBuf,
    pub(crate) additional_paths: Vec<PathBuf>,
    pub(crate) source: api::PeerSource,
    pub(crate) options: api::TransferOptions,
    /// Contextual line to print before starting, when the mode warrants one.
    pub(crate) note: Option<String>,
    pub(crate) config: Option<PathBuf>,
    pub(crate) identity: IdentityConfig,
    pub(crate) compression: api::CompressionPolicyV2,
    pub(crate) approve_large_transfer: bool,
    pub(crate) source_issue_action: SourceIssueActionArg,
    pub(crate) save_mode: SaveModeArg,
}

/// Resolve a `--room` value: use the code the user gave, or generate one via an
/// [`api::Invite`] and build a note that shows the code + a terminal QR for the
/// other side. `give_to` labels that side; `waiting` is the base status line.
fn resolve_room_code(
    room: Option<String>,
    broker: &str,
    relay: Option<&str>,
    role: api::Role,
    give_to: &str,
    waiting: &str,
) -> Result<(String, String), TransferError> {
    match room {
        Some(code) => Ok((code, waiting.to_string())),
        None => {
            let invite =
                api::Invite::room(broker.to_string(), relay.map(String::from))?.with_role(role);
            let qr = render_terminal_qr(&invite.payload())
                .map(|q| format!("\n{q}"))
                .unwrap_or_default();
            let note = format!(
                "your code: {}  - give this to the {give_to}{qr}\n{waiting}",
                invite.code()
            );
            Ok((invite.code().to_string(), note))
        }
    }
}

impl SendArgs {
    pub(crate) fn into_plan(self) -> Result<TransferPlan, TransferError> {
        let mut options = api::TransferOptions::default();
        options.resume = self.resume;
        options.relay = self.relay.clone();
        options.path = path_policy(self.relay_only, self.direct_only)?;

        let (source, note) = if let Some(room) = self.room {
            let broker = self
                .rendezvous
                .expect("clap requires --rendezvous with --room");
            let (code, note) = resolve_room_code(
                room,
                &broker,
                self.relay.as_deref(),
                api::Role::Send,
                "receiver",
                &format!("pairing in room via {broker}..."),
            )?;
            (api::PeerSource::Room { code, broker }, Some(note))
        } else if let Some(invite) = self.invite {
            (api::PeerSource::Invite { invite }, None)
        } else if self.enable_mdns {
            if self.peer.is_some() {
                return Err(TransferError::input(
                    "use either --enable-mdns or --peer, not both",
                ));
            }
            let token = self
                .token
                .expect("clap ensures --token is present with --enable-mdns");
            let source = api::PeerSource::Mdns { token: Some(token) };
            (source, Some("discovering receiver over mDNS...".into()))
        } else {
            let peer = self.peer.ok_or_else(|| {
                TransferError::input("send requires --peer unless --enable-mdns or --invite is set")
            })?;
            let token = self
                .token
                .expect("clap ensures --token is present without --invite");
            (api::PeerSource::Manual { peer, token }, None)
        };

        Ok(TransferPlan {
            path: self.file,
            additional_paths: self.additional_files,
            source,
            options,
            note,
            config: self.config,
            identity: identity_config(self.identity),
            compression: match self.compression {
                CompressionPolicyArg::Never => api::CompressionPolicyV2::Never,
                CompressionPolicyArg::Always => api::CompressionPolicyV2::Always,
                CompressionPolicyArg::Smart => api::CompressionPolicyV2::Smart,
            },
            approve_large_transfer: false,
            source_issue_action: self.source_issue_action,
            save_mode: SaveModeArg::Direct,
        })
    }
}

impl ReceiveArgs {
    pub(crate) fn into_plan(self) -> Result<TransferPlan, TransferError> {
        let mut options = api::TransferOptions::default();
        options.listen_addrs = Some(receive_addrs_for(self.ip_version));
        options.relay = self.relay.clone();
        options.path = path_policy(self.relay_only, self.direct_only)?;

        let (source, note) = if let Some(room) = self.room {
            let broker = self
                .rendezvous
                .expect("clap requires --rendezvous with --room");
            let (code, note) = resolve_room_code(
                room,
                &broker,
                self.relay.as_deref(),
                api::Role::Receive,
                "sender",
                &format!("waiting for sender via rendezvous {broker}..."),
            )?;
            (api::PeerSource::Room { code, broker }, Some(note))
        } else if self.enable_mdns {
            let source = api::PeerSource::Mdns { token: self.token };
            (source, Some("waiting for sender...".into()))
        } else {
            let token = self
                .token
                .expect("clap requires --token unless --enable-mdns is set");
            (api::PeerSource::ShowManual { token: Some(token) }, None)
        };

        Ok(TransferPlan {
            path: self.output,
            additional_paths: Vec::new(),
            source,
            options,
            note,
            config: self.config,
            identity: identity_config(self.identity),
            compression: api::CompressionPolicyV2::Smart,
            approve_large_transfer: self.approve_large_transfer,
            source_issue_action: SourceIssueActionArg::Fail,
            save_mode: self.save_mode,
        })
    }
}

/// Explains why `--direct-only` is refused, honestly and in full.
const DIRECT_ONLY_DISABLED: &str = "--direct-only is temporarily disabled: a relay-free direct path between two NATed peers \
     is not achievable. iroh's hole-punching is a QUIC NAT-traversal extension that runs over \
     a connection which, when both peers are NATed, must first be established through the relay \
     - so removing the relay removes the punch, not just the fallback. Direct paths still form \
     automatically when a relay is present (it coordinates the punch, then data flows direct), \
     when the peer is publicly reachable, or on the same LAN.";

fn path_policy(relay_only: bool, direct_only: bool) -> Result<api::PathPolicy, TransferError> {
    if direct_only {
        return Err(TransferError::input(DIRECT_ONLY_DISABLED));
    }
    Ok(if relay_only {
        api::PathPolicy::RelayOnly
    } else {
        api::PathPolicy::Auto
    })
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
#[path = "args_tests.rs"]
mod tests;
