use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use envoix_error::CoreError;
use envoix_protocol::PeerDescriptor;
use iroh::endpoint::{BindOpts, QuicTransportConfig, RelayMode, VarInt, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayUrl, TransportAddr};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use noq_proto::congestion::Bbr3Config;

use crate::candidates::CandidateFilter;
use crate::connection::IrohFrameConnection;
use crate::identity::{IdentityConfig, load_secret_key};
use crate::{ALPN, SessionError};

/// Local socket addresses an accepting iroh endpoint should bind.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct BindAddrs {
    addrs: Vec<BindAddr>,
}

impl BindAddrs {
    /// Binds one local socket address.
    pub fn single(addr: SocketAddr) -> Self {
        Self {
            addrs: vec![BindAddr::required(addr)],
        }
    }

    /// Binds unspecified IPv4 and IPv6 sockets on the requested port.
    ///
    /// Passing port `0` lets the OS choose an independent free port per family.
    /// The IPv6 bind is best-effort, matching iroh's default endpoint behavior.
    pub fn dual_stack(port: u16) -> Self {
        Self {
            addrs: vec![
                BindAddr::required(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)),
                BindAddr::optional(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)),
            ],
        }
    }

    fn iter(&self) -> impl Iterator<Item = BindAddr> + '_ {
        self.addrs.iter().copied()
    }

    /// Replace unspecified (`0.0.0.0` / `::`) binds with the concrete local
    /// interface addresses `filter` permits, so a denied range (e.g. Tailscale's
    /// `100.64.0.0/10` / `fd7a:115c:a1e0::/48`) is never bound - and therefore
    /// never used by iroh as a holepunch candidate, not merely hidden from the
    /// advertised descriptor. Specific binds are kept if permitted; an empty
    /// filter (or an empty survivor set) leaves the binds untouched.
    fn resolve_interfaces(self, filter: &CandidateFilter) -> Self {
        self.resolve_with(filter, &local_interface_addrs())
    }

    /// Testable core of [`resolve_interfaces`]. Each unspecified bind expands to
    /// *every* permitted concrete address of its family, so `[candidates]` scopes
    /// the candidate set (per the allow/deny policy) rather than picking one
    /// arbitrary survivor by enumeration order. Each keeps its subnet prefix so
    /// only the first per family is the default route (iroh permits one per
    /// family; a `prefix_len` of 0 would also count as a default route).
    fn resolve_with(self, filter: &CandidateFilter, locals: &[(IpAddr, u8)]) -> Self {
        if filter.is_empty() {
            return self;
        }
        let mut addrs = Vec::new();
        for bind in &self.addrs {
            if bind.addr.ip().is_unspecified() {
                let want_v6 = bind.addr.is_ipv6();
                let permitted: Vec<_> = locals
                    .iter()
                    .filter(|(ip, _)| ip.is_ipv6() == want_v6 && filter.permits_ip(*ip))
                    .collect();
                // iroh rejects adding a same-family bind once a default exists, so
                // the default route must be added last: mark the final address.
                let last = permitted.len().saturating_sub(1);
                for (i, (ip, prefix_len)) in permitted.into_iter().enumerate() {
                    addrs.push(BindAddr {
                        addr: SocketAddr::new(*ip, bind.addr.port()),
                        // Best-effort: a flaky NIC must not abort the endpoint.
                        required: false,
                        default_route: i == last,
                        prefix_len: *prefix_len,
                    });
                }
            } else if filter.permits_ip(bind.addr.ip()) {
                addrs.push(*bind);
            }
        }
        // If the filter left nothing to bind, keep the original request rather
        // than silently producing a relay-only endpoint.
        if addrs.is_empty() {
            self
        } else {
            Self { addrs }
        }
    }
}

/// Concrete local interface addresses (with subnet prefix length) worth binding:
/// loopback, link-local, and unspecified are dropped. Best-effort - an
/// enumeration error yields none.
fn local_interface_addrs() -> Vec<(IpAddr, u8)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| {
            let (ip, prefix) = match iface.addr {
                if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), v4.prefixlen),
                if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), v6.prefixlen),
            };
            (!ip.is_loopback() && !ip.is_unspecified() && !is_link_local(&ip))
                .then_some((ip, prefix))
        })
        .collect()
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct BindAddr {
    addr: SocketAddr,
    required: bool,
    /// Whether this socket is the default route for its IP family. iroh permits
    /// exactly one default route per family; extra bound addresses are not it.
    default_route: bool,
    /// Subnet prefix length for routing. `0` (an unspecified bind) also counts
    /// as a default route; concrete interface binds carry their real prefix.
    prefix_len: u8,
}

impl BindAddr {
    fn required(addr: SocketAddr) -> Self {
        Self {
            addr,
            required: true,
            default_route: true,
            prefix_len: 0,
        }
    }

    fn optional(addr: SocketAddr) -> Self {
        Self {
            addr,
            required: false,
            default_route: true,
            prefix_len: 0,
        }
    }
}

