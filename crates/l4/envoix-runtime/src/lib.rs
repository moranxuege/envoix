//! envoix-runtime (L4): the process-lifetime owner.
//!
//! The runtime brings up and tears down the worker domain for durable transfer
//! cards. It provides idempotent bootstrap/shutdown, a leased single-writer card
//! registry, join/panic supervision, admission, lazy restore, and terminal
//! hibernation.
//!
//! # Owns no transfer truth
//! The authority for every card is L3's `TransferRecord`, persisted through P4's
//! operation store. The runtime holds only leases, task handles, admission
//! permits, and derived read projections. If the process dies, every card is
//! reconstructable from the durable store alone (via [`SessionProvider`]).
//!
//! # Ports (dependency inversion)
//! L4 depends on L3 + L1 + L0 only. The concrete iroh executor and the concrete
//! operation-store binding are injected as L1-facing ports:
//! - [`SessionProvider`] restores a card's [`CommittedSession`](envoix_product::CommittedSession)
//!   from the durable store.
//! - [`AttemptExecutor`] drives one attempt and emits raw [`ExecutorSignal`]s.
//! - [`EvidenceSink`] receives only typed, redacted evidence through a bounded,
//!   non-blocking lane running outside card actors.
//!
//! The runtime owns the C7 `AttemptSupervisor` per card and performs the real
//! linearization, so a `RetirementAck` fed to the reducer is the genuine,
//! non-forgeable token minted by the supervisor — never hand-rolled.
//!
//! Frontends observe one card at a time through [`Runtime::subscribe`]. Every
//! attach has a fresh [`SubscriptionEpoch`], starts from current truth, and uses
//! a bounded queue that coalesces replaceable projection updates while reserving
//! a lossless lane for terminal transitions and capability duties. A full
//! lossless lane surfaces [`SubscriptionLag`] instead of silently dropping.
//!
//! # Deferred
//! - The concrete iroh executor and the M4 `Phase(Confirming)` emission are wired
//!   at the host / composition root.
//! - Generated binding schemas, Rust-to-platform glue, and bulk-byte/OS-handle
//!   containment are BN1.
//! - Durable, epoch/provenance-checked mutating command intake is BN2.
//! - Confirm timers, mailbox polling, capability-duty execution/results, and
//!   storage intents remain injected-host / later-slice seams.
//! - The evidence HTTP service and storage manifest are D1.
//! - The runtime `LOG_BASELINE` filter is BN4.

#![forbid(unsafe_code)]

mod card;
mod config;
mod error;
mod evidence;
mod port;
mod runtime;
mod subscription;

pub use config::RuntimeConfig;
pub use envoix_evidence::{EvidenceSink, EvidenceSinkError};
pub use error::{AcquireError, CommandError};
pub use port::{
    AttemptExecution, AttemptExecutor, ExecutorSignal, SessionProvider, StopHandle, StopToken,
    stop_channel,
};
pub use runtime::{Runtime, ShutdownReport};
pub use subscription::{
    CardSubscription, CardUpdate, CardUpdateKind, LosslessUpdateKind, SubscribeError,
    SubscriptionEpoch, SubscriptionLag, TryRecvError,
};
