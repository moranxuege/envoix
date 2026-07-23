use std::collections::HashMap;

use envoix_outcomes::OutcomeCode;
use envoix_types::RecordId;

use crate::{
    AdmittedAttemptEvent, AttemptEvent, AttemptPlan, AttemptStamp, RetirementAck, RetirementIntent,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenResult {
    Opened,
    Superseded,
    AlreadyOpen,
    RetiredGeneration,
    GenerationConflict,
    PreviousAttemptLive,
    StaleGeneration,
}

#[derive(Debug, Eq, PartialEq)]
pub enum EventAdmission {
    Accepted(AdmittedAttemptEvent),
    Stale,
    Unknown,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementRequestResult {
    Requested,
    AlreadyRequested,
    Stale,
    Unknown,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitPointResult {
    Crossed,
    AlreadyCrossed,
    RetirementWon,
    Stale,
    Unknown,
    Retired,
}

#[derive(Debug, Eq, PartialEq)]
pub enum RetirementAckResult {
    Acknowledged(RetirementAck),
    NotRequested,
    NotReady,
    AlreadyAcknowledged,
    Stale,
    Unknown,
}

/// Reference state machine for generation admission and attempt quiescence.
#[derive(Debug, Default)]
pub struct AttemptSupervisor {
    slots: HashMap<RecordId, AttemptSlot>,
}

impl AttemptSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a plan without overlapping it with an unacknowledged generation.
    pub fn open(&mut self, plan: AttemptPlan) -> OpenResult {
        let card = plan.stamp.card;
        let Some(current) = self.slots.get(&card) else {
            self.slots.insert(card, AttemptSlot::new(plan));
            return OpenResult::Opened;
        };

        if plan.stamp.generation < current.plan.stamp.generation {
            return OpenResult::StaleGeneration;
        }
        if plan.stamp.generation == current.plan.stamp.generation {
            if plan != current.plan {
                return OpenResult::GenerationConflict;
            }
            return if current.quiesced {
                OpenResult::RetiredGeneration
            } else {
                OpenResult::AlreadyOpen
            };
        }
        if !current.quiesced {
            return OpenResult::PreviousAttemptLive;
        }

        self.slots.insert(card, AttemptSlot::new(plan));
        OpenResult::Superseded
    }

    /// Accepts events only for the current generation while its attempt lives.
    pub fn observe(&self, event: AttemptEvent) -> EventAdmission {
        match self.slot(event.stamp) {
            Ok(slot) if slot.quiesced => EventAdmission::Retired,
            Ok(_) => EventAdmission::Accepted(AdmittedAttemptEvent { event }),
            Err(StampMismatch::Stale) => EventAdmission::Stale,
            Err(StampMismatch::Unknown) => EventAdmission::Unknown,
        }
    }

    /// Records a retirement intent without exposing its outcome before ack.
    pub fn request_retirement(
        &mut self,
        stamp: AttemptStamp,
        intent: RetirementIntent,
    ) -> RetirementRequestResult {
        let slot = match self.slot_mut(stamp) {
            Ok(slot) => slot,
            Err(StampMismatch::Stale) => return RetirementRequestResult::Stale,
            Err(StampMismatch::Unknown) => return RetirementRequestResult::Unknown,
        };
        if slot.quiesced {
            return RetirementRequestResult::Retired;
        }

        match (&slot.retirement, intent) {
            (Some(RetirementState::Resolved(_)), _)
            | (Some(RetirementState::FinalizePending), RetirementIntent::Finalize) => {
                RetirementRequestResult::AlreadyRequested
            }
            (_, RetirementIntent::Finalize) if slot.commit_crossed => {
                slot.retirement = Some(RetirementState::Resolved(OutcomeCode::Completed));
                RetirementRequestResult::Requested
            }
            (_, RetirementIntent::Finalize) => {
                slot.retirement = Some(RetirementState::FinalizePending);
                RetirementRequestResult::Requested
            }
            (_, RetirementIntent::Pause | RetirementIntent::Cancel) if slot.commit_crossed => {
                slot.retirement = Some(RetirementState::Resolved(OutcomeCode::Completed));
                RetirementRequestResult::Requested
            }
            (_, RetirementIntent::Pause) => {
                slot.retirement = Some(RetirementState::Resolved(OutcomeCode::Paused));
                RetirementRequestResult::Requested
            }
            (_, RetirementIntent::Cancel) => {
                slot.retirement = Some(RetirementState::Resolved(OutcomeCode::Cancelled));
                RetirementRequestResult::Requested
            }
        }
    }

    /// Marks the executor's single irreversible completion point.
    pub fn cross_commit_point(&mut self, stamp: AttemptStamp) -> CommitPointResult {
        let slot = match self.slot_mut(stamp) {
            Ok(slot) => slot,
            Err(StampMismatch::Stale) => return CommitPointResult::Stale,
            Err(StampMismatch::Unknown) => return CommitPointResult::Unknown,
        };
        if slot.quiesced {
            return CommitPointResult::Retired;
        }
        if slot.commit_crossed {
            return CommitPointResult::AlreadyCrossed;
        }
        if matches!(slot.retirement, Some(RetirementState::Resolved(_))) {
            return CommitPointResult::RetirementWon;
        }

        slot.commit_crossed = true;
        if matches!(slot.retirement, Some(RetirementState::FinalizePending)) {
            slot.retirement = Some(RetirementState::Resolved(OutcomeCode::Completed));
        }
        CommitPointResult::Crossed
    }

    /// Called by the executor only after it has stopped and released resources.
    pub fn acknowledge_retirement(&mut self, stamp: AttemptStamp) -> RetirementAckResult {
        let slot = match self.slot_mut(stamp) {
            Ok(slot) => slot,
            Err(StampMismatch::Stale) => return RetirementAckResult::Stale,
            Err(StampMismatch::Unknown) => return RetirementAckResult::Unknown,
        };
        if slot.quiesced {
            return RetirementAckResult::AlreadyAcknowledged;
        }

        let outcome = match slot.retirement {
            None => return RetirementAckResult::NotRequested,
            Some(RetirementState::FinalizePending) => return RetirementAckResult::NotReady,
            Some(RetirementState::Resolved(outcome)) => outcome,
        };
        slot.quiesced = true;

        RetirementAckResult::Acknowledged(RetirementAck { stamp, outcome })
    }

    pub fn is_quiesced(&self, stamp: AttemptStamp) -> bool {
        self.slot(stamp).is_ok_and(|slot| slot.quiesced)
    }

    fn slot(&self, stamp: AttemptStamp) -> Result<&AttemptSlot, StampMismatch> {
        let Some(slot) = self.slots.get(&stamp.card) else {
            return Err(StampMismatch::Unknown);
        };
        if stamp.generation < slot.plan.stamp.generation {
            return Err(StampMismatch::Stale);
        }
        if stamp.generation > slot.plan.stamp.generation {
            return Err(StampMismatch::Unknown);
        }
        Ok(slot)
    }

    fn slot_mut(&mut self, stamp: AttemptStamp) -> Result<&mut AttemptSlot, StampMismatch> {
        let Some(slot) = self.slots.get_mut(&stamp.card) else {
            return Err(StampMismatch::Unknown);
        };
        if stamp.generation < slot.plan.stamp.generation {
            return Err(StampMismatch::Stale);
        }
        if stamp.generation > slot.plan.stamp.generation {
            return Err(StampMismatch::Unknown);
        }
        Ok(slot)
    }
}

#[derive(Debug)]
struct AttemptSlot {
    plan: AttemptPlan,
    commit_crossed: bool,
    retirement: Option<RetirementState>,
    quiesced: bool,
}

impl AttemptSlot {
    const fn new(plan: AttemptPlan) -> Self {
        Self {
            plan,
            commit_crossed: false,
            retirement: None,
            quiesced: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetirementState {
    FinalizePending,
    Resolved(OutcomeCode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StampMismatch {
    Stale,
    Unknown,
}
