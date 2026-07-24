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

/// Why a frontend command was refused at intake. Every reason is
/// intake-level: the reducer's own refusals flow through committed truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRejected {
    /// The runtime has no projection for this card.
    UnknownCard,
    /// The issuing attachment is no longer the card's newest (commander)
    /// attachment: a reattach superseded it before intake.
    StaleEpoch,
    /// A newer attachment appeared after intake but before the actor
    /// linearized the command; it was dropped unapplied.
    Superseded,
    /// Lazily restoring the hibernated card was refused by the admission bound.
    AtCapacity,
    /// Shutdown has started; the runtime no longer accepts commands.
    RuntimeStopped,
    /// The actor died before answering; whether the command committed is
    /// unknown here — re-issue with the same identity to find out.
    Interrupted,
    /// The command identity is already owned by a DIFFERENT committed
    /// command. The submission was dropped; mint a fresh identity.
    Conflict,
    /// Restoring the card or reducing the command failed internally.
    Internal,
}

impl fmt::Display for CommandRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCard => formatter.write_str("the runtime has no projection for the card"),
            Self::StaleEpoch => {
                formatter.write_str("a newer attachment superseded the issuing epoch")
            }
            Self::Superseded => {
                formatter.write_str("a newer attachment appeared before the command applied")
            }
            Self::AtCapacity => formatter.write_str("the live-card admission bound is exhausted"),
            Self::RuntimeStopped => formatter.write_str("the runtime has stopped"),
            Self::Interrupted => formatter.write_str("the card actor died before answering"),
            Self::Conflict => {
                formatter.write_str("the command identity is owned by a different command")
            }
            Self::Internal => formatter.write_str("restoring or reducing the command failed"),
        }
    }
}

impl std::error::Error for CommandRejected {}
