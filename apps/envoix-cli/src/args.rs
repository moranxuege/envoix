//! Command-line arguments and their canonical transfer-plan projection.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

Invitation V2:
    envoix receive --create-invite --rendezvous <broker> --output ./received
    envoix send --invite <envoix://invite/v2/...> <file>
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
    /// Override the local Envoix Agent Unix socket.
    #[arg(long, global = true)]
    pub(crate) agent_socket: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Send one or more files or folders as one transfer job.
    Send(SendArgs),
    /// Receive one transfer job into an output directory.
    Receive(ReceiveArgs),
    /// Inspect or pair with the persistent local Agent.
    Agent(AgentArgs),
    /// Manage remembered devices owned by the local Agent.
    Devices(DevicesArgs),
    /// Inspect files received by the local Agent.
    Inbox(InboxArgs),
}

#[derive(Args, Debug)]
pub(crate) struct AgentArgs {
    #[command(subcommand)]
    pub(crate) command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentCommand {
    /// Show whether the Agent is running and listening for remembered peers.
    Status,
    /// Print the Agent's immutable Engine and Inbox snapshot.
    Snapshot {
        /// Maximum number of recent Inbox entries to include.
        #[arg(long, default_value_t = 20)]
        inbox_limit: usize,
    },
    /// Read Agent events after a snapshot or prior event cursor.
    Events {
        /// Agent instance ID returned by `agent snapshot` or a prior poll.
        #[arg(long)]
        instance_id: String,
        /// Last consumed event sequence.
        #[arg(long)]
        after: u64,
        /// Maximum number of events to return.
        #[arg(long, default_value_t = 64)]
        limit: usize,
    },
    /// Install and start the Agent as a systemd user service.
    Install {
        /// Inbox directory; defaults to the Agent state directory's Inbox.
        #[arg(long)]
        inbox: Option<PathBuf>,
        /// Name shown to sending devices.
        #[arg(long, default_value = "WSL")]
        device_name: String,
        /// Prebuilt envoix-agent binary; defaults to the CLI's directory or PATH.
        #[arg(long)]
        agent_binary: Option<PathBuf>,
    },
    /// Start the installed Agent service.
    Start,
    /// Stop the installed Agent service.
    Stop,
    /// Create a one-time receive invitation that becomes a remembered device.
    Pair {
        /// Name for the Mac or other sending device.
        #[arg(long)]
        name: String,
    },
}

#[derive(Args, Debug)]
pub(crate) struct DevicesArgs {
    #[command(subcommand)]
    pub(crate) command: DevicesCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DevicesCommand {
    /// List devices that can reconnect without a new invitation.
    List,
    /// Revoke one remembered device and delete its credential.
    Forget {
        /// Device ID or exact label shown by `devices list`.
        device: String,
        /// Confirm Relationship revocation and credential deletion.
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Args, Debug)]
pub(crate) struct InboxArgs {
    #[command(subcommand)]
    pub(crate) command: InboxCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum InboxCommand {
    /// List newest completed transfers.
    List {
        /// Maximum number of transfers to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Print the saved path(s) from the newest completed transfer.
    Latest,
}

/// Arguments for `envoix send`.
#[derive(Args, Debug)]
pub(crate) struct SendArgs {
    /// Receiver peer descriptor (manual mode). Cannot be combined with --invite.
    #[arg(long, conflicts_with_all = ["invite", "create_invite"])]
    pub(crate) peer: Option<PeerDescriptor>,
    /// Explicit TOML config file path.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
    /// Use iroh mDNS discovery when available. Cannot be combined with --invite.
    #[arg(long, conflicts_with_all = ["invite", "create_invite"])]
    pub(crate) enable_mdns: bool,
    /// Persistent iroh identity file. Created if missing.
    #[arg(long, conflicts_with = "ephemeral_identity")]
    pub(crate) identity: Option<PathBuf>,
    /// Use a fresh iroh identity for this run.
    #[arg(long)]
    pub(crate) ephemeral_identity: bool,
    /// Shared ASCII pairing token (>=12 bytes). Required in manual and mDNS modes.
    #[arg(
        long,
        required_unless_present_any = ["invite", "create_invite"],
        conflicts_with_all = ["invite", "create_invite"]
    )]
    pub(crate) token: Option<String>,
    /// Complete directional InviteV2 payload.
    #[arg(long, conflicts_with_all = ["peer", "enable_mdns", "token", "create_invite"])]
    pub(crate) invite: Option<String>,
    /// Create a directional invitation and wait as its sender.
    #[arg(long, requires = "rendezvous", conflicts_with_all = ["peer", "invite", "enable_mdns", "token"])]
    pub(crate) create_invite: bool,
    /// Rendezvous broker address, `<endpoint-id>@<ip:port>`.
    #[arg(long)]
    pub(crate) rendezvous: Option<String>,
    /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
    #[arg(long)]
    pub(crate) relay: Option<String>,
    /// Force the data path through the relay (no direct/holepunch). For A/B
    /// testing relay vs direct; requires --relay.
    #[arg(long, requires = "relay")]
    pub(crate) relay_only: bool,
    /// (Temporarily disabled - see docs/design/client-api.md 5.5.) A relay-free
    /// direct path between two NATed peers is not achievable: iroh's
    /// hole-punching runs over a connection that must first be established via
    /// the relay, so removing the relay removes the punch itself.
    #[arg(long, conflicts_with = "relay_only", hide = true)]
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
    /// Shared ASCII pairing token (>=12 bytes). Required in manual and mDNS modes.
    #[arg(
        long,
        required_unless_present_any = ["enable_mdns", "invite", "create_invite"],
        conflicts_with_all = ["invite", "create_invite"]
    )]
    pub(crate) token: Option<String>,
    /// Complete directional InviteV2 payload.
    #[arg(long, conflicts_with_all = ["enable_mdns", "token", "create_invite"])]
    pub(crate) invite: Option<String>,
    /// Create a directional invitation and wait as its receiver.
    #[arg(long, requires = "rendezvous", conflicts_with_all = ["invite", "enable_mdns", "token"])]
    pub(crate) create_invite: bool,
    /// Rendezvous broker address, `<endpoint-id>@<ip:port>`.
    #[arg(long)]
    pub(crate) rendezvous: Option<String>,
    /// Relay URL for WAN/NAT reachability, e.g. https://relay.example.com:8444.
    #[arg(long)]
    pub(crate) relay: Option<String>,
    /// Force the data path through the relay (no direct/holepunch). For A/B
    /// testing relay vs direct; requires --relay.
    #[arg(long, requires = "relay")]
    pub(crate) relay_only: bool,
    /// (Temporarily disabled - see docs/design/client-api.md 5.5.) A relay-free
    /// direct path between two NATed peers is not achievable: iroh's
    /// hole-punching runs over a connection that must first be established via
    /// the relay, so removing the relay removes the punch itself.
    #[arg(long, conflicts_with = "relay_only", hide = true)]
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

fn create_invitation_source(
    broker: &str,
    relay: Option<&str>,
    role: api::TransferRole,
    give_to: &str,
    waiting: &str,
) -> Result<(api::PeerSource, String), TransferError> {
    let created = api::create_invitation(
        broker.to_string(),
        relay.into_iter().map(str::to_string).collect(),
        role,
        unix_now()?,
    )?;
    let qr = render_terminal_qr(&created.payload)
        .map(|qr| format!("\n{qr}"))
        .unwrap_or_default();
    let note = format!(
        "Complete invitation: {}{qr}\nGive this value to the {give_to}.\n{waiting}",
        created.payload
    );
    Ok((
        api::PeerSource::invitation(created.into_bootstrap(), broker.to_string())?,
        note,
    ))
}

fn join_full_invitation(
    payload: &str,
    role: api::TransferRole,
) -> Result<(api::PeerSource, Option<String>), TransferError> {
    let validated = api::parse_invitation_for_role(payload, role, unix_now()?)?;
    let public = &validated.invitation().public_context;
    let broker = public.broker.clone();
    let relay = public.relay_urls.first().cloned();
    Ok((
        api::PeerSource::invitation(validated.into_bootstrap(), broker)?,
        relay,
    ))
}

fn unix_now() -> Result<u64, TransferError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| TransferError::input("system clock is before the Unix epoch"))
}

