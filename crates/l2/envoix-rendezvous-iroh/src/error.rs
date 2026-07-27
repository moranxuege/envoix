use std::fmt;

use envoix_outcomes::OutcomeCode;
use envoix_rendezvous::RendezvousError;

use crate::ConfigError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrohOperation {
    Bind,
    Accept,
    Connect,
    OpenStream,
    Close,
}

impl fmt::Display for IrohOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind => formatter.write_str("bind rendezvous endpoint"),
            Self::Accept => formatter.write_str("accept rendezvous connection"),
            Self::Connect => formatter.write_str("connect rendezvous broker"),
            Self::OpenStream => formatter.write_str("open rendezvous stream"),
            Self::Close => formatter.write_str("close rendezvous connection"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrohWait {
    Bind,
    Handshake,
    Connect,
    Stream,
    Close,
}

impl fmt::Display for IrohWait {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind => formatter.write_str("endpoint bind"),
            Self::Handshake => formatter.write_str("broker handshake"),
            Self::Connect => formatter.write_str("broker connect"),
            Self::Stream => formatter.write_str("broker stream"),
            Self::Close => formatter.write_str("broker close"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IrohRendezvousError {
    InvalidConfig(ConfigError),
    Deadline {
        wait: IrohWait,
    },
    /// The server was reachable and turned the caller away: its connection
    /// budget is full. Distinct from `Deadline` and `Transport` on purpose —
    /// the caller should come back, not conclude the network is broken.
    Refused,
    Transport {
        operation: IrohOperation,
    },
    Core(RendezvousError),
    ConnectionTaskFailed,
}

impl IrohRendezvousError {
    pub const fn outcome_code(&self) -> OutcomeCode {
        match self {
            Self::InvalidConfig(_) => OutcomeCode::Internal,
            Self::Deadline { .. } => OutcomeCode::Timeout,
            // The nearest truthful code in the shared outcome vocabulary: the
            // rendezvous is not available to this caller right now. That
            // vocabulary has no "busy", and adding one rewrites a generated
            // binding contract, so the refusal stays typed at this layer.
            Self::Refused => OutcomeCode::NetworkUnreachable,
            Self::Transport {
                operation: IrohOperation::Bind | IrohOperation::Connect,
            } => OutcomeCode::NetworkUnreachable,
            Self::Transport { .. } => OutcomeCode::PeerLost,
            Self::Core(error) => error.outcome_code(),
            Self::ConnectionTaskFailed => OutcomeCode::Internal,
        }
    }
}

impl fmt::Display for IrohRendezvousError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => error.fmt(formatter),
            Self::Deadline { wait } => write!(formatter, "{wait} deadline exceeded"),
            Self::Refused => {
                formatter.write_str("rendezvous refused the connection: its budget is full")
            }
            Self::Transport { operation } => write!(formatter, "failed to {operation}"),
            Self::Core(error) => error.fmt(formatter),
            Self::ConnectionTaskFailed => formatter.write_str("rendezvous connection task failed"),
        }
    }
}

impl std::error::Error for IrohRendezvousError {}

impl From<ConfigError> for IrohRendezvousError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidConfig(error)
    }
}

impl From<RendezvousError> for IrohRendezvousError {
    fn from(error: RendezvousError) -> Self {
        Self::Core(error)
    }
}