impl From<SocketAddr> for BindAddrs {
    fn from(addr: SocketAddr) -> Self {
        Self::single(addr)
    }
}

/// A bound local iroh endpoint ready to accept Envoix connections.
#[derive(Clone, Debug)]
pub struct BoundEndpoint {
    pub(crate) local_endpoint: Endpoint,
    /// Filter applied to the addresses this endpoint advertises to a peer.
    pub(crate) candidates: CandidateFilter,
}

impl BoundEndpoint {
    /// Set the filter applied to advertised candidate addresses.
    pub fn with_candidate_filter(mut self, candidates: CandidateFilter) -> Self {
        self.candidates = candidates;
        self
    }

    /// Returns the endpoint ID as a stable display string.
    pub fn endpoint_id(&self) -> String {
        self.local_endpoint.id().to_string()
    }

    /// Returns the advertised direct socket addresses (after the candidate
    /// filter).
    pub fn direct_addrs(&self) -> Vec<SocketAddr> {
        self.candidates
            .apply(self.local_endpoint.addr().ip_addrs().copied())
    }

    /// Returns an app-level direct peer descriptor for this local endpoint.
    pub fn peer_descriptor(&self) -> Result<PeerDescriptor, SessionError> {
        let addrs = self.direct_addrs();
        // Distinguish "the filter removed everything" from "the endpoint has no
        // address at all", so an over-aggressive [candidates] config gets a
        // pointed error rather than a bare "no direct addresses".
        if addrs.is_empty()
            && self.local_endpoint.addr().ip_addrs().next().is_some()
            && !self.candidates.is_empty()
        {
            return Err(CoreError::InvalidInput(
                "the candidate filter removed every advertisable address; \
                 relax the [candidates] allow/deny config (run with -v to see \
                 which rule dropped which address)"
                    .into(),
            ));
        }
        PeerDescriptor::new(self.endpoint_id(), addrs)
    }

    /// Returns this endpoint's full iroh address (id + direct addrs, plus its
    /// relay home when a relay is configured), for advertising to a peer to
    /// dial. Direct addrs pass through the candidate filter; the relay home is
    /// always kept (filtering candidates must not remove the relay fallback).
    pub fn endpoint_addr(&self) -> EndpointAddr {
        let addr = self.local_endpoint.addr();
        if self.candidates.is_empty() {
            return addr;
        }
        let ips = self
            .candidates
            .apply(addr.ip_addrs().copied())
            .into_iter()
            .map(TransportAddr::Ip);
        let relays = addr.relay_urls().cloned().map(TransportAddr::Relay);
        EndpointAddr::from_parts(self.local_endpoint.id(), ips.chain(relays))
    }

    pub(crate) async fn accept(&self) -> Result<IrohFrameConnection, SessionError> {
        let incoming = self
            .local_endpoint
            .accept()
            .await
            .ok_or_else(|| CoreError::Transport("iroh endpoint closed".into()))?;
        let connection = incoming
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        let (send, recv) = connection
            .accept_bi()
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        Ok(IrohFrameConnection::new(
            self.local_endpoint.clone(),
            connection,
            send,
            recv,
        ))
    }
}

pub(crate) fn peer_addr_from_descriptor(
    peer: &PeerDescriptor,
) -> Result<EndpointAddr, SessionError> {
    peer.validate()?;
    let id = peer
        .endpoint_id
        .parse::<EndpointId>()
        .map_err(|error| CoreError::InvalidInput(format!("invalid endpoint id: {error}")))?;
    Ok(EndpointAddr::from_parts(
        id,
        peer.direct_addrs.iter().copied().map(TransportAddr::Ip),
    ))
}

/// Parse a rendezvous broker address `<endpoint-id>@<ip:port>` into an
/// [`EndpointAddr`]. When `relay` (a relay URL) is given it is added as a
/// fallback transport, so the broker stays reachable even if direct UDP to it
/// is blocked.
pub fn parse_broker_addr(addr: &str, relay: Option<&str>) -> Result<EndpointAddr, SessionError> {
    let (id, socket) = addr.split_once('@').ok_or_else(|| {
        CoreError::InvalidInput("rendezvous address must be <endpoint-id>@<ip:port>".into())
    })?;
    let id = id
        .parse::<EndpointId>()
        .map_err(|error| CoreError::InvalidInput(format!("invalid endpoint id: {error}")))?;
    let socket = socket
        .parse::<SocketAddr>()
        .map_err(|error| CoreError::InvalidInput(format!("invalid broker address: {error}")))?;
    let mut addrs = vec![TransportAddr::Ip(socket)];
    if let Some(relay) = relay {
        let relay = relay
            .parse::<RelayUrl>()
            .map_err(|error| CoreError::InvalidInput(format!("invalid relay url: {error}")))?;
        addrs.push(TransportAddr::Relay(relay));
    }
    Ok(EndpointAddr::from_parts(id, addrs))
}

