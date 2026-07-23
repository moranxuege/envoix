use std::fmt;
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyOperation {
    Read,
    CreateParent,
    Create,
    Write,
    Sync,
}

impl fmt::Display for KeyOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => formatter.write_str("read node key"),
            Self::CreateParent => formatter.write_str("create node-key directory"),
            Self::Create => formatter.write_str("create node key"),
            Self::Write => formatter.write_str("write node key"),
            Self::Sync => formatter.write_str("sync node key"),
        }
    }
}

#[derive(Debug)]
pub enum KeyError {
    Io {
        operation: KeyOperation,
        source: io::Error,
    },
    InvalidLength {
        actual: usize,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "failed to {operation}: {source}"),
            Self::InvalidLength { actual } => {
                write!(
                    formatter,
                    "node key must be exactly 32 bytes, found {actual}"
                )
            }
        }
    }
}

impl std::error::Error for KeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidLength { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ServerError {
    Key(KeyError),
    RendezvousConfig(envoix_rendezvous::ConfigError),
    IrohConfig(envoix_rendezvous_iroh::ConfigError),
    Iroh(envoix_rendezvous_iroh::IrohRendezvousError),
    Signal(io::Error),
    ServerTaskFailed,
    ShutdownDeadline,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(error) => error.fmt(formatter),
            Self::RendezvousConfig(error) => error.fmt(formatter),
            Self::IrohConfig(error) => error.fmt(formatter),
            Self::Iroh(error) => error.fmt(formatter),
            Self::Signal(error) => write!(formatter, "failed to wait for shutdown signal: {error}"),
            Self::ServerTaskFailed => formatter.write_str("rendezvous server task failed"),
            Self::ShutdownDeadline => formatter.write_str("rendezvous shutdown deadline exceeded"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<KeyError> for ServerError {
    fn from(error: KeyError) -> Self {
        Self::Key(error)
    }
}

impl From<envoix_rendezvous_iroh::IrohRendezvousError> for ServerError {
    fn from(error: envoix_rendezvous_iroh::IrohRendezvousError) -> Self {
        Self::Iroh(error)
    }
}
