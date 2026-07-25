//! Bounded, redacted evidence projected one-way out of transfer authority.
//!
//! This crate has no dependency on the L3 product reducer or on the runtime.
//! Its write-only [`EvidenceSink`] intake accepts only typed values. Evidence
//! failures are intentionally represented by a small typed status with no
//! detail string, and no value from this crate is a reducer input.

#![forbid(unsafe_code)]

pub mod identifiers;
pub mod release;

mod manifest;
mod model;
mod timeline;

pub use manifest::{
    AbiSchemaManifest, BUILD_TRUST_MANIFEST, BuildTrustManifest, ProtocolManifest,
    TrustRootFingerprintSlot,
};
pub use model::{
    BoundedSafeDisplay, DiagnosticsDegraded, DiagnosticsStatus, EvidenceOutcome, EvidenceProgress,
    EvidenceRecord, EvidenceValue, MAX_SAFE_DISPLAY_BYTES, RedactedId, RedactedIdKind, SessionKey,
    TimelineEntry,
};
pub use timeline::{
    EvidenceSink, EvidenceSinkError, NoopEvidenceSink, SessionTimeline, TimelineStore,
};
