use envoix_error::CoreError;
use envoix_protocol::{PeerDescriptor, TransferProtocol};
#[cfg(any(target_os = "ios", target_os = "android"))]
use iroh::dns::{BoxIter, DnsError, DnsResolver, Resolver, TxtRecordData};
use iroh::endpoint::{BindOpts, QuicTransportConfig, RelayMode, VarInt, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayUrl, TransportAddr, Watcher as _};
use iroh_mdns_address_lookup::MdnsAddressLookup;
#[cfg(any(target_os = "ios", target_os = "android"))]
use n0_future::boxed::BoxFuture;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::candidates::CandidateFilter;
use crate::connection::IrohFrameConnection;
use crate::identity::{IdentityConfig, load_secret_key};
use crate::{EventSink, SessionError, TransferEvent};

const ENDPOINT_ADDR_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const ENDPOINT_ADDR_WAIT_POLL: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(any(target_os = "ios", target_os = "android"))]
const PLATFORM_DNS_FALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(any(target_os = "ios", target_os = "android"))]
#[derive(Debug)]
struct PlatformSystemDnsResolver {
    fallback: Option<DnsResolver>,
}

#[cfg(any(target_os = "ios", target_os = "android"))]
impl Default for PlatformSystemDnsResolver {
    fn default() -> Self {
        Self {
            fallback: platform_dns_fallback_server().map(DnsResolver::with_nameserver),
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
impl Resolver for PlatformSystemDnsResolver {
    fn lookup_ipv4(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv4Addr>, DnsError>> {
        let fallback = self.fallback.clone();
        Box::pin(async move {
            let addrs = match system_lookup(&host).await {
                Ok(addrs) => addrs,
                Err(error) => {
                    let Some(fallback) = fallback else {
                        return Err(error);
                    };
                    tracing::debug!(host, "falling back from platform system DNS");
                    let ips = fallback
                        .lookup_ipv4(&host, PLATFORM_DNS_FALLBACK_TIMEOUT)
                        .await?
                        .filter_map(|ip| match ip {
                            IpAddr::V4(ip) => Some(ip),
                            IpAddr::V6(_) => None,
                        })
                        .collect::<Vec<_>>();
                    let result: BoxIter<Ipv4Addr> = Box::new(ips.into_iter());
                    return Ok(result);
                }
            };
            let ips = ipv4_addresses(addrs);
            if !ips.is_empty() {
                let result: BoxIter<Ipv4Addr> = Box::new(ips.into_iter());
                return Ok(result);
            }

            let Some(fallback) = fallback else {
                let result: BoxIter<Ipv4Addr> = Box::new(std::iter::empty());
                return Ok(result);
            };
            tracing::debug!(
                host,
                "platform system DNS returned no IPv4 addresses; using fallback resolver"
            );
            let ips = fallback
                .lookup_ipv4(&host, PLATFORM_DNS_FALLBACK_TIMEOUT)
                .await?
                .filter_map(|ip| match ip {
                    IpAddr::V4(ip) => Some(ip),
                    IpAddr::V6(_) => None,
                })
                .collect::<Vec<_>>();
            let result: BoxIter<Ipv4Addr> = Box::new(ips.into_iter());
            Ok(result)
        })
    }

    fn lookup_ipv6(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv6Addr>, DnsError>> {
        let fallback = self.fallback.clone();
        Box::pin(async move {
            let addrs = match system_lookup(&host).await {
                Ok(addrs) => addrs,
                Err(error) => {
                    let Some(fallback) = fallback else {
                        return Err(error);
                    };
                    tracing::debug!(host, "falling back from platform system DNS");
                    let ips = fallback
                        .lookup_ipv6(&host, PLATFORM_DNS_FALLBACK_TIMEOUT)
                        .await?
                        .filter_map(|ip| match ip {
                            IpAddr::V4(_) => None,
                            IpAddr::V6(ip) => Some(ip),
                        })
                        .collect::<Vec<_>>();
                    let result: BoxIter<Ipv6Addr> = Box::new(ips.into_iter());
                    return Ok(result);
                }
            };
            let ips = ipv6_addresses(addrs);
            if !ips.is_empty() {
                let result: BoxIter<Ipv6Addr> = Box::new(ips.into_iter());
                return Ok(result);
            }

            let Some(fallback) = fallback else {
                let result: BoxIter<Ipv6Addr> = Box::new(std::iter::empty());
                return Ok(result);
            };
            tracing::debug!(
                host,
                "platform system DNS returned no IPv6 addresses; using fallback resolver"
            );
            let ips = fallback
                .lookup_ipv6(&host, PLATFORM_DNS_FALLBACK_TIMEOUT)
                .await?
                .filter_map(|ip| match ip {
                    IpAddr::V4(_) => None,
                    IpAddr::V6(ip) => Some(ip),
                })
                .collect::<Vec<_>>();
            let result: BoxIter<Ipv6Addr> = Box::new(ips.into_iter());
            Ok(result)
        })
    }

    fn lookup_txt(&self, _host: String) -> BoxFuture<Result<BoxIter<TxtRecordData>, DnsError>> {
        // Envoix clears iroh's DNS address lookup and only adds mDNS separately,
        // so these endpoints need the system resolver for relay A/AAAA records
        // only. Returning an empty TXT answer avoids routing the query back to
        // Hickory while preserving that invariant.
        Box::pin(async {
            let result: BoxIter<TxtRecordData> = Box::new(std::iter::empty());
            Ok(result)
        })
    }

    fn clear_cache(&self) {}

    fn reset(&self) -> Box<dyn Resolver> {
        Box::new(Self::default())
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) fn platform_system_dns_resolver() -> DnsResolver {
    DnsResolver::custom(PlatformSystemDnsResolver::default())
}

#[cfg(any(target_os = "ios", target_os = "android"))]
async fn system_lookup(host: &str) -> Result<Vec<SocketAddr>, DnsError> {
    match tokio::net::lookup_host((host, 0)).await {
        Ok(addrs) => {
            let addrs = addrs.collect::<Vec<_>>();
            tracing::debug!(host, ?addrs, "platform system DNS lookup completed");
            Ok(addrs)
        }
        Err(error) => {
            tracing::warn!(host, %error, "platform system DNS lookup failed");
            Err(DnsError::from(n0_error::anyerr!(
                error,
                "platform system DNS lookup failed for {host}"
            )))
        }
    }
}

#[cfg(target_os = "ios")]
fn platform_dns_fallback_server() -> Option<SocketAddr> {
    std::env::var("ENVOIX_IOS_DNS_SERVER")
        .ok()
        .and_then(|value| value.parse().ok())
}

#[cfg(target_os = "android")]
fn platform_dns_fallback_server() -> Option<SocketAddr> {
    None
}

#[cfg(any(target_os = "ios", target_os = "android", test))]
fn ipv4_addresses(addrs: impl IntoIterator<Item = SocketAddr>) -> Vec<Ipv4Addr> {
    let mut result = Vec::new();
    for ip in addrs.into_iter().filter_map(|addr| match addr.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }) {
        if !result.contains(&ip) {
            result.push(ip);
        }
    }
    result
}

#[cfg(any(target_os = "ios", target_os = "android", test))]
fn ipv6_addresses(addrs: impl IntoIterator<Item = SocketAddr>) -> Vec<Ipv6Addr> {
    let mut result = Vec::new();
    for ip in addrs.into_iter().filter_map(|addr| match addr.ip() {
        IpAddr::V4(_) => None,
        IpAddr::V6(ip) => Some(ip),
    }) {
        if !result.contains(&ip) {
            result.push(ip);
        }
    }
    result
}

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

    /// Freeze OS-assigned ports from an advertised descriptor for a later
    /// rebind. Concrete IPs are intentionally not retained: interfaces may
    /// change while a transfer is paused, while same-network peers still need
    /// the old QR's socket ports to remain valid.
    pub fn rebind_from_advertised(&self, advertised: &[SocketAddr]) -> Option<Self> {
        if self.addrs.iter().all(|bind| bind.addr.port() != 0) {
            return None;
        }
        let v4_port = advertised
            .iter()
            .find(|addr| addr.is_ipv4())
            .map(SocketAddr::port);
        let v6_port = advertised
            .iter()
            .find(|addr| addr.is_ipv6())
            .map(SocketAddr::port);
        let mut addrs = Vec::with_capacity(self.addrs.len());
        for bind in &self.addrs {
            if bind.addr.port() != 0 {
                addrs.push(*bind);
                continue;
            }
            let port = if bind.addr.is_ipv4() {
                v4_port
            } else {
                v6_port
            };
            match port {
                Some(port) => addrs.push(BindAddr {
                    addr: SocketAddr::new(bind.addr.ip(), port),
                    ..*bind
                }),
                None if bind.required => return None,
                None => {}
            }
        }
        (!addrs.is_empty()).then_some(Self { addrs })
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

    /// Wait until this endpoint has learned an address worth advertising.
    ///
    /// Direct addresses are available immediately from local sockets, while a
    /// relay home takes a round-trip to register. When `want_relay` is true we
    /// wait for the relay home so cross-network peers do not receive a
    /// direct-only address by accident.
    pub(crate) async fn ready_endpoint_addr(&self, want_relay: bool) -> EndpointAddr {
        let deadline = tokio::time::Instant::now() + ENDPOINT_ADDR_WAIT_TIMEOUT;
        loop {
            let raw = self.local_endpoint.addr();
            let ready = if want_relay {
                raw.relay_urls().next().is_some()
            } else {
                !raw.is_empty()
            };
            if ready {
                return self.endpoint_addr();
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(ENDPOINT_ADDR_WAIT_POLL).await;
        }

        let addr = self.endpoint_addr();
        if want_relay && addr.relay_urls().next().is_none() {
            let relay_status = relay_status_summary(&self.local_endpoint);
            tracing::warn!(
                relay_status,
                "relay configured but its home did not register in time; advertising a \
                 direct-only address - a peer that cannot reach us directly may fail to connect"
            );
        }
        addr
    }

    pub(crate) async fn accept_with_events(
        &self,
        events: &dyn EventSink,
    ) -> Result<IrohFrameConnection, SessionError> {
        events.on_event(TransferEvent::Diagnostic {
            message: format!(
                "accept waiting {}",
                endpoint_addr_shape(&self.endpoint_addr())
            ),
        });
        let incoming = self
            .local_endpoint
            .accept()
            .await
            .ok_or_else(|| CoreError::Transport("iroh endpoint closed".into()))?;
        events.on_event(TransferEvent::Diagnostic {
            message: "accept incoming received; awaiting connection".to_string(),
        });
        let connection = incoming
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        let alpn = connection.alpn();
        let protocol = TransferProtocol::from_alpn(alpn).ok_or_else(|| {
            CoreError::Protocol(format!(
                "unsupported negotiated ALPN {:?}",
                String::from_utf8_lossy(alpn)
            ))
        })?;
        events.on_event(TransferEvent::Diagnostic {
            message: format!(
                "accept connection established alpn={}",
                String::from_utf8_lossy(alpn)
            ),
        });
        let (send, recv) = connection
            .accept_bi()
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        events.on_event(TransferEvent::Diagnostic {
            message: "accept stream opened".to_string(),
        });
        Ok(IrohFrameConnection::new(
            self.local_endpoint.clone(),
            connection,
            send,
            recv,
            protocol,
        ))
    }
}

fn endpoint_addr_shape(addr: &EndpointAddr) -> String {
    format!(
        "endpoint={} direct={} relay={}",
        short_endpoint_id(&addr.id.to_string()),
        addr.ip_addrs().count(),
        addr.relay_urls().count()
    )
}

pub(crate) fn relay_status_summary(endpoint: &Endpoint) -> String {
    let mut watcher = endpoint.home_relay_status();
    let statuses = watcher.get();
    if statuses.is_empty() {
        return "home relay status unavailable".to_string();
    }
    statuses
        .iter()
        .map(|status| match status.last_error() {
            Some(error) => format!(
                "{} connected={} error={error:#}",
                status.url(),
                status.is_connected()
            ),
            None => format!("{} connected={}", status.url(), status.is_connected()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn short_endpoint_id(id: &str) -> &str {
    let end = id.len().min(12);
    &id[..end]
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
    window: u32,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        &[TransferProtocol::SingleFileV1],
        false,
        relay,
        relay_only,
        candidates,
        window,
    )
    .await
}

pub(crate) async fn build_transfer_accept_endpoint(
    listen_addrs: BindAddrs,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        &[TransferProtocol::SingleFileV1, TransferProtocol::ManifestV1],
        false,
        relay,
        relay_only,
        candidates,
        window,
    )
    .await
}

pub(crate) async fn build_advertising_accept_endpoint(
    listen_addrs: BindAddrs,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        &[TransferProtocol::SingleFileV1],
        true,
        relay,
        relay_only,
        candidates,
        window,
    )
    .await
}

pub(crate) async fn build_transfer_advertising_accept_endpoint(
    listen_addrs: BindAddrs,
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        Some(listen_addrs),
        identity,
        &[TransferProtocol::SingleFileV1, TransferProtocol::ManifestV1],
        true,
        relay,
        relay_only,
        candidates,
        window,
    )
    .await
}

pub(crate) async fn build_dial_endpoint(
    identity: &IdentityConfig,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<Endpoint, SessionError> {
    build_endpoint(
        None,
        identity,
        &[],
        false,
        relay,
        relay_only,
        candidates,
        window,
    )
    .await
}

/// Default per-stream QUIC flow-control window (receive and send), in bytes.
/// Four MiB bounds mobile queueing while still allowing callers to opt into a
/// larger per-session window for high-bandwidth, high-latency paths.
pub const DEFAULT_DATA_STREAM_WINDOW: u32 = 4 * 1024 * 1024;
/// Accepted range for a caller-supplied window: below ~1 MiB throttles even a
/// LAN, above 128 MiB risks excessive per-transfer memory on constrained
/// devices. A value outside this range is rejected (never silently clamped).
pub const MIN_DATA_STREAM_WINDOW: u32 = 1024 * 1024;
pub const MAX_DATA_STREAM_WINDOW: u32 = 128 * 1024 * 1024;

/// QUIC transport tuning for high-latency links (e.g. trans-Pacific, ~280 ms RTT).
///
/// quinn's default per-stream receive window is sized for a 100 ms / 100 Mbit
/// link (1.25 MB). A single stream can have at most `window / RTT` bytes in
/// flight, so at 280 ms RTT that default caps one transfer at ~4.5 MB/s no
/// matter how fast the link is. We raise the per-stream flow-control window
/// (and the matching send window) so one transfer can fill a long fat pipe;
/// iroh's holepunching/multipath defaults (from the builder) are left untouched.
///
/// `window` is frozen per session (carried on [`crate::SessionConfig`]), never a
/// global: it never enters the wire header, resume state, or any hash, so it
/// affects throughput only — concurrent sessions each keep their own value.
fn data_transport_config(window: u32) -> QuicTransportConfig {
    let builder = QuicTransportConfig::builder()
        .stream_receive_window(VarInt::from_u32(window))
        .send_window(window as u64);
    // Keep noq's stable CUBIC default. noq-proto 1.0.x BBRv3 can underflow in
    // `inflight_at_loss` on a lossy path and panic the whole mobile process.
    // BBRv3 must not be re-enabled until that upstream invariant is fixed and
    // covered by a lossy-link regression test.
    builder.build()
}

// The endpoint knobs are independent flags/handles, not a cohesive config worth
// its own type; the three thin wrappers above pin the common combinations.
#[allow(clippy::too_many_arguments)]
async fn build_endpoint(
    local_listen_addrs: Option<BindAddrs>,
    identity: &IdentityConfig,
    accepted_protocols: &[TransferProtocol],
    advertise_self: bool,
    relay: &Option<String>,
    relay_only: bool,
    candidates: &CandidateFilter,
    window: u32,
) -> Result<Endpoint, SessionError> {
    let secret_key = load_secret_key(identity).await?;
    let builder = Endpoint::builder(presets::N0);
    // Defined by build.rs only when the NAT harness supplies its generated CA.
    #[cfg(envoix_nat_test_local_ca)]
    let builder = builder.ca_tls_config(
        iroh::tls::CaTlsConfig::embedded().with_extra_roots([include_bytes!(env!(
            "ENVOIX_NAT_TEST_CA_DER_PATH"
        ))
        .to_vec()
        .into()]),
    );
    let mut builder = builder
        .secret_key(secret_key)
        .relay_mode(relay_mode(relay)?)
        .transport_config(data_transport_config(window))
        .clear_address_lookup();
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        builder = builder.dns_resolver(platform_system_dns_resolver());
    }
    if !accepted_protocols.is_empty() {
        builder = builder.alpns(
            accepted_protocols
                .iter()
                .map(|protocol| protocol.alpn().to_vec())
                .collect(),
        );
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
}
