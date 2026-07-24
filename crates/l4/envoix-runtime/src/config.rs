use std::num::NonZeroUsize;
use std::time::Duration;

/// Static policy for one runtime instance.
///
/// Retry/backoff *timing* is a caller/config concern and is deliberately absent
/// here; `max_commit_attempts` is only the bound handed to each card's
/// `CommittedSession` for the record-write barrier.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeConfig {
    /// Maximum number of cards with a live worker at once (admission bound).
    pub max_live_cards: NonZeroUsize,
    /// Bound on how long `shutdown` waits for one card actor / task to stop.
    pub shutdown_grace: Duration,
    /// Record-commit retry bound handed to each restored/admitted session.
    pub max_commit_attempts: NonZeroUsize,
}

impl RuntimeConfig {
    pub const fn new(
        max_live_cards: NonZeroUsize,
        shutdown_grace: Duration,
        max_commit_attempts: NonZeroUsize,
    ) -> Self {
        Self {
            max_live_cards,
            shutdown_grace,
            max_commit_attempts,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_live_cards: NonZeroUsize::new(256).expect("256 is nonzero"),
            shutdown_grace: Duration::from_secs(5),
            max_commit_attempts: NonZeroUsize::new(3).expect("3 is nonzero"),
        }
    }
}
