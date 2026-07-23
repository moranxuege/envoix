use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use iroh::RelayUrl;

pub const DEFAULT_DATA_STREAM_WINDOW: u32 = 16 * 1024 * 1024;
pub const MIN_DATA_STREAM_WINDOW: u32 = 1024 * 1024;
pub const MAX_DATA_STREAM_WINDOW: u32 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_AUTH_FAILURES: u16 = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    FlowWindowOutOfRange { actual: u64 },
    ZeroTimeout { kind: WaitKind },
    ZeroAuthFailureLimit,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlowWindowOutOfRange { actual } => write!(
                formatter,
                "data stream window {actual} must be between \
                 {MIN_DATA_STREAM_WINDOW} and {MAX_DATA_STREAM_WINDOW} bytes"
            ),
            Self::ZeroTimeout { kind } => write!(formatter, "{kind} timeout must be non-zero"),
            Self::ZeroAuthFailureLimit => {
                formatter.write_str("authentication failure limit must be non-zero")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitKind {
    Bind,
    Connect,
    Stream,
    PeerClose,
    EndpointClose,
}

impl fmt::Display for WaitKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind => formatter.write_str("bind"),
            Self::Connect => formatter.write_str("connect"),
            Self::Stream => formatter.write_str("stream"),
            Self::PeerClose => formatter.write_str("peer close"),
            Self::EndpointClose => formatter.write_str("endpoint close"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowWindow(u32);

impl FlowWindow {
    pub fn new(bytes: u64) -> Result<Self, ConfigError> {
        if !(u64::from(MIN_DATA_STREAM_WINDOW)..=u64::from(MAX_DATA_STREAM_WINDOW)).contains(&bytes)
        {
            return Err(ConfigError::FlowWindowOutOfRange { actual: bytes });
        }
        Ok(Self(bytes as u32))
    }

    pub const fn bytes(self) -> u32 {
        self.0
    }
}

impl Default for FlowWindow {
    fn default() -> Self {
        Self(DEFAULT_DATA_STREAM_WINDOW)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CongestionControl {
    #[default]
    Bbr3,
    Cubic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionTransportConfig {
    pub flow_window: FlowWindow,
    pub congestion: CongestionControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindAddresses {
    pub ipv4: SocketAddrV4,
    pub ipv6: Option<SocketAddrV6>,
}

impl BindAddresses {
    pub const fn dual_stack(port: u16) -> Self {
        Self {
            ipv4: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port),
            ipv6: Some(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)),
        }
    }

    pub const fn ipv4_only(ipv4: SocketAddrV4) -> Self {
        Self { ipv4, ipv6: None }
    }

    pub fn socket_addrs(self) -> impl Iterator<Item = SocketAddr> {
        std::iter::once(SocketAddr::V4(self.ipv4)).chain(self.ipv6.map(SocketAddr::V6))
    }
}

#[derive(Clone, Debug)]
pub struct SessionEndpointConfig {
    pub bind: BindAddresses,
    /// `None` disables relays; `Some` installs exactly one caller-selected relay.
    pub relay: Option<RelayUrl>,
    pub transport: SessionTransportConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionTimeouts {
    bind: Duration,
    connect: Duration,
    stream: Duration,
    peer_close: Duration,
    endpoint_close: Duration,
}

impl SessionTimeouts {
    pub fn new(
        bind: Duration,
        connect: Duration,
        stream: Duration,
        peer_close: Duration,
        endpoint_close: Duration,
    ) -> Result<Self, ConfigError> {
        let timeouts = Self {
            bind,
            connect,
            stream,
            peer_close,
            endpoint_close,
        };
        for (kind, value) in [
            (WaitKind::Bind, bind),
            (WaitKind::Connect, connect),
            (WaitKind::Stream, stream),
            (WaitKind::PeerClose, peer_close),
            (WaitKind::EndpointClose, endpoint_close),
        ] {
            if value.is_zero() {
                return Err(ConfigError::ZeroTimeout { kind });
            }
        }
        Ok(timeouts)
    }

    pub const fn bind(self) -> Duration {
        self.bind
    }

    pub const fn connect(self) -> Duration {
        self.connect
    }

    pub const fn stream(self) -> Duration {
        self.stream
    }

    pub const fn peer_close(self) -> Duration {
        self.peer_close
    }

    pub const fn endpoint_close(self) -> Duration {
        self.endpoint_close
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthFailureBudget {
    maximum: u16,
    failures: u16,
}

impl AuthFailureBudget {
    pub fn new(maximum: u16) -> Result<Self, ConfigError> {
        if maximum == 0 {
            return Err(ConfigError::ZeroAuthFailureLimit);
        }
        Ok(Self {
            maximum,
            failures: 0,
        })
    }

    /// Returns `false` once another unauthenticated candidate may not be accepted.
    pub fn record_failure(&mut self) -> bool {
        self.failures = self.failures.saturating_add(1);
        self.failures < self.maximum
    }

    pub const fn failures(self) -> u16 {
        self.failures
    }

    pub const fn maximum(self) -> u16 {
        self.maximum
    }
}
