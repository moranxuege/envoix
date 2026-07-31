use std::collections::{HashMap, HashSet};

use envoix_types::{AttemptGen, RecordId};

use crate::{AdmittedDutyResult, Duty, DutyProvenance, DutyResult};

/// Result of registering a duty against the ledger's current generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Registration {
    Registered,
    NoCurrentGeneration,
    StaleGeneration,
    FutureGeneration,
    AlreadyOutstanding,
    AlreadyDischarged,
}

/// Result of changing the authoritative current generation for a card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationUpdate {
    Initialized,
    Advanced,
    Unchanged,
    RejectedRegression,
}

/// Classification applied before an adapter result can reach product state.
#[derive(Debug, Eq, PartialEq)]
pub enum Admission {
    Fresh(AdmittedDutyResult),
    Stale,
    Duplicate,
    Unknown,
    /// The result named an outstanding duty and answered in the wrong
    /// vocabulary for its kind — a source duty reporting a bare outcome, or a
    /// non-source duty reporting an acquisition.
    ///
    /// Distinct from `Unknown` on purpose. `Unknown` says the ledger has no
    /// such duty; this says it has one and the adapter answered a different
    /// question. Collapsing them would hide a real adapter defect inside the
    /// routing noise that is expected and ignorable.
    Incompatible,
}

/// Reference implementation of duty registration and result admission.
#[derive(Debug, Default)]
pub struct DutyLedger {
    current_generations: HashMap<RecordId, AttemptGen>,
    outstanding: HashMap<DutyProvenance, Duty>,
    /// Admitted, and not yet known to have been ACTED ON.
    ///
    /// The middle state a one-phase ledger did not have. Admission used to
    /// discharge immediately, so a result the card never committed was recorded
    /// as done: the platform was told its work had landed, the card sat waiting
    /// for an answer that would now be refused as a duplicate, and only a
    /// restart could clear it — because the ledger is process memory rebuilt at
    /// boot, which is the only reason that was survivable at all.
    in_flight: HashMap<DutyProvenance, Duty>,
    discharged: HashSet<DutyProvenance>,
}

impl DutyLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_generation(&self, card: RecordId) -> Option<AttemptGen> {
        self.current_generations.get(&card).copied()
    }

    /// Establishes or monotonically advances the card's current generation.
    ///
    /// The caller owns generation authority; registering a duty never changes
    /// it implicitly.
    pub fn advance_generation(
        &mut self,
        card: RecordId,
        generation: AttemptGen,
    ) -> GenerationUpdate {
        match self.current_generations.get(&card).copied() {
            None => {
                self.current_generations.insert(card, generation);
                GenerationUpdate::Initialized
            }
            Some(current) if generation < current => GenerationUpdate::RejectedRegression,
            Some(current) if generation == current => GenerationUpdate::Unchanged,
            Some(_) => {
                self.current_generations.insert(card, generation);
                self.outstanding.retain(|provenance, _| {
                    provenance.card != card || provenance.generation >= generation
                });
                self.in_flight.retain(|provenance, _| {
                    provenance.card != card || provenance.generation >= generation
                });
                self.discharged.retain(|provenance| {
                    provenance.card != card || provenance.generation >= generation
                });
                GenerationUpdate::Advanced
            }
        }
    }

    /// Records a duty only when it belongs to the card's current generation.
    pub fn register(&mut self, duty: Duty) -> Registration {
        let provenance = duty.provenance;
        let Some(current) = self.current_generations.get(&provenance.card).copied() else {
            return Registration::NoCurrentGeneration;
        };

        if provenance.generation < current {
            return Registration::StaleGeneration;
        }
        if provenance.generation > current {
            return Registration::FutureGeneration;
        }
        if self.discharged.contains(&provenance) {
            return Registration::AlreadyDischarged;
        }
        // An answer in flight is still this duty's answer. Re-registering would
        // dispatch the work a second time while the first result is being
        // applied.
        if self.outstanding.contains_key(&provenance) || self.in_flight.contains_key(&provenance) {
            return Registration::AlreadyOutstanding;
        }

        self.outstanding.insert(provenance, duty);
        Registration::Registered
    }

    /// Admits an exact, current, outstanding result once and discharges it.
    pub fn admit(&mut self, result: DutyResult) -> Admission {
        let provenance = result.provenance;
        let Some(current) = self.current_generations.get(&provenance.card).copied() else {
            return Admission::Unknown;
        };

        if provenance.generation < current {
            return Admission::Stale;
        }
        if self.discharged.contains(&provenance) || self.in_flight.contains_key(&provenance) {
            return Admission::Duplicate;
        }

        let Some(duty) = self.outstanding.get(&provenance).copied() else {
            return Admission::Unknown;
        };
        // Checked BEFORE the duty is discharged: an adapter that answered the
        // wrong question has not done the work, and consuming the duty would
        // lose it silently. It stays outstanding so a correct report can still
        // arrive, or a retirement can clear it.
        if !result.report.answers(duty.kind) {
            return Admission::Incompatible;
        }
        // The FIRST phase. The duty leaves `outstanding` so nothing dispatches
        // it again, and does not reach `discharged` until something says the
        // result was acted on — see `finalize` and `abandon`.
        self.outstanding.remove(&provenance);
        self.in_flight.insert(provenance, duty);

        Admission::Fresh(AdmittedDutyResult {
            duty,
            report: result.report,
        })
    }

    /// The second phase: this result reached durable product state.
    ///
    /// Only now is the duty done. A caller that never calls this has not lied to
    /// anyone — the duty simply stays in flight until it is abandoned.
    pub fn finalize(&mut self, provenance: DutyProvenance) {
        if self.in_flight.remove(&provenance).is_some() {
            self.discharged.insert(provenance);
        }
    }

    /// The other second phase: the result did NOT reach product state.
    ///
    /// The duty goes back to outstanding, so the same answer can be admitted
    /// again rather than being refused as a duplicate of something nothing
    /// acted on. This is what makes a lost delivery recoverable within the
    /// process instead of only across a restart.
    pub fn abandon(&mut self, provenance: DutyProvenance) {
        if let Some(duty) = self.in_flight.remove(&provenance) {
            self.outstanding.insert(provenance, duty);
        }
    }

    pub fn outstanding_len(&self) -> usize {
        self.outstanding.len()
    }
}
