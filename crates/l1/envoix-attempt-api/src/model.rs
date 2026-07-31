use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, Direction, RecordId, TransferId};
use serde::{Deserialize, Serialize};

/// Identifies one execution generation of a durable transfer card.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AttemptStamp {
    pub card: RecordId,
    pub generation: AttemptGen,
}

/// Whether an attempt may resume — never from WHERE.
///
/// The offset used to travel here, taken from the local card's last observed
/// progress, and the executor refused a peer whose reported prefix disagreed
/// with it. That was wrong in both directions. A card's progress is its own
/// memory of a previous run; the peer's storage is the authority on what
/// survives there now, and it can legitimately hold LESS (a failed digest, a
/// discarded tail) or MORE (it checkpointed bytes whose progress event never
/// reached this card, or it already holds the whole file and says so).
///
/// The product reducer had already written that conclusion down — progress is
/// kept monotone precisely so a "valid larger durable peer prefix" is not made
/// to look like a violation — while the executor was enforcing the opposite.
///
/// So the offset is gone rather than merely unenforced: it had exactly one
/// reader, and that reader was making a decision no local value can support.
/// What the peer actually resumed from comes back as `ResumeEstablished`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeIntent {
    /// Start over. A peer reporting anything but a zero prefix IS a violation:
    /// the sender disabled resume on the wire, so a nonzero answer is a peer
    /// ignoring what it was told.
    Fresh,
    /// Resume from whatever the receiver actually holds.
    Allowed,
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
    /// How much of this file the peers agreed NOT to transfer again.
    ///
    /// The only event that may move a card's progress DOWN. Everything else is
    /// monotone, because an untrusted executor must not be able to make a
    /// progress bar run backwards — but the resumed prefix is settled by
    /// negotiation with the peer, and the card's prior guess can be wrong in
    /// either direction. Without this, a card projects the offset it HOPED to
    /// resume from and never learns what actually happened.
    ///
    /// Emitted once per attempt, after the negotiation and before any progress.
    ResumeEstablished {
        offset: ByteCount,
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
