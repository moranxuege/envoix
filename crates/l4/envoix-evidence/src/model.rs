use std::fmt;

use envoix_attempt_api::AttemptStamp;
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{ArtifactId, ByteCount, RecordId, RequestId, TransferId};
use serde::Serialize;

/// The largest safe-display payload retained in one timeline entry — the
/// owner's published maximum, not a retention number invented beside it.
pub const MAX_SAFE_DISPLAY_BYTES: usize = SafeDisplay::MAX_BYTES;

/// The logging-ledger correlation key for one attempt generation.
pub type SessionKey = AttemptStamp;

/// A safe-display value copied from L0 and bounded for diagnostics storage.
///
/// There is deliberately no public string constructor. The only source is the
/// L0 [`envoix_outcomes::SafeDisplay`] carried by an [`Outcome`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BoundedSafeDisplay(String);

impl BoundedSafeDisplay {
    fn from_outcome(outcome: &Outcome) -> Self {
        let display = outcome.display.as_str();
        let end = display
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= MAX_SAFE_DISPLAY_BYTES)
            .last()
            .unwrap_or(0);
        let bounded = if display.len() <= MAX_SAFE_DISPLAY_BYTES {
            display
        } else {
            &display[..end]
        };
        Self(bounded.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The typed L0 outcome projection retained by evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceOutcome {
    code: OutcomeCode,
    phase: Phase,
    retry: Retryability,
    recovery: Option<Recovery>,
    display: BoundedSafeDisplay,
}

impl EvidenceOutcome {
    fn from_outcome(outcome: &Outcome) -> Self {
        Self {
            code: outcome.code,
            phase: outcome.phase,
            retry: outcome.retry,
            recovery: outcome.recovery,
            display: BoundedSafeDisplay::from_outcome(outcome),
        }
    }

    pub const fn code(&self) -> OutcomeCode {
        self.code
    }

    pub const fn phase(&self) -> Phase {
        self.phase
    }

    pub const fn retryability(&self) -> Retryability {
        self.retry
    }

    pub const fn recovery(&self) -> Option<Recovery> {
        self.recovery
    }

    pub const fn display(&self) -> &BoundedSafeDisplay {
        &self.display
    }
}

/// Typed transfer progress. Both fields are L0 byte counts, never prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceProgress {
    transferred: ByteCount,
    total: ByteCount,
}

impl EvidenceProgress {
    pub const fn new(transferred: ByteCount, total: ByteCount) -> Self {
        Self { transferred, total }
    }

    pub const fn transferred(self) -> ByteCount {
        self.transferred
    }

    pub const fn total(self) -> ByteCount {
        self.total
    }
}

/// The identifier class retained after the identifier value is discarded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactedIdKind {
    Record,
    Transfer,
    Artifact,
    Request,
}

/// A deliberately non-reversible identifier projection.
///
/// The raw value is consumed only to establish its typed class and is never
/// stored. The session key already provides the required card/generation
/// correlation, so timeline identifiers need no recoverable payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedId {
    kind: RedactedIdKind,
}

impl RedactedId {
    pub const fn record(_id: RecordId) -> Self {
        Self {
            kind: RedactedIdKind::Record,
        }
    }

    pub const fn transfer(_id: TransferId) -> Self {
        Self {
            kind: RedactedIdKind::Transfer,
        }
    }

    pub const fn artifact(_id: ArtifactId) -> Self {
        Self {
            kind: RedactedIdKind::Artifact,
        }
    }

    pub const fn request(_id: RequestId) -> Self {
        Self {
            kind: RedactedIdKind::Request,
        }
    }

    pub const fn kind(self) -> RedactedIdKind {
        self.kind
    }
}

impl fmt::Display for RedactedId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[redacted:{:?}]", self.kind)
    }
}

/// Values accepted by the evidence timeline.
///
/// There is no string/detail/error variant and no `From<String>` or
/// `From<&str>` implementation:
///
/// ```compile_fail
/// use envoix_evidence::EvidenceValue;
///
/// let raw_error = String::from("peer at 203.0.113.4 rejected secret invite");
/// let _ = EvidenceValue::from(raw_error);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum EvidenceValue {
    Phase(Phase),
    Progress(EvidenceProgress),
    Outcome(EvidenceOutcome),
    Identifier(RedactedId),
}

impl EvidenceValue {
    pub const fn phase(phase: Phase) -> Self {
        Self::Phase(phase)
    }

    pub const fn progress(progress: EvidenceProgress) -> Self {
        Self::Progress(progress)
    }

    pub fn outcome(outcome: &Outcome) -> Self {
        Self::Outcome(EvidenceOutcome::from_outcome(outcome))
    }

    pub const fn identifier(identifier: RedactedId) -> Self {
        Self::Identifier(identifier)
    }
}

/// One typed event submitted to a sink.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceRecord {
    session: SessionKey,
    value: EvidenceValue,
}

impl EvidenceRecord {
    pub const fn new(session: SessionKey, value: EvidenceValue) -> Self {
        Self { session, value }
    }

    pub const fn session(&self) -> SessionKey {
        self.session
    }

    pub const fn value(&self) -> &EvidenceValue {
        &self.value
    }

    pub(crate) fn into_parts(self) -> (SessionKey, EvidenceValue) {
        (self.session, self.value)
    }
}

/// Fixed-size metadata proving that timeline entries were dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticsDegraded {
    dropped_events: u64,
}

impl DiagnosticsDegraded {
    pub const fn dropped_events(self) -> u64 {
        self.dropped_events
    }

    pub(crate) const fn one() -> Self {
        Self { dropped_events: 1 }
    }

    pub(crate) fn increment(&mut self) {
        self.dropped_events = self.dropped_events.saturating_add(1);
    }
}

/// Whether a bounded session timeline is complete within this process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum DiagnosticsStatus {
    Complete,
    DiagnosticsDegraded(DiagnosticsDegraded),
}

/// One retained timeline value with its session-local ordering sequence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TimelineEntry {
    sequence: u64,
    value: EvidenceValue,
}

impl TimelineEntry {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn value(&self) -> &EvidenceValue {
        &self.value
    }

    pub(crate) const fn new(sequence: u64, value: EvidenceValue) -> Self {
        Self { sequence, value }
    }
}
