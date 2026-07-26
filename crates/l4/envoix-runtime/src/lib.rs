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
//! # Command intake (BN2)
//! Frontend mutating commands enter only through [`Runtime::submit_command`]:
//! provenance is the live [`CardSubscription`] itself (the card's newest
//! attachment is its commander), acceptance is answered separately from the
//! committed completion (a [`CommandTicket`]), and every command carries a
//! caller-minted `CommandId` deduplicated through the durable ledger riding
//! inside the L3 record — exactly-once across a process death. Internal
//! inputs (executor signals, retirement acks, restore) never pass this gate.
//!
//! # Deferred
//! - The concrete iroh executor and the M4 `Phase(Confirming)` emission are wired
//!   at the host / composition root.
//! - Generated command binding schemas are BN3.
//! - Confirm timers, mailbox polling, capability-duty execution/results, and
//!   storage intents remain injected-host / later-slice seams.
//! - The evidence HTTP service and storage manifest are D1.
//! - The runtime `LOG_BASELINE` filter is BN4.

#![forbid(unsafe_code)]

mod card;
mod command;
mod config;
mod error;
mod evidence;
mod port;
mod runtime;
mod subscription;

pub use command::{CommandCompletion, CommandTicket, CommandVerdict};
pub use config::RuntimeConfig;
// Façade completeness: the L3/L1 types that cross this crate's public API
// (through `CardUpdateKind` and `TransferRecord`) are re-exported so L5 can
// consume them without a direct lower-layer dependency.
pub use envoix_attempt_api::RetirementIntent;
pub use envoix_capabilities::{Duty, DutyKind, DutyProvenance};
pub use envoix_evidence::{EvidenceSink, EvidenceSinkError};
pub use envoix_product::{
    CapabilityAction, CommandLedger, MAX_INVITE_INPUT_LENGTH, MAX_INVITE_LINK_LENGTH,
    MAX_ROOM_CODE_LENGTH, PairingChannel, PauseOrigin, ProductCommand, ProductIdentity,
    ProductState, Quiescence, TransferRecord, WorkerKind,
};
pub use error::{AcquireError, CommandRejected};
pub use port::{
    AttemptExecution, AttemptExecutor, ExecutorSignal, SessionProvider, StopHandle, StopSignal,
    StopToken, stop_channel,
};
pub use runtime::{Runtime, ShutdownReport};
pub use subscription::{
    CardSubscription, CardUpdate, CardUpdateKind, LosslessUpdateKind, SubscribeError,
    SubscriptionEpoch, SubscriptionLag, TryRecvError,
};
