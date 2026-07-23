use std::fmt;

use envoix_outcomes::OutcomeCode;

use crate::{ConfigError, ControlError, RejectionReason};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoOperation {
    ReadControl,
    WriteControl,
    Relay,
    Shutdown,
}

impl fmt::Display for IoOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadControl => formatter.write_str("read rendezvous control"),
            Self::WriteControl => formatter.write_str("write rendezvous control"),
            Self::Relay => formatter.write_str("relay rendezvous bytes"),
            Self::Shutdown => formatter.write_str("close rendezvous stream"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitKind {
    Join,
    Room,
    Relay,
    Close,
    Reply,
}

impl fmt::Display for WaitKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Join => formatter.write_str("join"),
            Self::Room => formatter.write_str("room"),
            Self::Relay => formatter.write_str("relay"),
            Self::Close => formatter.write_str("close"),
            Self::Reply => formatter.write_str("reply"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RendezvousError {
    InvalidConfig(ConfigError),
    Control(ControlError),
    Io { operation: IoOperation },
    Deadline { wait: WaitKind },
    Rejected(RejectionReason),
    Expired,
    PeerClosed,
    RegistryUnavailable,
    WaiterIdExhausted,
}

impl RendezvousError {
    pub const fn outcome_code(&self) -> OutcomeCode {
        match self {
            Self::Control(ControlError::UnsupportedVersion) => OutcomeCode::VersionMismatch,
            Self::Deadline { .. } | Self::Expired => OutcomeCode::Timeout,
            Self::PeerClosed => OutcomeCode::PeerLost,
            Self::Io { .. } => OutcomeCode::NetworkUnreachable,
            Self::Rejected(RejectionReason::WaitingRoomsFull) => OutcomeCode::NetworkUnreachable,
            Self::InvalidConfig(_)
            | Self::Control(_)
            | Self::Rejected(_)
            | Self::RegistryUnavailable
            | Self::WaiterIdExhausted => OutcomeCode::Internal,
        }
    }
}

impl fmt::Display for RendezvousError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => error.fmt(formatter),
            Self::Control(error) => error.fmt(formatter),
            Self::Io { operation } => write!(formatter, "failed to {operation}"),
            Self::Deadline { wait } => write!(formatter, "{wait} deadline exceeded"),
            Self::Rejected(reason) => write!(formatter, "rendezvous join rejected: {reason}"),
            Self::Expired => formatter.write_str("rendezvous room expired"),
            Self::PeerClosed => formatter.write_str("rendezvous peer closed"),
            Self::RegistryUnavailable => formatter.write_str("rendezvous registry unavailable"),
            Self::WaiterIdExhausted => formatter.write_str("rendezvous waiter ids exhausted"),
        }
    }
}

impl std::error::Error for RendezvousError {}

impl From<ConfigError> for RendezvousError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidConfig(error)
    }
}

impl From<ControlError> for RendezvousError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}
