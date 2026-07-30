//! Typed platform duties and provenance-based result admission.

#![forbid(unsafe_code)]

mod ledger;
mod model;
mod source_key;
mod source_report;

pub use ledger::{Admission, DutyLedger, GenerationUpdate, Registration};
pub use model::{
    AdmittedDutyResult, AdmittedSourceResult, Duty, DutyKind, DutyProvenance, DutyReport,
    DutyResult,
};
pub use source_key::SourceAcquisitionKey;
pub use source_report::{
    SourceAcquisitionFailure, SourceReport, SourceRetention, SourceSeekability,
};

#[cfg(test)]
mod tests;
