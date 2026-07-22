use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutKind {
    Discovery,
    Connect,
    Pairing,
    Authentication,
    CompletionAck,
    TransportIdle,
    GracefulClose,
}

impl fmt::Display for TimeoutKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery => formatter.write_str("discovery"),
            Self::Connect => formatter.write_str("connect"),
            Self::Pairing => formatter.write_str("pairing"),
            Self::Authentication => formatter.write_str("authentication"),
            Self::CompletionAck => formatter.write_str("completion ACK"),
            Self::TransportIdle => formatter.write_str("transport idle"),
            Self::GracefulClose => formatter.write_str("graceful close"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeoutError {
    Zero(TimeoutKind),
    OuterMustExceedInner {
        outer: TimeoutKind,
        inner: TimeoutKind,
    },
}

impl fmt::Display for TimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero(kind) => write!(formatter, "{kind} timeout must be non-zero"),
            Self::OuterMustExceedInner { outer, inner } => {
                write!(formatter, "{outer} timeout must exceed {inner} timeout")
            }
        }
    }
}

impl std::error::Error for TimeoutError {}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeoutOverrides {
    pub discovery: Option<Duration>,
    pub connect: Option<Duration>,
    pub pairing: Option<Duration>,
    pub authentication: Option<Duration>,
    pub completion_ack: Option<Duration>,
    pub transport_idle: Option<Duration>,
    pub graceful_close: Option<Duration>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Timeouts {
    discovery: Duration,
    connect: Duration,
    pairing: Duration,
    authentication: Duration,
    completion_ack: Duration,
    transport_idle: Duration,
    graceful_close: Duration,
}

impl Timeouts {
    pub fn new(
        discovery: Duration,
        connect: Duration,
        pairing: Duration,
        authentication: Duration,
        completion_ack: Duration,
        transport_idle: Duration,
        graceful_close: Duration,
    ) -> Result<Self, TimeoutError> {
        let timeouts = Self {
            discovery,
            connect,
            pairing,
            authentication,
            completion_ack,
            transport_idle,
            graceful_close,
        };
        timeouts.validate()?;
        Ok(timeouts)
    }

    pub fn standard() -> Self {
        Self::new(
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(30),
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(90),
            Duration::from_secs(10),
        )
        .expect("standard timeout ordering is valid")
    }

    pub fn discovery(&self) -> Duration {
        self.discovery
    }

    pub fn connect(&self) -> Duration {
        self.connect
    }

    pub fn pairing(&self) -> Duration {
        self.pairing
    }

    pub fn authentication(&self) -> Duration {
        self.authentication
    }

    pub fn completion_ack(&self) -> Duration {
        self.completion_ack
    }

    pub fn transport_idle(&self) -> Duration {
        self.transport_idle
    }

    pub fn graceful_close(&self) -> Duration {
        self.graceful_close
    }

    pub(crate) fn resolve(&self, overrides: &TimeoutOverrides) -> Result<Self, TimeoutError> {
        Self::new(
            overrides.discovery.unwrap_or(self.discovery),
            overrides.connect.unwrap_or(self.connect),
            overrides.pairing.unwrap_or(self.pairing),
            overrides.authentication.unwrap_or(self.authentication),
            overrides.completion_ack.unwrap_or(self.completion_ack),
            overrides.transport_idle.unwrap_or(self.transport_idle),
            overrides.graceful_close.unwrap_or(self.graceful_close),
        )
    }

    pub(crate) fn validate(&self) -> Result<(), TimeoutError> {
        for (kind, value) in [
            (TimeoutKind::Discovery, self.discovery),
            (TimeoutKind::Connect, self.connect),
            (TimeoutKind::Pairing, self.pairing),
            (TimeoutKind::Authentication, self.authentication),
            (TimeoutKind::CompletionAck, self.completion_ack),
            (TimeoutKind::TransportIdle, self.transport_idle),
            (TimeoutKind::GracefulClose, self.graceful_close),
        ] {
            if value.is_zero() {
                return Err(TimeoutError::Zero(kind));
            }
        }

        require_outer(
            TimeoutKind::Pairing,
            self.pairing,
            TimeoutKind::GracefulClose,
            self.graceful_close,
        )?;
        for (inner, value) in [
            (TimeoutKind::Pairing, self.pairing),
            (TimeoutKind::Authentication, self.authentication),
            (TimeoutKind::CompletionAck, self.completion_ack),
        ] {
            require_outer(
                TimeoutKind::TransportIdle,
                self.transport_idle,
                inner,
                value,
            )?;
        }
        Ok(())
    }
}

fn require_outer(
    outer: TimeoutKind,
    outer_value: Duration,
    inner: TimeoutKind,
    inner_value: Duration,
) -> Result<(), TimeoutError> {
    if outer_value > inner_value {
        Ok(())
    } else {
        Err(TimeoutError::OuterMustExceedInner { outer, inner })
    }
}
