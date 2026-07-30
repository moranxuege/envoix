use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};
use serde::{Deserialize, Serialize};

use crate::{SourceAcquisitionKey, SourceReport};

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

/// What a duty answered, in the vocabulary its own kind can speak.
///
/// Source acquisition is the one duty whose answer is not an outcome code: it
/// must state retention and seekability, which no code can carry. Every other
/// kind answers an outcome and nothing more. The ledger enforces the pairing
/// ([`crate::Admission::Incompatible`]), so a matched kind and report is a fact
/// downstream code can rely on rather than re-check.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DutyReport {
    Outcome(OutcomeCode),
    Source(SourceReport),
}

impl DutyReport {
    /// Whether a duty of `kind` is allowed to answer this.
    ///
    /// Both directions are refusals, not just one: a source duty answering a
    /// bare outcome is the missing-facts defect, and a notification answering
    /// `source_acquired` is an adapter reporting on work it never did.
    pub const fn answers(&self, kind: DutyKind) -> bool {
        match (self, kind) {
            (Self::Source(_), DutyKind::SourceHandle) => true,
            (Self::Outcome(_), DutyKind::SourceHandle) | (Self::Source(_), _) => false,
            (Self::Outcome(_), _) => true,
        }
    }
}

/// An untrusted adapter response. It must pass through [`crate::DutyLedger`]
/// before product state may consume it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DutyResult {
    pub provenance: DutyProvenance,
    pub report: DutyReport,
}

/// A result whose provenance was accepted exactly once by the ledger.
#[derive(Debug, Eq, PartialEq)]
pub struct AdmittedDutyResult {
    pub(crate) duty: Duty,
    pub(crate) report: DutyReport,
}

impl AdmittedDutyResult {
    pub const fn duty(&self) -> Duty {
        self.duty
    }

    pub const fn report(&self) -> DutyReport {
        self.report
    }

    /// The outcome, for the kinds that answer one. `None` for a source duty,
    /// which never does.
    pub const fn outcome(&self) -> Option<OutcomeCode> {
        match self.report {
            DutyReport::Outcome(outcome) => Some(outcome),
            DutyReport::Source(_) => None,
        }
    }

    /// This result as a source acquisition answer, if that is what it is.
    ///
    /// The only way to obtain an [`AdmittedSourceResult`], which is why that
    /// type can be trusted to mean "the ledger admitted exactly this, once, for
    /// a duty that was outstanding". A caller cannot assemble one from a
    /// provenance and a report it liked the look of.
    pub const fn into_source(self) -> Option<AdmittedSourceResult> {
        match self.report {
            DutyReport::Source(report) => Some(AdmittedSourceResult {
                duty: self.duty,
                report,
            }),
            DutyReport::Outcome(_) => None,
        }
    }
}

/// An admitted answer to a source acquisition, and the duty it discharges.
///
/// No public constructor: [`AdmittedDutyResult::into_source`] is the only
/// source of one. The product's source transitions take this type rather than a
/// key plus a report, so an unadmitted platform claim cannot reach the reducer
/// even by naming the right acquisition. THAT is what makes this type
/// trustworthy, and it survives cloning untouched.
///
/// `Clone`, deliberately. It once was not, on the reasoning that an
/// admitted-once answer should be a move-only token — but "exactly once" is the
/// LEDGER's guarantee (a repeat provenance admits as `Duplicate`) and the
/// reducer's (a settled answer is accepted only from `Acquiring` under the exact
/// key). Move-only added nothing to either and made delivery lossy: the runtime
/// had to hand the single value to an actor that took it before applying it, so
/// an actor that died mid-apply destroyed the only copy, and the caller could
/// not tell that from success. Delivery is at-least-once over an idempotent
/// reducer, which is a shape that can retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSourceResult {
    duty: Duty,
    report: SourceReport,
}

impl AdmittedSourceResult {
    /// The acquisition this answers. Derived from the discharged duty's own
    /// provenance, so it cannot name an acquisition the duty did not belong to.
    pub const fn acquisition(&self) -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(self.duty.provenance)
    }

    pub const fn report(&self) -> SourceReport {
        self.report
    }
}
