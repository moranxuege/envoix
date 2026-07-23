use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, Direction, RecordId, TransferId};
use serde::{Deserialize, Serialize};

/// Identifies one execution generation of a durable transfer card.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AttemptStamp {
    pub card: RecordId,
    pub generation: AttemptGen,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeIntent {
    Fresh,
    ResumeFrom { offset: ByteCount },
}

/// Product-resolved input for one transport-independent attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttemptPlan {
    pub stamp: AttemptStamp,
    pub direction: Direction,
    pub transfer: TransferId,
    pub artifact: ArtifactId,
    pub resume: ResumeIntent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptEventKind {
    Phase(Phase),
    Progress {
        transferred: ByteCount,
    },
    /// A terminal observation is not proof that the attempt is quiescent.
    Terminal(OutcomeCode),
}

/// Untrusted executor output, stamped for generation admission.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttemptEvent {
    pub stamp: AttemptStamp,
    pub kind: AttemptEventKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementIntent {
    Pause,
    Cancel,
    Finalize,
}

/// An event admitted for the currently live generation.
#[derive(Debug, Eq, PartialEq)]
pub struct AdmittedAttemptEvent {
    pub(crate) event: AttemptEvent,
}

impl AdmittedAttemptEvent {
    pub const fn event(&self) -> AttemptEvent {
        self.event
    }
}

/// Proof that the executor stopped, released its lease and handles, and
/// acknowledged the linearized outcome.
///
/// The token has no public constructor and is deliberately non-cloneable.
///
/// ```compile_fail
/// use envoix_attempt_api::RetirementAck;
///
/// let _forged = RetirementAck {};
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct RetirementAck {
    pub(crate) stamp: AttemptStamp,
    pub(crate) outcome: OutcomeCode,
}

impl RetirementAck {
    pub const fn stamp(&self) -> AttemptStamp {
        self.stamp
    }

    pub const fn outcome(&self) -> OutcomeCode {
        self.outcome
    }
}
