//! The identity of ONE source acquisition.
//!
//! A picked document must belong to exactly one card, and the Tier 0
//! adversarial review found it belonged to nobody: the Android adapter held the
//! URI in a process-global slot with no identity at all, so a document chosen
//! for card B could be consumed by card A's queued duty. That is an ownership
//! error, not a retry problem.
//!
//! The identity it needed already existed and was already card-scoped — the
//! source duty's own [`DutyProvenance`]. This type is that provenance promoted
//! to a name, so that "which acquisition is this?" has one answer with one
//! type, and a call site cannot key on the card alone by accident. Every part
//! of it is minted by the authority inside card creation; the frontend's create
//! request id is deliberately NOT it.
//!
//! The generation is what makes a re-pick distinct. `on_repick_source` advances
//! it before asking again, so a late answer to a superseded request names a key
//! that is no longer current and can be refused on sight rather than binding a
//! document to an attempt that has moved on.

use envoix_types::{AttemptGen, RecordId, RequestId};
use serde::{Deserialize, Serialize};

use crate::DutyProvenance;

/// Which source acquisition a picked document, a claim, or a report belongs to.
///
/// Equality is over all three fields, and that is the point: a value that
/// matches the card but not the generation, or the generation but not the
/// request, is a DIFFERENT acquisition and must not be honoured.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SourceAcquisitionKey {
    card: RecordId,
    generation: AttemptGen,
    request: RequestId,
}

impl SourceAcquisitionKey {
    /// The key for the source duty the authority just minted.
    ///
    /// Taking the whole provenance rather than three loose arguments is what
    /// stops a caller assembling a key from parts that never travelled
    /// together.
    pub const fn of(provenance: DutyProvenance) -> Self {
        Self {
            card: provenance.card,
            generation: provenance.generation,
            request: provenance.request,
        }
    }

    pub const fn card(&self) -> RecordId {
        self.card
    }

    pub const fn generation(&self) -> AttemptGen {
        self.generation
    }

    pub const fn request(&self) -> RequestId {
        self.request
    }

    /// The provenance this key names, for the duty wire.
    pub const fn provenance(&self) -> DutyProvenance {
        DutyProvenance {
            card: self.card,
            generation: self.generation,
            request: self.request,
        }
    }

    /// Whether `other` is this same acquisition.
    ///
    /// Spelled out rather than left to `==` at call sites so the intent is
    /// legible where it matters: an adapter answering a claim is deciding
    /// whether a document belongs to the asker, and "the card matches" is not
    /// that question.
    pub fn is(&self, other: &Self) -> bool {
        self == other
    }
}