/// Convert an optional relay URL into an iroh [`RelayMode`]: `None` -> disabled
/// (LAN/direct, unchanged behavior); `Some(url)` -> a single custom relay so an
/// endpoint behind NAT stays reachable over WAN.
pub(crate) fn relay_mode(relay: &Option<String>) -> Result<RelayMode, SessionError> {
    match relay {
        None => Ok(RelayMode::Disabled),
        Some(url) => {
            let url: RelayUrl = url
                .parse()
                .map_err(|error| CoreError::InvalidInput(format!("invalid relay url: {error}")))?;
            Ok(RelayMode::Custom(RelayMap::from(url)))
        }
    }
}

pub(crate) async fn build_accept_endpoint(
    listen_addrs: BindAddrs,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        true,
        false,
        relay,
        relay_only,
        candidates,
    )
    .await
}

pub(crate) async fn build_advertising_accept_endpoint(
    listen_addrs: BindAddrs,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        true,
        true,
        relay,
        relay_only,
        candidates,
    )
    .await
}

pub(crate) async fn build_dial_endpoint(
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
) -> Result<Endpoint, SessionError> {
    build_endpoint(None, identity, false, false, relay, relay_only, candidates).await
}

/// QUIC transport tuning for high-latency links (e.g. trans-Pacific, ~280 ms RTT).
///
/// quinn's default per-stream receive window is sized for a 100 ms / 100 Mbit
/// link (1.25 MB). A single stream can have at most `window / RTT` bytes in
/// flight, so at 280 ms RTT that default caps one transfer at ~4.5 MB/s no
/// matter how fast the link is. We raise the per-stream flow-control window
/// (and the matching send window) so one transfer can fill a long fat pipe;
/// iroh's holepunching/multipath defaults (from the builder) are left untouched.
fn data_transport_config() -> QuicTransportConfig {
    // 16 MiB fills ~57 MB/s at 280 ms RTT, with headroom for lower-latency links.
    const WINDOW: u32 = 16 * 1024 * 1024;
    let mut builder = QuicTransportConfig::builder()
        .stream_receive_window(VarInt::from_u32(WINDOW))
        .send_window(WINDOW as u64);
    // Default to BBRv3. The loss-based default (CUBIC) treats every packet loss
    // as congestion and backs off, which erodes throughput on lossy long-fat
    // links (e.g. trans-Pacific, ~0.3% loss at 280 ms RTT): measured ~2.5x
    // slower than BBRv3 there, while the two match on clean paths. BBRv3 instead
    // paces at the measured bandwidth and rides through non-congestion loss. Set
    // ENVOIX_CC=cubic to fall back to CUBIC.
    let use_cubic = std::env::var("ENVOIX_CC").is_ok_and(|v| v.eq_ignore_ascii_case("cubic"));
    if !use_cubic {
        builder = builder.congestion_controller_factory(Arc::new(Bbr3Config::default()));
    }
    builder.build()
}

async fn build_endpoint(
    local_listen_addrs: Option<BindAddrs>,
    identity: &IdentityConfig,
    accept_incoming: bool,
    advertise_self: bool,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
) -> Result<Endpoint, SessionError> {
    let secret_key = load_secret_key(identity).await?;
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(relay_mode(relay)?)
        .transport_config(data_transport_config())
        .clear_address_lookup();
    if accept_incoming {
        builder = builder.alpns(vec![ALPN.to_vec()]);
    }
    if advertise_self {
        builder = builder.address_lookup(MdnsAddressLookup::builder().advertise(true));
    }
    if relay_only {
        // Bind no IP transport, so this endpoint can only reach peers through the
        // relay - forces the relay data path (for A/B testing relay vs direct).
        // Requires a relay to be configured, else the endpoint can reach no one.
        builder = builder.clear_ip_transports();
    } else if !candidates.is_empty() {
        // A candidate filter is set: bind only the concrete local interface
        // addresses it permits, so a denied range (e.g. Tailscale) is never bound
        // and iroh cannot use it as a holepunch candidate. This also covers the
        // dial endpoint (which otherwise binds all interfaces via the default).
        let base = local_listen_addrs.unwrap_or_else(|| BindAddrs::dual_stack(0));
        builder = builder.clear_ip_transports();
        for bind_addr in base.resolve_interfaces(candidates).iter() {
            builder = builder
                .bind_addr_with_opts(
                    bind_addr.addr,
                    BindOpts::default()
                        .set_is_required(bind_addr.required)
                        .set_is_default_route(bind_addr.default_route)
                        .set_prefix_len(bind_addr.prefix_len),
                )
                .map_err(|error| CoreError::Transport(error.to_string()))?;
        }
    } else if let Some(addrs) = local_listen_addrs {
        builder = builder.clear_ip_transports();
        for bind_addr in addrs.iter() {
            builder = builder
                .bind_addr_with_opts(
                    bind_addr.addr,
                    BindOpts::default()
                        .set_is_required(bind_addr.required)
                        .set_is_default_route(bind_addr.default_route)
                        .set_prefix_len(bind_addr.prefix_len),
                )
                .map_err(|error| CoreError::Transport(error.to_string()))?;
        }
    }
    builder
        .bind()
        .await
        .map_err(|error| CoreError::Transport(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

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
}
