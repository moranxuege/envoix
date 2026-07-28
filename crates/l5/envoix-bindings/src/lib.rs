//! envoix-bindings (L5): the generated read, command and capability contracts.
//!
//! Three TOML schemas are the single sources of truth: `schema/read.schema` for
//! everything a frontend may observe, `schema/command.schema` for the mutating
//! command conversation (submit → acceptance → completion, BN2's frozen
//! semantics), and `schema/capability.schema` for what a frontend asks its own
//! platform adapter to do before any card exists. One deterministic generator
//! emits all three into per-schema Rust/Dart/Kotlin/Swift artifacts; drift
//! tests regenerate all twelve and fail on any byte difference.
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

/// Generated capability-contract types and codec; see
/// `generated/rust/capability.rs`.
///
/// Its two peers are a frontend and its platform adapter (Dart and Kotlin on
/// Android, Rust on the local CLI, SwiftUI and AVFoundation on Apple); a
/// capability frame never reaches the host, which decodes [`command`] alone.
pub mod capability {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/rust/capability.rs"
    ));
}

/// Generated duty-contract types and codec; see `generated/rust/duty.rs`.
///
/// Its two peers are the Rust authority and a platform duty EXECUTOR (the
/// Kotlin service on Android, Swift on Apple). Unlike [`capability`], this
/// exchange is card-scoped and its answer is admitted exactly once by the C6
/// duty ledger — a decoded report is a well-formed claim, never an admitted
/// result.
pub mod duty {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/rust/duty.rs"
    ));
}

pub use model::{
    Decl, DeclKind, Direction, EnumDecl, FieldDecl, FieldTy, FrontendBody, RuleValue, SchemaDoc,
    StructDecl, UnionDecl, UnionVariant,
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

/// The capability-schema source this build was generated from.
pub fn capability_schema_text() -> &'static str {
    include_str!("../schema/capability.schema")
}

/// The duty-schema source this build was generated from.
pub fn duty_schema_text() -> &'static str {
    include_str!("../schema/duty.schema")
}
