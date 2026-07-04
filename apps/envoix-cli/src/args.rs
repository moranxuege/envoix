//! Command-line arguments: clap definitions and parse tests.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use envoix_client::PeerDescriptor;

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
    #[arg(long, requires = "rendezvous", conflicts_with_all = ["peer", "invite", "enable_mdns", "token"])]
    pub(crate) room: Option<String>,
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
    /// Force a direct data path: no relay fallback for the transfer (the
    /// relay is still used to reach the broker). Direct-or-fail.
    #[arg(long, requires = "room", conflicts_with = "relay_only")]
    pub(crate) direct_only: bool,
    /// File to send.
    pub(crate) file: PathBuf,
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
    #[arg(long, requires = "rendezvous", conflicts_with_all = ["token", "enable_mdns"])]
    pub(crate) room: Option<String>,
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
    /// Force a direct data path: no relay fallback for the transfer (the
    /// relay is still used to reach the broker). Direct-or-fail.
    #[arg(long, requires = "room", conflicts_with = "relay_only")]
    pub(crate) direct_only: bool,
    /// Address family to bind for receiving.
    #[arg(long, value_enum, default_value_t = IpVersion::Dual)]
    pub(crate) ip_version: IpVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum IpVersion {
    Dual,
    Ipv4,
    Ipv6,
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
            Command::Send(SendArgs {
                peer,
                config: None,
                enable_mdns,
                resume,
                ref token,
                invite: None,
                ref file,
                ..
            }) if peer == Some(PEER.parse().unwrap())
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
            Command::Send(SendArgs {
                peer: None,
                config: None,
                enable_mdns: true,
                resume: true,
                ref token,
                invite: None,
                ref file,
                ..
            }) if token.as_deref() == Some("abcdefghijkl")
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

        assert!(matches!(
            cli.command,
            Command::Send(SendArgs { resume: false, .. })
        ));
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
            Command::Receive(ReceiveArgs {
                output,
                config: None,
                enable_mdns,
                token: Some(ref token),
                ip_version,
                ..
            }) if output == std::path::Path::new("received")
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
            Command::Receive(ReceiveArgs {
                ip_version: IpVersion::Ipv4,
                ..
            })
        ));
    }

    #[test]
    fn parses_receive_enable_mdns_command() {
        let cli =
            Cli::try_parse_from(["envoix", "receive", "--enable-mdns", "--output", "received"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive(ReceiveArgs {
                output,
                enable_mdns: true,
                token: None,
                ..
            }) if output == std::path::Path::new("received")
        ));
    }

    #[test]
    fn parses_receive_auto_alias() {
        let cli =
            Cli::try_parse_from(["envoix", "receive", "--auto", "--output", "received"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive(ReceiveArgs {
                enable_mdns: true,
                ..
            })
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
            Command::Receive(ReceiveArgs {
                enable_mdns: false,
                token: Some(ref t),
                ..
            }) if t == "abcdefghijkl"
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
            Command::Send(SendArgs { room: Some(ref r), rendezvous: Some(ref rv), .. })
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
            Command::Receive(ReceiveArgs { room: Some(ref r), .. }) if r == "123456-amber-comet"
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
            Command::Receive(ReceiveArgs {
                ip_version: IpVersion::Ipv6,
                ..
            })
        ));
    }

    #[test]
    fn parses_send_invite_command() {
        let cli = Cli::try_parse_from(["envoix", "send", "--invite", "envoix:dGVzdA", "hello.txt"])
            .unwrap();

        assert!(matches!(
            cli.command,
            Command::Send(SendArgs {
                peer: None,
                config: None,
                enable_mdns: false,
                resume: true,
                token: None,
                ref invite,
                ref file,
                ..
            }) if invite.as_deref() == Some("envoix:dGVzdA")
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
            Command::Send(SendArgs {
                config,
                ..
            }) if config == Some(std::path::PathBuf::from("envoix.toml"))
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
