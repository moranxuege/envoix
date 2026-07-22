use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SchedulerToken(u64);

impl SchedulerToken {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for SchedulerToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        if value == 0 {
            return Err(D::Error::custom("scheduler token must be non-zero"));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for SchedulerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryScheduleError {
    TokenExhausted,
}

impl fmt::Display for RetryScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("retry scheduler token space exhausted")
    }
}

impl std::error::Error for RetryScheduleError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryRefusal {
    NotCurrent,
}

impl fmt::Display for RetryRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("retry token is stale, duplicate, or not scheduled")
    }
}

impl std::error::Error for RetryRefusal {}

/// Durable state that must be committed after scheduling or spending a token.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RetryScheduleState {
    last_issued: u64,
    current: Option<SchedulerToken>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryScheduleWire {
    last_issued: u64,
    current: Option<SchedulerToken>,
}

impl<'de> Deserialize<'de> for RetryScheduleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RetryScheduleWire::deserialize(deserializer)?;
        if wire
            .current
            .is_some_and(|token| token.get() != wire.last_issued)
        {
            return Err(D::Error::custom(
                "current scheduler token must equal the last issued token",
            ));
        }
        Ok(Self {
            last_issued: wire.last_issued,
            current: wire.current,
        })
    }
}

impl RetryScheduleState {
    pub fn schedule(&mut self) -> Result<SchedulerToken, RetryScheduleError> {
        self.last_issued = self
            .last_issued
            .checked_add(1)
            .ok_or(RetryScheduleError::TokenExhausted)?;
        let token = SchedulerToken(self.last_issued);
        self.current = Some(token);
        Ok(token)
    }

    pub const fn current(&self) -> Option<SchedulerToken> {
        self.current
    }

    pub fn spend(&mut self, presented: SchedulerToken) -> Result<RetryPermit, RetryRefusal> {
        if self.current != Some(presented) {
            return Err(RetryRefusal::NotCurrent);
        }
        self.current = None;
        Ok(RetryPermit(presented))
    }
}

/// A launch capability produced only by atomically spending the current token.
#[derive(Debug, Eq, PartialEq)]
pub struct RetryPermit(SchedulerToken);

impl RetryPermit {
    pub const fn into_token(self) -> SchedulerToken {
        self.0
    }
}

pub trait MonotonicClock {
    type Instant: Copy + Ord;

    fn now(&self) -> Self::Instant;
    fn checked_add(&self, instant: Self::Instant, duration: Duration) -> Option<Self::Instant>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    DeadlineOverflow,
}

impl fmt::Display for TimerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("retry deadline exceeds monotonic clock range")
    }
}

impl std::error::Error for TimerError {}

/// Transient deadline. Only its token, never its monotonic instant, is durable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryTimer<I> {
    token: SchedulerToken,
    not_before: I,
}

impl<I: Copy + Ord> RetryTimer<I> {
    pub fn after<C>(clock: &C, token: SchedulerToken, delay: Duration) -> Result<Self, TimerError>
    where
        C: MonotonicClock<Instant = I>,
    {
        let now = clock.now();
        let not_before = clock
            .checked_add(now, delay)
            .ok_or(TimerError::DeadlineOverflow)?;
        Ok(Self { token, not_before })
    }

    pub fn is_due<C>(&self, clock: &C) -> bool
    where
        C: MonotonicClock<Instant = I>,
    {
        clock.now() >= self.not_before
    }

    pub const fn token(&self) -> SchedulerToken {
        self.token
    }
}
