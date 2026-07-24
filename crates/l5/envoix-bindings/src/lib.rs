//! envoix-bindings (L5): the generated read contract.
//!
//! One TOML schema (`schema/read.schema`) is the single source of truth for
//! everything a frontend may observe: card snapshots/updates, subscription
//! epoch rules, capability duties, the evidence timeline, typed outcomes, and
//! build/trust metadata. Deterministic emitters generate the read-side glue
//! for Rust (`generated/rust/read.rs`, the reference codec included below),
//! Dart, Kotlin, and Swift; a drift test regenerates all four and fails on any
//! byte difference.
//!
//! # Containment
//! The schema's scalar vocabulary has no bytes/blob type and no
//! handle/path/URI type, so bulk payload bytes and OS handles cannot cross the
//! binding. Every numeric field is range-checked, every string/hex/list field
//! is bounded, unknown enum variants / union kinds / fields / schema versions
//! are typed decode failures, and no decode path panics on hostile input.
//!
//! # Layering
//! L5 depends on L0 + L4 only. The L3/L1 types crossing L4's public API reach
//! this crate through the `envoix-runtime` façade re-exports; the projection
//! in [`project`] turns live L4 values into generated view types.

#![forbid(unsafe_code)]

pub mod emit;
mod model;
mod parse;
mod project;

/// Generated read-contract types and codec; see `generated/rust/read.rs`.
pub mod read {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/rust/read.rs"
    ));
}

pub use model::{
    Decl, DeclKind, EnumDecl, FieldDecl, FieldTy, SchemaDoc, StructDecl, UnionDecl, UnionVariant,
};
pub use parse::{SchemaParseError, parse_schema};
pub use project::{
    build_manifest_frame, card_update_frame, closed_frame, evidence_frame, lag_frame,
    subscribe_rejected_frame,
};

/// The schema source this build was generated from.
pub fn read_schema_text() -> &'static str {
    include_str!("../schema/read.schema")
}
