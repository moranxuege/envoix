use std::fmt;

use envoix_attempt_api::OpenResult;
use envoix_auth::AuthError;
use envoix_outcomes::OutcomeCode;
use envoix_session_iroh::SessionError;
use envoix_transfer::TransferError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptError {
    WrongDirection,
    SupervisorPoisoned,
    CannotOpen(OpenResult),
    InvalidTimeout,
    Authentication(AuthError),
    Transfer(TransferError),
    Session(SessionError),
    ProtocolEnvelope,
    RetirementHandshake,
    /// The authority would not answer for the peer's declaration, or its answer
    /// never arrived. Never treated as admission — a silent channel says nothing
    /// about whether a declaration was accepted.
    PeerContentRefused,
    /// The card would not freeze this transfer's content, so `Complete` was
    /// never sent. Refusing is safe; sending without the durable memory is not.
    ContentLockRefused,
    TaskStopped,
}

impl AttemptError {
    pub const fn outcome_code(&self) -> OutcomeCode {
        match self {
            Self::Authentication(error) => error.outcome_code(),
            Self::Transfer(error) => error.outcome_code(),
            Self::Session(error) => error.outcome_code(),
            Self::WrongDirection
            | Self::SupervisorPoisoned
            | Self::CannotOpen(_)
            | Self::InvalidTimeout
            | Self::ProtocolEnvelope
            | Self::RetirementHandshake
            | Self::PeerContentRefused
            | Self::ContentLockRefused
            | Self::TaskStopped => OutcomeCode::Internal,
        }
    }
}

impl fmt::Display for AttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDirection => {
                formatter.write_str("attempt direction does not match executor")
            }
            Self::SupervisorPoisoned => formatter.write_str("attempt supervisor unavailable"),
            Self::CannotOpen(result) => write!(formatter, "attempt cannot open: {result:?}"),
            Self::InvalidTimeout => formatter.write_str("attempt timeout must be non-zero"),
            Self::Authentication(error) => error.fmt(formatter),
            Self::Transfer(error) => error.fmt(formatter),
            Self::Session(error) => error.fmt(formatter),
            Self::ProtocolEnvelope => formatter.write_str("attempt received an invalid frame"),
            Self::RetirementHandshake => {
                formatter.write_str("attempt retirement handshake is inconsistent")
            }
            Self::PeerContentRefused => {
                formatter.write_str("no authority answered for what the peer declared")
            }
            Self::ContentLockRefused => {
                formatter.write_str("no authority froze this transfer's content")
            }
            Self::TaskStopped => formatter.write_str("attempt executor task stopped"),
        }
    }
}

impl std::error::Error for AttemptError {}
