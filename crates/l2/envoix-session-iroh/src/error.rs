use std::fmt;

use envoix_outcomes::OutcomeCode;

use crate::config::{ConfigError, WaitKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionOperation {
    Bind,
    Connect,
    Accept,
    OpenStream,
    ReadFrame,
    WriteFrame,
    ExportBinding,
    CloseStream,
    CloseEndpoint,
}

impl fmt::Display for SessionOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind => formatter.write_str("bind endpoint"),
            Self::Connect => formatter.write_str("connect peer"),
            Self::Accept => formatter.write_str("accept peer"),
            Self::OpenStream => formatter.write_str("open data stream"),
            Self::ReadFrame => formatter.write_str("read frame"),
            Self::WriteFrame => formatter.write_str("write frame"),
            Self::ExportBinding => formatter.write_str("export channel binding"),
            Self::CloseStream => formatter.write_str("close data stream"),
            Self::CloseEndpoint => formatter.write_str("close endpoint"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    InvalidConfig(ConfigError),
    Cancelled,
    DeadlineExceeded { wait: WaitKind },
    PeerClosed,
    VersionMismatch,
    MalformedEnvelope,
    OperationFailed { operation: SessionOperation },
}

impl SessionError {
    pub const fn outcome_code(&self) -> OutcomeCode {
        match self {
            Self::Cancelled => OutcomeCode::Cancelled,
            Self::DeadlineExceeded { .. } => OutcomeCode::Timeout,
            Self::PeerClosed => OutcomeCode::PeerLost,
            Self::VersionMismatch => OutcomeCode::VersionMismatch,
            Self::OperationFailed {
                operation: SessionOperation::Bind | SessionOperation::Connect,
            } => OutcomeCode::NetworkUnreachable,
            Self::InvalidConfig(_) | Self::MalformedEnvelope | Self::OperationFailed { .. } => {
                OutcomeCode::Internal
            }
        }
    }

    pub(crate) const fn operation(operation: SessionOperation) -> Self {
        Self::OperationFailed { operation }
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => error.fmt(formatter),
            Self::Cancelled => formatter.write_str("session cancelled"),
            Self::DeadlineExceeded { wait } => write!(formatter, "{wait} deadline exceeded"),
            Self::PeerClosed => formatter.write_str("session peer closed"),
            Self::VersionMismatch => formatter.write_str("session wire version is unsupported"),
            Self::MalformedEnvelope => formatter.write_str("session frame envelope is invalid"),
            Self::OperationFailed { operation } => write!(formatter, "failed to {operation}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<ConfigError> for SessionError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidConfig(error)
    }
}
