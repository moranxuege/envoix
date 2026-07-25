//! Android capability adapter (L6): typed platform duties + host identifiers.
//!
//! Frontends never see this crate; they observe duties through the generated
//! read contract. The Rust host (`hosts/envoix-host-android`) dispatches
//! [`WorkOrder`]s to the Kotlin service over an in-process lane and feeds
//! [`WorkReport`]s back through the C6 ledger. Platform-specific payloads live
//! here — never in the shared contracts.

#![forbid(unsafe_code)]

pub mod identifiers;

mod adapter;
mod duty;

pub use adapter::{DutyAdapter, IssueDecision};
pub use duty::{
    EXECUTED_KINDS, LaneError, MAX_DISPLAY_NAME_BYTES, MAX_LANE_FRAME_BYTES, MAX_STAGED_PATH_BYTES,
    Notice, WireHex16, WireHex32, WireProvenance, Work, WorkOrder, WorkReport, platform_work,
};

#[cfg(test)]
mod tests;
