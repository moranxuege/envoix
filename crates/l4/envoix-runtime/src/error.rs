use std::fmt;

/// Why a card could not be brought live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireError {
    /// The card already has a live single-writer owner in this runtime.
    AlreadyLive,
    /// The live-card admission bound is exhausted.
    AtCapacity,
    /// The runtime is shutting down and no longer admits cards.
    NotAdmitting,
    /// The card is not present in the durable store (restore only).
    Absent,
    /// Reconstructing the durable card failed (e.g. identity exhaustion).
    Internal,
}

impl fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyLive => formatter.write_str("the card already has a live owner"),
            Self::AtCapacity => formatter.write_str("the live-card admission bound is exhausted"),
            Self::NotAdmitting => formatter.write_str("the runtime is not admitting cards"),
            Self::Absent => formatter.write_str("the card is absent from the durable store"),
            Self::Internal => formatter.write_str("restoring the durable card failed"),
        }
    }
}

impl std::error::Error for AcquireError {}

/// Why a command could not be delivered to a card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError {
    /// No live actor owns the card (it is hibernated or was never admitted).
    NotLive,
    /// The reducer rejected the input (e.g. identity exhaustion).
    Internal,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLive => formatter.write_str("the card has no live owner"),
            Self::Internal => formatter.write_str("the card rejected the command"),
        }
    }
}

impl std::error::Error for CommandError {}
