//! envoix-bindings (L5): the generated read and command contracts.
//!
//! Two TOML schemas are the single sources of truth: `schema/read.schema` for
//! everything a frontend may observe, and `schema/command.schema` for the
//! mutating command conversation (submit → acceptance → completion, BN2's
//! frozen semantics). One deterministic generator emits both into per-schema
//! Rust/Dart/Kotlin/Swift artifacts; drift tests regenerate all eight and fail
//! on any byte difference.
//!
//! # Containment
//! The shared scalar vocabulary has no bytes/blob type and no handle/path/URI
//! type, so bulk payload bytes and OS handles cannot cross the binding in
//! either direction. Every numeric field is range-checked, every
//! string/hex/list field is bounded, unknown enum variants / union kinds /
//! fields / schema versions are typed decode failures, and no decode path
//! panics on hostile input — including the command direction, where hostile
//! bytes arrive AT the Rust boundary.
//!
//! # Layering
//! L5 depends on L0 + L4 only. The L3/L1 types crossing L4's public API reach
//! this crate through the `envoix-runtime` façade re-exports; [`project`]
//! turns live L4 values into read views, and [`bridge`] converts between the
//! command contract and L4's live command vocabulary.

#![forbid(unsafe_code)]

pub mod bridge;
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

/// Generated command-contract types and codec; see `generated/rust/command.rs`.
pub mod command {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/rust/command.rs"
    ));
}

pub use model::{
    Decl, DeclKind, EnumDecl, FieldDecl, FieldTy, RuleValue, SchemaDoc, StructDecl, UnionDecl,
    UnionVariant,
};
pub use parse::{SchemaParseError, parse_schema};
pub use project::{
    build_manifest_frame, card_update_frame, closed_frame, evidence_frame, lag_frame,
    subscribe_rejected_frame,
};

/// The read-schema source this build was generated from.
pub fn read_schema_text() -> &'static str {
    include_str!("../schema/read.schema")
}

/// The command-schema source this build was generated from.
pub fn command_schema_text() -> &'static str {
    include_str!("../schema/command.schema")
}
