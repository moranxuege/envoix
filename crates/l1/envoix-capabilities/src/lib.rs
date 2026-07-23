//! Typed platform duties and provenance-based result admission.

#![forbid(unsafe_code)]

mod ledger;
mod model;

pub use ledger::{Admission, DutyLedger, GenerationUpdate, Registration};
pub use model::{AdmittedDutyResult, Duty, DutyKind, DutyProvenance, DutyResult};

#[cfg(test)]
mod tests;
