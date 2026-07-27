use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use envoix_rendezvous::ClientConfig;
use iroh::{RelayUrl, SecretKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigField {
    BindDeadline,
    HandshakeDeadline,
    ConnectDeadline,
    StreamDeadline,
    CloseDeadline,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BindDeadline => formatter.write_str("endpoint bind deadline"),
            Self::HandshakeDeadline => formatter.write_str("server handshake deadline"),
            Self::ConnectDeadline => formatter.write_str("broker connect deadline"),
            Self::StreamDeadline => formatter.write_str("broker stream deadline"),
            Self::CloseDeadline => formatter.write_str("broker close deadline"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    ZeroDuration { field: ConfigField },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration { field } => write!(formatter, "{field} must be non-zero"),
        }
    }
}

impl std::error::Error for ConfigError {}

pub struct EndpointConfig {
    pub bind: SocketAddr,
    pub relay: Option<RelayUrl>,
    pub secret_key: SecretKey,
    bind_deadline: Duration,
}

impl EndpointConfig {
    pub fn new(
        bind: SocketAddr,
        relay: Option<RelayUrl>,
        secret_key: SecretKey,
        bind_deadline: Duration,
    ) -> Result<Self, ConfigError> {
        if bind_deadline.is_zero() {
            return Err(ConfigError::ZeroDuration {
                field: ConfigField::BindDeadline,
            });
        }
        Ok(Self {
            bind,
            relay,
            secret_key,
            bind_deadline,
        })
    }

    pub const fn bind_deadline(&self) -> Duration {
        self.bind_deadline
    }
}

/// How the adapter serves a connection it has been given. How MANY it may be
/// given is a budget, and budgets belong to whoever is being budgeted — see
/// [`crate::ConnectionAdmission`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrohServerConfig {
    handshake_deadline: Duration,
}

impl IrohServerConfig {
    pub fn new(handshake_deadline: Duration) -> Result<Self, ConfigError> {
        if handshake_deadline.is_zero() {
            return Err(ConfigError::ZeroDuration {
                field: ConfigField::HandshakeDeadline,
            });
        }
        Ok(Self { handshake_deadline })
    }

    pub const fn handshake_deadline(self) -> Duration {
        self.handshake_deadline
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrohClientConfig {
    connect_deadline: Duration,
    stream_deadline: Duration,
    close_deadline: Duration,
    rendezvous: ClientConfig,
}

impl IrohClientConfig {
    pub fn new(
        connect_deadline: Duration,
        stream_deadline: Duration,
        close_deadline: Duration,
        rendezvous: ClientConfig,
    ) -> Result<Self, ConfigError> {
        for (field, value) in [
            (ConfigField::ConnectDeadline, connect_deadline),
            (ConfigField::StreamDeadline, stream_deadline),
            (ConfigField::CloseDeadline, close_deadline),
        ] {
            if value.is_zero() {
                return Err(ConfigError::ZeroDuration { field });
            }
        }
        Ok(Self {
            connect_deadline,
            stream_deadline,
            close_deadline,
            rendezvous,
        })
    }

    pub const fn connect_deadline(self) -> Duration {
        self.connect_deadline
    }

    pub const fn stream_deadline(self) -> Duration {
        self.stream_deadline
    }

    pub const fn close_deadline(self) -> Duration {
        self.close_deadline
    }

    pub const fn rendezvous(self) -> ClientConfig {
        self.rendezvous
    }
}
