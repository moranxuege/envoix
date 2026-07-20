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
        Command::Send(SendArgs { room: Some(Some(ref r)), rendezvous: Some(ref rv), .. })
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
        Command::Receive(ReceiveArgs { room: Some(Some(ref r)), .. }) if r == "123456-amber-comet"
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
#[test]
fn send_plan_maps_room_args_to_room_source_with_path_policy() {
    let cli = Cli::try_parse_from([
        "envoix",
        "send",
        "--room",
        "123456-amber-comet",
        "--rendezvous",
        "id@1.2.3.4:8445",
        "--relay",
        "https://r.example:8444",
        "--relay-only",
        "file.txt",
    ])
    .unwrap();
    let Command::Send(args) = cli.command else {
        panic!()
    };
    let plan = args.into_plan().unwrap();
    assert!(matches!(plan.source, api::PeerSource::Room { .. }));
    assert_eq!(plan.options.path, api::PathPolicy::RelayOnly);
    assert_eq!(
        plan.options.relay.as_deref(),
        Some("https://r.example:8444")
    );
    assert!(plan.note.unwrap().contains("id@1.2.3.4:8445"));
}

#[test]
fn receive_plan_without_flags_listens_manually() {
    let cli = Cli::try_parse_from([
        "envoix",
        "receive",
        "--output",
        "out",
        "--token",
        "abcdefghijkl",
    ])
    .unwrap();
    let Command::Receive(args) = cli.command else {
        panic!()
    };
    let plan = args.into_plan().unwrap();
    assert!(matches!(
        plan.source,
        api::PeerSource::ShowManual { token: Some(_) }
    ));
    assert_eq!(plan.options.path, api::PathPolicy::Auto);
    assert!(plan.note.is_none());
}

#[test]
fn send_plan_rejects_peer_with_mdns() {
    let cli = Cli::try_parse_from([
        "envoix",
        "send",
        "--enable-mdns",
        "--token",
        "abcdefghijkl",
        "--peer",
        "id@1.2.3.4:1",
        "file.txt",
    ])
    .unwrap();
    let Command::Send(args) = cli.command else {
        panic!()
    };
    assert!(args.into_plan().is_err());
}
#[test]
fn verbose_flag_counts() {
    let cli = Cli::try_parse_from([
        "envoix",
        "receive",
        "-vv",
        "--output",
        "out",
        "--token",
        "abcdefghijkl",
    ])
    .unwrap();
    assert_eq!(cli.verbose, 2);
    let cli = Cli::try_parse_from([
        "envoix",
        "receive",
        "--output",
        "out",
        "--token",
        "abcdefghijkl",
    ])
    .unwrap();
    assert_eq!(cli.verbose, 0);
}
