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
//! permits, and a derived read snapshot. If the process dies, every card is
//! reconstructable from the durable store alone (via [`SessionProvider`]).
//!
//! # Ports (dependency inversion)
//! L4 depends on L3 + L1 + L0 only. The concrete iroh executor and the concrete
//! operation-store binding are injected as L1-facing ports:
//! - [`SessionProvider`] restores a card's [`CommittedSession`](envoix_product::CommittedSession)
//!   from the durable store.
//! - [`AttemptExecutor`] drives one attempt and emits raw [`ExecutorSignal`]s.
//!
//! The runtime owns the C7 `AttemptSupervisor` per card and performs the real
//! linearization, so a `RetirementAck` fed to the reducer is the genuine,
//! non-forgeable token minted by the supervisor — never hand-rolled.
//!
//! # Deferred (named non-goals for RT1)
//! - The concrete iroh executor and the M4 `Phase(Confirming)` emission are wired
//!   at the host / composition root; RT1 drives an injected executor port.
//! - Confirm timers, mailbox polling, capability duties, and storage intents are
//!   accepted as typed effects but left as no-op seams (RT2 / P6 integration / BN).
//! - Frontend subscriber epochs + bounded/coalesced queues are RT2.
//! - Evidence projection is RT3.

#![forbid(unsafe_code)]

mod card;
mod config;
mod error;
mod port;
mod runtime;

pub use config::RuntimeConfig;
pub use error::{AcquireError, CommandError};
pub use port::{
    AttemptExecution, AttemptExecutor, ExecutorSignal, SessionProvider, StopHandle, StopToken,
    stop_channel,
};
pub use runtime::{Runtime, ShutdownReport};
