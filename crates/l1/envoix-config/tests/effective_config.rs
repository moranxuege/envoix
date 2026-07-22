use std::cell::Cell;
use std::time::Duration;

use envoix_config::{
    CandidatePolicy, CandidateSet, ConfigDefaults, ConfigError, ConfigResolver, DataPathPolicy,
    MailboxEndpoint, MonotonicClock, RawConfig, Reachability, RelayEndpoint, RendezvousEndpoint,
    RendezvousFallback, ResolvedTransport, RetryRefusal, RetryScheduleState, RetryTimer,
    TimeoutError, TimeoutKind, TimeoutOverrides, Timeouts, TransferTuning, TransportConfigError,
    TransportPolicy,
};
use envoix_types::ByteCount;

fn rendezvous_policy() -> TransportPolicy {
    TransportPolicy::Rendezvous {
        endpoint: RendezvousEndpoint::new("node@rdz.test:9445").unwrap(),
        data_path: DataPathPolicy::Automatic {
            relay: Some(RelayEndpoint::new("https://relay.test:9444").unwrap()),
        },
        candidates: CandidatePolicy::Any,
        fallback: RendezvousFallback::LocalDiscovery,
    }
}

fn defaults() -> ConfigDefaults {
    ConfigDefaults::new(
        rendezvous_policy(),
        TransferTuning::new(ByteCount::new(64 * 1024), ByteCount::new(16 * 1024 * 1024)).unwrap(),
        MailboxEndpoint::new("https://mailbox.test:9460").unwrap(),
        Timeouts::standard(),
    )
    .unwrap()
}

#[test]
fn frozen_config_and_timeout_ordering() {
    let resolver = ConfigResolver::new(defaults());
    let mut raw = RawConfig {
        chunk_size: Some(ByteCount::new(128 * 1024)),
        ..RawConfig::default()
    };
    let effective = resolver
        .resolve(&raw, Reachability::InternetAndLocal)
        .unwrap();

    assert_eq!(
        effective.transport(),
        ResolvedTransport::RendezvousThenLocal
    );
    assert_eq!(effective.tuning().chunk_size(), ByteCount::new(128 * 1024));
    assert_eq!(
        effective.tuning().data_stream_window(),
        ByteCount::new(16 * 1024 * 1024)
    );
    assert!(effective.timeouts().pairing() > effective.timeouts().graceful_close());
    assert!(effective.timeouts().transport_idle() > effective.timeouts().completion_ack());
    assert!(effective.timeouts().transport_idle() > effective.timeouts().authentication());

    let frozen_policy = effective.user_policy().clone();
    raw.chunk_size = Some(ByteCount::new(1024 * 1024));
    raw.transport = Some(TransportPolicy::LocalDiscovery {
        candidates: CandidatePolicy::Any,
    });
    assert_eq!(effective.user_policy(), &frozen_policy);
    assert_eq!(effective.tuning().chunk_size(), ByteCount::new(128 * 1024));

    let inverted = RawConfig {
        timeouts: TimeoutOverrides {
            pairing: Some(Duration::from_secs(8)),
            graceful_close: Some(Duration::from_secs(10)),
            ..TimeoutOverrides::default()
        },
        ..RawConfig::default()
    };
    assert!(matches!(
        resolver.resolve(&inverted, Reachability::InternetAndLocal),
        Err(ConfigError::Timeout(TimeoutError::OuterMustExceedInner {
            outer: TimeoutKind::Pairing,
            inner: TimeoutKind::GracefulClose,
        }))
    ));
}

#[test]
fn only_persisted_scheduler_token_spends_retry() {
    let mut schedule = RetryScheduleState::default();
    let stale = schedule.schedule().unwrap();
    let current = schedule.schedule().unwrap();

    let persisted = serde_json::to_string(&schedule).unwrap();
    let mut restored: RetryScheduleState = serde_json::from_str(&persisted).unwrap();
    assert_eq!(restored.current(), Some(current));
    assert_eq!(restored.spend(stale), Err(RetryRefusal::NotCurrent));

    let permit = restored.spend(current).unwrap();
    assert_eq!(permit.into_token(), current);
    assert_eq!(restored.spend(current), Err(RetryRefusal::NotCurrent));
    assert_eq!(restored.current(), None);

    assert!(
        serde_json::from_str::<RetryScheduleState>(r#"{"last_issued":1,"current":2}"#).is_err()
    );
}

#[test]
fn restore_keeps_policy_reresolves_reachability() {
    let resolver = ConfigResolver::new(defaults());
    let initial = resolver
        .resolve(&RawConfig::default(), Reachability::InternetAndLocal)
        .unwrap();
    let durable_json = serde_json::to_string(initial.user_policy()).unwrap();
    assert!(!durable_json.contains("internet_and_local"));
    assert!(!durable_json.contains("internet_only"));

    let durable_policy = serde_json::from_str(&durable_json).unwrap();
    let local = ConfigResolver::restore(durable_policy, Reachability::LocalOnly).unwrap();
    assert_eq!(local.transport(), ResolvedTransport::LocalDiscovery);
    assert_eq!(local.user_policy(), initial.user_policy());

    let durable_policy = serde_json::from_str(&durable_json).unwrap();
    let internet = ConfigResolver::restore(durable_policy, Reachability::InternetOnly).unwrap();
    assert_eq!(internet.transport(), ResolvedTransport::Rendezvous);
    assert_eq!(internet.user_policy(), initial.user_policy());
}

#[test]
fn closed_candidate_policy_rejects_an_empty_filtered_set() {
    assert_eq!(
        CandidateSet::new(Vec::new()),
        Err(TransportConfigError::EmptyCandidateSet)
    );
    assert!(serde_json::from_str::<CandidateSet>("[]").is_err());
}

struct ManualClock {
    millis: Cell<u64>,
}

impl ManualClock {
    fn new() -> Self {
        Self {
            millis: Cell::new(0),
        }
    }

    fn advance(&self, duration: Duration) {
        self.millis.set(
            self.millis
                .get()
                .checked_add(u64::try_from(duration.as_millis()).unwrap())
                .unwrap(),
        );
    }
}

impl MonotonicClock for ManualClock {
    type Instant = u64;

    fn now(&self) -> Self::Instant {
        self.millis.get()
    }

    fn checked_add(&self, instant: Self::Instant, duration: Duration) -> Option<Self::Instant> {
        instant.checked_add(u64::try_from(duration.as_millis()).ok()?)
    }
}

#[test]
fn retry_timer_uses_injected_monotonic_clock() {
    let mut schedule = RetryScheduleState::default();
    let token = schedule.schedule().unwrap();
    let clock = ManualClock::new();
    let timer = RetryTimer::after(&clock, token, Duration::from_secs(5)).unwrap();

    assert!(!timer.is_due(&clock));
    clock.advance(Duration::from_secs(5));
    assert!(timer.is_due(&clock));
    assert_eq!(timer.token(), token);
}
