use std::time::Duration;

/// Token-bucket policy: `events` tokens are replenished per `period`, up to
/// `burst`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    pub events: u32,
    pub period: Duration,
    pub burst: u32,
}

impl RateLimitConfig {
    pub const fn per_minute(events: u32, burst: u32) -> Self {
        Self {
            events,
            period: Duration::from_secs(60),
            burst,
        }
    }

    pub(crate) fn validate(self) -> Result<(), &'static str> {
        if self.events == 0 || self.burst == 0 || self.period.is_zero() {
            return Err("rate limits must have non-zero events, period, and burst");
        }
        Ok(())
    }
}

/// Complete Room-service resource and abuse policy.
///
/// Defaults are deployment starting points, not protocol constants. The server
/// exposes every field as a command-line option so production values can be
/// tuned from load and shared-NAT testing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerConfig {
    pub room_ttl: Duration,
    pub room_tombstone_ttl: Duration,
    pub room_attempt_limit: u32,
    pub room_attempt_rate: RateLimitConfig,
    pub endpoint_join_rate: RateLimitConfig,
    pub ip_join_rate: RateLimitConfig,
    pub subnet_join_rate: RateLimitConfig,
    pub max_connections: usize,
    pub max_connections_per_endpoint: usize,
    pub max_connections_per_room: usize,
    pub max_room_states: usize,
    pub max_waiting_creators: usize,
    pub max_source_states: usize,
    pub source_state_ttl: Duration,
    pub handshake_timeout: Duration,
    pub join_timeout: Duration,
    pub relay_ttl: Duration,
    pub relay_idle_timeout: Duration,
    pub slow_frame_timeout: Duration,
    pub close_grace: Duration,
    pub max_frame_body: usize,
    pub max_retry_after: Duration,
    pub unavailable_retry_after: Duration,
}

impl BrokerConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.room_attempt_rate.validate()?;
        self.endpoint_join_rate.validate()?;
        self.ip_join_rate.validate()?;
        self.subnet_join_rate.validate()?;
        if self.room_ttl.is_zero()
            || self.room_tombstone_ttl.is_zero()
            || self.source_state_ttl.is_zero()
            || self.handshake_timeout.is_zero()
            || self.join_timeout.is_zero()
            || self.relay_ttl.is_zero()
            || self.relay_idle_timeout.is_zero()
            || self.slow_frame_timeout.is_zero()
            || self.close_grace.is_zero()
            || self.max_retry_after.is_zero()
        {
            return Err("broker durations must be non-zero");
        }
        if self.room_attempt_limit == 0
            || self.max_connections == 0
            || self.max_connections_per_endpoint == 0
            || self.max_connections_per_room < 2
            || self.max_room_states == 0
            || self.max_waiting_creators == 0
            || self.max_source_states < 3
            || self.max_frame_body == 0
            || self.max_frame_body > u32::MAX as usize
        {
            return Err("broker limits must be non-zero and room connections must be at least two");
        }
        if self.max_waiting_creators > self.max_room_states {
            return Err("waiting-creator limit cannot exceed the Room-state limit");
        }
        if self.unavailable_retry_after > self.max_retry_after {
            return Err("unavailable retry_after cannot exceed the server retry_after cap");
        }
        Ok(())
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            room_ttl: Duration::from_secs(300),
            room_tombstone_ttl: Duration::from_secs(300),
            room_attempt_limit: 6,
            room_attempt_rate: RateLimitConfig {
                events: 6,
                period: Duration::from_secs(300),
                burst: 2,
            },
            endpoint_join_rate: RateLimitConfig::per_minute(10, 20),
            ip_join_rate: RateLimitConfig::per_minute(30, 60),
            subnet_join_rate: RateLimitConfig::per_minute(120, 240),
            max_connections: 256,
            max_connections_per_endpoint: 8,
            max_connections_per_room: 2,
            max_room_states: 8192,
            max_waiting_creators: 4096,
            max_source_states: 8192,
            source_state_ttl: Duration::from_secs(600),
            handshake_timeout: Duration::from_secs(10),
            join_timeout: Duration::from_secs(10),
            relay_ttl: Duration::from_secs(120),
            relay_idle_timeout: Duration::from_secs(30),
            slow_frame_timeout: Duration::from_secs(10),
            close_grace: Duration::from_secs(10),
            max_frame_body: 64 * 1024,
            max_retry_after: Duration::from_secs(300),
            unavailable_retry_after: Duration::from_secs(1),
        }
    }
}
