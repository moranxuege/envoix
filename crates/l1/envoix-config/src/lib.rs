//! Frozen attempt configuration, timeout ownership, and retry scheduling contracts.

#![forbid(unsafe_code)]

mod config;
mod retry;
mod timeout;
mod transport;

pub use config::{
    ConfigDefaults, ConfigError, ConfigResolver, EffectiveConfig, RawConfig, Reachability,
    ResolvedTransport, TransferTuning, TuningField, UserPolicy,
};
pub use retry::{
    MonotonicClock, RetryPermit, RetryRefusal, RetryScheduleError, RetryScheduleState, RetryTimer,
    SchedulerToken, TimerError,
};
pub use timeout::{TimeoutError, TimeoutKind, TimeoutOverrides, Timeouts};
pub use transport::{
    CandidatePolicy, CandidateRule, CandidateSet, DataPathPolicy, MailboxEndpoint, RelayEndpoint,
    RendezvousEndpoint, RendezvousFallback, TransportConfigError, TransportPolicy,
};
