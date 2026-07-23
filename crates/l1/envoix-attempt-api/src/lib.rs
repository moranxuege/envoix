//! Transport-free plans, events, and retirement acknowledgement for one attempt.

#![forbid(unsafe_code)]

mod model;
mod supervisor;

pub use model::{
    AdmittedAttemptEvent, AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, ResumeIntent,
    RetirementAck, RetirementIntent,
};
pub use supervisor::{
    AttemptSupervisor, CommitPointResult, EventAdmission, OpenResult, RetirementAckResult,
    RetirementRequestResult,
};

#[cfg(test)]
mod tests;
