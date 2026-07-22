use std::fmt;

use envoix_types::ByteCount;
use serde::{Deserialize, Serialize};

use crate::timeout::{TimeoutError, TimeoutOverrides, Timeouts};
use crate::transport::{MailboxEndpoint, RendezvousFallback, TransportPolicy};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuningField {
    ChunkSize,
    DataStreamWindow,
}

impl fmt::Display for TuningField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChunkSize => formatter.write_str("chunk size"),
            Self::DataStreamWindow => formatter.write_str("data stream window"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    Timeout(TimeoutError),
    ZeroTuning(TuningField),
    NoReachableTransport,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(error) => error.fmt(formatter),
            Self::ZeroTuning(field) => write!(formatter, "{field} must be non-zero"),
            Self::NoReachableTransport => {
                formatter.write_str("current reachability cannot satisfy transport policy")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<TimeoutError> for ConfigError {
    fn from(error: TimeoutError) -> Self {
        Self::Timeout(error)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransferTuning {
    chunk_size: ByteCount,
    data_stream_window: ByteCount,
}

impl TransferTuning {
    pub fn new(chunk_size: ByteCount, data_stream_window: ByteCount) -> Result<Self, ConfigError> {
        let tuning = Self {
            chunk_size,
            data_stream_window,
        };
        tuning.validate()?;
        Ok(tuning)
    }

    pub fn chunk_size(&self) -> ByteCount {
        self.chunk_size
    }

    pub fn data_stream_window(&self) -> ByteCount {
        self.data_stream_window
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.chunk_size.get() == 0 {
            return Err(ConfigError::ZeroTuning(TuningField::ChunkSize));
        }
        if self.data_stream_window.get() == 0 {
            return Err(ConfigError::ZeroTuning(TuningField::DataStreamWindow));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RawConfig {
    pub transport: Option<TransportPolicy>,
    pub chunk_size: Option<ByteCount>,
    pub data_stream_window: Option<ByteCount>,
    pub receipt_server: Option<MailboxEndpoint>,
    pub timeouts: TimeoutOverrides,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDefaults {
    transport: TransportPolicy,
    tuning: TransferTuning,
    receipt_server: MailboxEndpoint,
    timeouts: Timeouts,
}

impl ConfigDefaults {
    pub fn new(
        transport: TransportPolicy,
        tuning: TransferTuning,
        receipt_server: MailboxEndpoint,
        timeouts: Timeouts,
    ) -> Result<Self, ConfigError> {
        tuning.validate()?;
        timeouts.validate()?;
        Ok(Self {
            transport,
            tuning,
            receipt_server,
            timeouts,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserPolicy {
    transport: TransportPolicy,
    tuning: TransferTuning,
    receipt_server: MailboxEndpoint,
    timeouts: Timeouts,
}

impl UserPolicy {
    pub fn transport(&self) -> &TransportPolicy {
        &self.transport
    }

    pub fn tuning(&self) -> &TransferTuning {
        &self.tuning
    }

    pub fn receipt_server(&self) -> &MailboxEndpoint {
        &self.receipt_server
    }

    pub fn timeouts(&self) -> &Timeouts {
        &self.timeouts
    }

    fn validate(&self) -> Result<(), ConfigError> {
        self.tuning.validate()?;
        self.timeouts.validate()?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reachability {
    Offline,
    LocalOnly,
    InternetOnly,
    InternetAndLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedTransport {
    LocalDiscovery,
    Rendezvous,
    RendezvousThenLocal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveConfig {
    user_policy: UserPolicy,
    transport: ResolvedTransport,
}

impl EffectiveConfig {
    pub fn user_policy(&self) -> &UserPolicy {
        &self.user_policy
    }

    pub fn transport(&self) -> ResolvedTransport {
        self.transport
    }

    pub fn tuning(&self) -> &TransferTuning {
        self.user_policy.tuning()
    }

    pub fn receipt_server(&self) -> &MailboxEndpoint {
        self.user_policy.receipt_server()
    }

    pub fn timeouts(&self) -> &Timeouts {
        self.user_policy.timeouts()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigResolver {
    defaults: ConfigDefaults,
}

impl ConfigResolver {
    pub const fn new(defaults: ConfigDefaults) -> Self {
        Self { defaults }
    }

    pub fn resolve(
        &self,
        raw: &RawConfig,
        reachability: Reachability,
    ) -> Result<EffectiveConfig, ConfigError> {
        let tuning = TransferTuning::new(
            raw.chunk_size.unwrap_or(self.defaults.tuning.chunk_size()),
            raw.data_stream_window
                .unwrap_or(self.defaults.tuning.data_stream_window()),
        )?;
        let user_policy = UserPolicy {
            transport: raw
                .transport
                .clone()
                .unwrap_or_else(|| self.defaults.transport.clone()),
            tuning,
            receipt_server: raw
                .receipt_server
                .clone()
                .unwrap_or_else(|| self.defaults.receipt_server.clone()),
            timeouts: self.defaults.timeouts.resolve(&raw.timeouts)?,
        };
        Self::restore(user_policy, reachability)
    }

    pub fn restore(
        user_policy: UserPolicy,
        reachability: Reachability,
    ) -> Result<EffectiveConfig, ConfigError> {
        user_policy.validate()?;
        let transport = resolve_transport(&user_policy.transport, reachability)?;
        Ok(EffectiveConfig {
            user_policy,
            transport,
        })
    }
}

fn resolve_transport(
    policy: &TransportPolicy,
    reachability: Reachability,
) -> Result<ResolvedTransport, ConfigError> {
    let resolved = match policy {
        TransportPolicy::LocalDiscovery { .. } => match reachability {
            Reachability::LocalOnly | Reachability::InternetAndLocal => {
                Some(ResolvedTransport::LocalDiscovery)
            }
            Reachability::Offline | Reachability::InternetOnly => None,
        },
        TransportPolicy::Rendezvous { fallback, .. } => match (reachability, fallback) {
            (Reachability::InternetAndLocal, RendezvousFallback::LocalDiscovery) => {
                Some(ResolvedTransport::RendezvousThenLocal)
            }
            (Reachability::InternetOnly | Reachability::InternetAndLocal, _) => {
                Some(ResolvedTransport::Rendezvous)
            }
            (Reachability::LocalOnly, RendezvousFallback::LocalDiscovery) => {
                Some(ResolvedTransport::LocalDiscovery)
            }
            (Reachability::Offline | Reachability::LocalOnly, RendezvousFallback::Disabled) => None,
            (Reachability::Offline, RendezvousFallback::LocalDiscovery) => None,
        },
    };
    resolved.ok_or(ConfigError::NoReachableTransport)
}
