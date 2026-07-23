use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};
use serde::{Deserialize, Serialize};

/// Identifies the card, attempt generation, and request that issued a duty.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct DutyProvenance {
    pub card: RecordId,
    pub generation: AttemptGen,
    pub request: RequestId,
}

/// Platform capability domains. Platform-specific request payloads belong in
/// the adapter layer, not in this shared contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DutyKind {
    SourceHandle,
    Grant,
    Staging,
    Publication,
    Courier,
    Foreground,
    Notification,
    Lock,
    OpenShare,
}

impl DutyKind {
    pub const ALL: [Self; 9] = [
        Self::SourceHandle,
        Self::Grant,
        Self::Staging,
        Self::Publication,
        Self::Courier,
        Self::Foreground,
        Self::Notification,
        Self::Lock,
        Self::OpenShare,
    ];
}

/// A durable, idempotent request for one platform capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Duty {
    pub provenance: DutyProvenance,
    pub kind: DutyKind,
}

/// An untrusted adapter response. It must pass through [`crate::DutyLedger`]
/// before product state may consume it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DutyResult {
    pub provenance: DutyProvenance,
    pub outcome: OutcomeCode,
}

/// A result whose provenance was accepted exactly once by the ledger.
#[derive(Debug, Eq, PartialEq)]
pub struct AdmittedDutyResult {
    pub(crate) duty: Duty,
    pub(crate) outcome: OutcomeCode,
}

impl AdmittedDutyResult {
    pub const fn duty(&self) -> Duty {
        self.duty
    }

    pub const fn outcome(&self) -> OutcomeCode {
        self.outcome
    }
}
