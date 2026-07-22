//! Structured outcomes carried as data rather than inferred from error prose.

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeCode {
    Completed,
    Cancelled,
    Paused,
    PeerLost,
    Timeout,
    Unauthenticated,
    VersionMismatch,
    StorageFault,
    PublishFailed,
    SourceUnreadable,
    NetworkUnreachable,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Pairing,
    Authenticating,
    Transferring,
    Confirming,
    Publishing,
    Restoring,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Retryability {
    Retryable,
    Terminal,
    NeedsUser,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    RePickSource,
    RetryLater,
    ReconnectPeer,
}

/// Text approved for UI and diagnostic output; callers must not include secrets,
/// paths, or URIs when constructing it.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafeDisplay(String);

impl SafeDisplay {
    pub fn new(controlled_text: impl Into<String>) -> Self {
        Self(controlled_text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Outcome {
    pub code: OutcomeCode,
    pub phase: Phase,
    pub retry: Retryability,
    pub recovery: Option<Recovery>,
    pub display: SafeDisplay,
}

impl Outcome {
    pub fn new(code: OutcomeCode, phase: Phase, retry: Retryability, display: SafeDisplay) -> Self {
        Self {
            code,
            phase,
            retry,
            recovery: None,
            display,
        }
    }

    #[must_use]
    pub const fn with_recovery(mut self, recovery: Recovery) -> Self {
        self.recovery = Some(recovery);
        self
    }
}