impl SendArgs {
    pub(crate) fn into_plan(self) -> Result<TransferPlan, TransferError> {
        let mut options = api::TransferOptions::default();
        options.relay = self.relay.clone();
        options.path = path_policy(self.relay_only, self.direct_only)?;

        let (source, note) = if self.create_invite {
            let broker = self
                .rendezvous
                .expect("clap requires --rendezvous with --create-invite");
            let (source, note) = create_invitation_source(
                &broker,
                self.relay.as_deref(),
                api::TransferRole::Sender,
                "receiver",
                &format!("waiting for the receiver via {broker}..."),
            )?;
            (source, Some(note))
        } else if let Some(payload) = self.invite {
            let (source, relay) = join_full_invitation(&payload, api::TransferRole::Sender)?;
            options.relay = relay;
            (source, None)
        } else if self.enable_mdns {
            if self.peer.is_some() {
                return Err(TransferError::input(
                    "use either --enable-mdns or --peer, not both",
                ));
            }
            let token = self
                .token
                .expect("clap ensures --token is present with --enable-mdns");
            let source = api::PeerSource::mdns(Some(token))?;
            (source, Some("discovering receiver over mDNS...".into()))
        } else {
            let peer = self.peer.ok_or_else(|| {
                TransferError::input("send requires --peer unless --enable-mdns or --invite is set")
            })?;
            let token = self
                .token
                .expect("clap ensures --token is present without --invite");
            (api::PeerSource::manual(peer, token)?, None)
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

        let (source, note) = if self.create_invite {
            let broker = self
                .rendezvous
                .expect("clap requires --rendezvous with --create-invite");
            let (source, note) = create_invitation_source(
                &broker,
                self.relay.as_deref(),
                api::TransferRole::Receiver,
                "sender",
                &format!("waiting for sender via rendezvous {broker}..."),
            )?;
            (source, Some(note))
        } else if let Some(payload) = self.invite {
            let (source, relay) = join_full_invitation(&payload, api::TransferRole::Receiver)?;
            options.relay = relay;
            (source, None)
        } else if self.enable_mdns {
            let source = api::PeerSource::mdns(self.token)?;
            (source, Some("waiting for sender...".into()))
        } else {
            let token = self
                .token
                .expect("clap requires --token unless --enable-mdns is set");
            (api::PeerSource::show_manual(Some(token))?, None)
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
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn naked_room_code_flag_is_retired() {
        assert!(
            Cli::try_parse_from([
                "envoix",
                "send",
                "--room-code",
                "123456-k7m4-9v2d",
                "--rendezvous",
                "broker",
                "./hello.txt",
            ])
            .is_err()
        );
    }

    #[test]
    fn complete_invitation_flag_remains_available() {
        assert!(
            Cli::try_parse_from([
                "envoix",
                "send",
                "--invite",
                "envoix://invite/v2/example",
                "./hello.txt",
            ])
            .is_ok()
        );
    }

    #[test]
    fn agent_and_inbox_commands_are_available_without_transfer_flags() {
        assert!(Cli::try_parse_from(["envoix", "agent", "status"]).is_ok());
        assert!(
            Cli::try_parse_from(["envoix", "agent", "snapshot", "--inbox-limit", "10"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "envoix",
                "agent",
                "events",
                "--instance-id",
                "agent_fixture",
                "--after",
                "0",
                "--limit",
                "32",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "envoix",
                "agent",
                "install",
                "--inbox",
                "./inbox",
                "--device-name",
                "Dev WSL",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["envoix", "agent", "start"]).is_ok());
        assert!(Cli::try_parse_from(["envoix", "agent", "stop"]).is_ok());
        assert!(Cli::try_parse_from(["envoix", "agent", "pair", "--name", "MacBook"]).is_ok());
        assert!(Cli::try_parse_from(["envoix", "devices", "list"]).is_ok());
        assert!(Cli::try_parse_from(["envoix", "devices", "forget", "MacBook", "--yes"]).is_ok());
        assert!(Cli::try_parse_from(["envoix", "devices", "forget", "MacBook"]).is_err());
        assert!(Cli::try_parse_from(["envoix", "inbox", "latest"]).is_ok());
    }
}
