use super::*;
use iroh::SecretKey;

#[test]
fn advertised_ports_are_reused_without_pinning_interface_ips() {
    let dynamic = BindAddrs::dual_stack(0);
    let fixed = dynamic
        .rebind_from_advertised(&[
            "192.0.2.4:41000".parse().unwrap(),
            "[2001:db8::4]:42000".parse().unwrap(),
        ])
        .unwrap();
    let addrs = fixed.iter().map(|bind| bind.addr).collect::<Vec<_>>();

    assert_eq!(addrs[0], "0.0.0.0:41000".parse().unwrap());
    assert_eq!(addrs[1], "[::]:42000".parse().unwrap());
    assert!(fixed.rebind_from_advertised(&[]).is_none());
}

#[test]
fn platform_system_dns_separates_and_deduplicates_address_families() {
    let addresses = [
        "192.0.2.2:0".parse().unwrap(),
        "[2001:db8::2]:0".parse().unwrap(),
        "192.0.2.1:0".parse().unwrap(),
        "192.0.2.2:0".parse().unwrap(),
        "[2001:db8::1]:0".parse().unwrap(),
    ];

    assert_eq!(
        ipv4_addresses(addresses),
        [
            "192.0.2.2".parse::<Ipv4Addr>().unwrap(),
            "192.0.2.1".parse::<Ipv4Addr>().unwrap()
        ]
    );
    assert_eq!(
        ipv6_addresses(addresses),
        [
            "2001:db8::2".parse::<Ipv6Addr>().unwrap(),
            "2001:db8::1".parse::<Ipv6Addr>().unwrap()
        ]
    );
}

#[test]
fn resolve_interfaces_excludes_denied_ranges_from_the_bind() {
    // Deny a real local interface address; resolving the unspecified binds
    // must not include it (so iroh never binds it, never uses it as a
    // candidate) - this is what makes the filter suppress e.g. Tailscale.
    let locals = local_interface_addrs();
    let Some(&(denied, _)) = locals.first() else {
        return; // no non-loopback interfaces in this environment
    };
    let cidr = match denied {
        IpAddr::V4(_) => format!("{denied}/32"),
        IpAddr::V6(_) => format!("{denied}/128"),
    };
    let filter = CandidateFilter::from_lists(&[], &[cidr]).unwrap();
    let resolved: Vec<IpAddr> = BindAddrs::dual_stack(0)
        .resolve_interfaces(&filter)
        .iter()
        .map(|bind| bind.addr.ip())
        .collect();
    assert!(
        !resolved.contains(&denied),
        "denied interface {denied} must not be bound, got {resolved:?}"
    );
}

#[test]
fn resolve_interfaces_is_a_noop_without_a_filter() {
    let base = BindAddrs::dual_stack(0);
    assert_eq!(
        base.clone().resolve_interfaces(&CandidateFilter::default()),
        base
    );
}

#[test]
fn resolve_with_binds_all_permitted_addresses_per_family() {
    // `[candidates]` scopes the set: two permitted IPv4 interfaces must BOTH
    // be bound (not one arbitrary survivor by enumeration order), only the
    // first marked as the family's default route; a denied address is dropped.
    let a: IpAddr = "10.0.0.5".parse().unwrap();
    let b: IpAddr = "192.168.1.5".parse().unwrap();
    let denied: IpAddr = "100.64.0.5".parse().unwrap(); // Tailscale CGNAT
    let locals = [(a, 24), (denied, 10), (b, 24)];
    let filter = CandidateFilter::from_lists(&[], &["100.64.0.0/10".into()]).unwrap();

    let bound = BindAddrs::dual_stack(0).resolve_with(&filter, &locals);
    let ips: Vec<IpAddr> = bound.iter().map(|bind| bind.addr.ip()).collect();

    assert!(
        ips.contains(&a) && ips.contains(&b),
        "both permitted IPv4 addresses must be bound, got {ips:?}"
    );
    assert!(!ips.contains(&denied), "denied address must be dropped");
    let v4_default_routes = bound
        .iter()
        .filter(|bind| bind.addr.is_ipv4() && bind.default_route)
        .count();
    assert_eq!(v4_default_routes, 1, "exactly one IPv4 default route");
}

#[test]
fn dual_stack_bind_addrs_include_ipv4_and_ipv6_unspecified() {
    let addrs: Vec<_> = BindAddrs::dual_stack(0)
        .iter()
        .map(|bind_addr| bind_addr.addr)
        .collect();

    assert_eq!(addrs.len(), 2);
    assert!(addrs.contains(&"0.0.0.0:0".parse().unwrap()));
    assert!(addrs.contains(&"[::]:0".parse().unwrap()));
}

#[test]
fn dual_stack_makes_ipv6_best_effort() {
    let addrs: Vec<_> = BindAddrs::dual_stack(0).iter().collect();

    assert!(
        addrs
            .iter()
            .any(|bind_addr| bind_addr.addr.is_ipv4() && bind_addr.required)
    );
    assert!(
        addrs
            .iter()
            .any(|bind_addr| bind_addr.addr.is_ipv6() && !bind_addr.required)
    );
}

#[test]
fn broker_addr_parses_id_and_socket() {
    let id = SecretKey::generate().public();
    let addr = parse_broker_addr(&format!("{id}@127.0.0.1:8445"), None).unwrap();
    let socket: SocketAddr = "127.0.0.1:8445".parse().unwrap();
    assert_eq!(
        addr,
        EndpointAddr::from_parts(id, [TransportAddr::Ip(socket)])
    );
}

#[test]
fn broker_addr_appends_relay() {
    let id = SecretKey::generate().public();
    let addr = parse_broker_addr(
        &format!("{id}@127.0.0.1:8445"),
        Some("https://relay.example:8444"),
    )
    .unwrap();
    assert_eq!(addr.relay_urls().count(), 1);
}

#[test]
fn broker_addr_requires_at_sign() {
    assert!(parse_broker_addr("127.0.0.1:8445", None).is_err());
}
