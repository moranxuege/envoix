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

// Re-exported so the L5 projection can destructure the deployment identity by
// name: L5 may see L4 and L0, never L1, and the manifest is L4's to publish.
pub use envoix_deployment::DeploymentIdentity;
pub use manifest::{AbiSchemaManifest, BUILD_TRUST_MANIFEST, BuildTrustManifest, ProtocolManifest};
pub use model::{
    BoundedSafeDisplay, DiagnosticsDegraded, DiagnosticsStatus, EvidenceOutcome, EvidenceProgress,
    EvidenceRecord, EvidenceValue, MAX_SAFE_DISPLAY_BYTES, RedactedId, RedactedIdKind, SessionKey,
    TimelineEntry,
};
pub use timeline::{
    EvidenceSink, EvidenceSinkError, NoopEvidenceSink, SessionTimeline, TimelineStore,
};
