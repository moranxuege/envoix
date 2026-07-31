//! Typed platform duties and provenance-based result admission.

#![forbid(unsafe_code)]

mod ledger;
mod model;
mod source_key;
mod source_report;
mod source_session;

pub use ledger::{Admission, DutyLedger, GenerationUpdate, Registration};
pub use model::{
    AdmittedDutyResult, AdmittedSourceResult, Duty, DutyKind, DutyProvenance, DutyReport,
    DutyResult,
};
pub use source_key::SourceAcquisitionKey;
pub use source_report::{
    AcquiredItem, AcquiredSelection, AcquiredSelectionError, SourceAcquisitionFailure,
    SourceReport, SourceRetention, SourceSeekability,
};
pub use source_session::{SourceReadError, SourceSession};

#[cfg(test)]
mod tests;
