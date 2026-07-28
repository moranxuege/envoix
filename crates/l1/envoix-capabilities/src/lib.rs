//! Typed platform duties and provenance-based result admission.

#![forbid(unsafe_code)]

mod ledger;
mod model;
mod source_key;

pub use ledger::{Admission, DutyLedger, GenerationUpdate, Registration};
pub use model::{AdmittedDutyResult, Duty, DutyKind, DutyProvenance, DutyResult};
pub use source_key::SourceAcquisitionKey;

#[cfg(test)]
mod tests;
