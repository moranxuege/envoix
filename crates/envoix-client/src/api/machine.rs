//! The transfer-session state machine (see `docs/design/transfer-state-machine.md`).
//!
//! A *session* is one card in a UI: it spans multiple *attempts* (run → pause →
//! resume → run …). This module is the PURE reducer — no I/O, no clocks, no
//! platform types: `Session::reduce(Input) -> Vec<Effect>`. A driver owns the
//! attempts and executes the effects; frontends render [`Session`] snapshots
//! and stop interpreting events.
//!
//! The rules that kill the July bug class by construction:
//! - **User intent is a first-class input**: a user-initiated state is left
//!   only by another user action or by the outcome that action requested.
//! - **Attempt tagging**: every core event carries the attempt it belongs to;
//!   inputs from a stale attempt are dropped before any transition logic runs.
//! - **Observations move the machine along legal edges only**; an observation
//!   with no legal edge is dropped, never applied.

use envoix_session::TransferDirection;
use envoix_types::DataPath;
use serde::{Deserialize, Serialize};

use super::error::TransferFailure;
use super::event::SessionFailureCode as FailureCode;

/// Where a pause came from — a label detail, not a distinct state: the
/// affordance (Resume) is identical.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseOrigin {
    /// The local user paused.
    Local,
    /// The peer reported pausing (typed, best-effort).
    Peer,
    /// The connection was lost with progress on disk — resumable, cause unknown.
    Lost,
}

/// The session state (one card).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "origin")]
pub enum State {
    /// SEND only: staging a platform source (e.g. an Android `content://`)
    /// into the transfer's path before any attempt. No peer is contacted; the
    /// record exists so the staging survives process death (see the
    /// `Preparing` design addendum).
    Preparing,
    /// Advertising an invite / parked in the room; no peer yet.
    Waiting,
    /// Pairing and connecting.
    Connecting,
    /// Hashing (resume prefix or an existing final file); no bytes moving.
    Verifying,
    /// Bytes moving.
    Transferring,
    /// SEND only: all bytes and the Complete frame sent; awaiting the ack.
    Confirming,
    /// Resumable stop.
    Paused(PauseOrigin),
    /// Send delivered every byte; proof pending (mailbox poll active).
    Unconfirmed,
    /// Receive bytes are committed to staging; the platform still needs to
    /// publish them into its user-visible destination.
    AwaitingPublication,
    /// Done. A receive may re-enter Connecting to serve a peer's re-verify.
    Completed,
    /// Genuine failure (typed reason retained).
    Failed,
    /// User abandoned this transfer.
    Cancelled,
}

impl State {
    /// States with a live attempt underneath.
    fn is_active(self) -> bool {
        // Exhaustive on purpose (no `_`): adding a State is a compile error until
        // it is classified here. NB the frontend's isActive / isTerminal are
        // deliberately DIFFERENT predicates (Preparing pins the tray there); each
        // is independently exhaustive so a new state must be classified in all.
        match self {
            State::Waiting
            | State::Connecting
            | State::Verifying
            | State::Transferring
            | State::Confirming => true,
            State::Preparing
            | State::Paused(_)
            | State::Unconfirmed
            | State::AwaitingPublication
            | State::Completed
            | State::Failed
            | State::Cancelled => false,
        }
    }
}

/// A core event, already reduced to the machine's alphabet (the driver maps
/// the public `TransferEvent` stream onto this).
#[derive(Clone, Debug, PartialEq)]
pub enum AttemptEvent {
    Advertised,
    Pairing,
    Connecting,
    Connected(DataPath),
    PathChanged(DataPath),
    Started {
        transfer_id: String,
        file_name: String,
        total: u64,
        bytes_resumed: u64,
    },
    Progress {
        bytes: u64,
    },
    Verifying {
        transfer_id: String,
        file_name: String,
    },
    Verified,
    Confirming {
        file_hash: String,
    },
    Completed {
        transfer_id: String,
        file_name: String,
        bytes: u64,
        completed_file_path: Option<String>,
    },
    Failed {
        reason_code: FailureCode,
        reason: String,
    },
    /// The attempt future returned. The belt behind the "every failed run ends
    /// its stream with a typed Failed" contract: normally a no-op because a
    /// terminal event already moved the machine to a resting state.
    RunEnded {
        failure: Option<(FailureCode, String)>,
    },
}

/// One input to the reducer.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// User intent: pause the current attempt (resumable).
    Pause,
    /// User intent: abandon this transfer (peer told to discard, best-effort).
    Cancel,
    /// User intent: resume/retry — launches a new attempt.
    Resume,
    /// Staging finished: the source is at the transfer's path, launch attempt 1.
    /// Staging copied more bytes into the durable path (snapshot-only, never
    /// persisted). Keeps the machine the single source of bytes.
    StageProgress {
        generation: u32,
        bytes: u64,
    },
    StageComplete {
        generation: u32,
    },
    /// Staging failed (e.g. the source could not be read); the reason is kept.
    StageFailed {
        generation: u32,
        reason: String,
    },
    /// A core event from attempt `attempt`.
    Event {
        attempt: u32,
        event: AttemptEvent,
    },
    /// The driver's confirm timer for attempt `attempt` expired.
    ConfirmTimeout {
        attempt: u32,
    },
    /// The mailbox receipt was fetched and VERIFIED against the local file.
    ReceiptVerified,
    /// The mailbox slot opened with this transfer's key but named different
    /// content than the committed sent facts (see [`Facts::receipt_mismatch`]).
    ReceiptMismatch,
    /// The driver could not commit the record after bounded retries: the
    /// durable authority is unwritable, so the session ends visibly instead
    /// of running ahead of a store that cannot follow.
    StorageFailed,
    /// Receiver: the sealed receipt POST was acknowledged by the rdz - the
    /// confirmation duty is discharged (monotone fact, any state).
    ReceiptPosted,
    /// The native platform published a staged receive and reports its final
    /// path or URI.
    Published {
        path: String,
    },
}

/// Side effects for the driver. The machine never performs them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    /// Spawn a new attempt (the session's `attempt` was already incremented).
    StartAttempt {
        resume: bool,
    },
    /// Pause the current attempt's cancel token.
    PauseToken,
    /// Cancel the current attempt's cancel token.
    CancelToken,
    StartConfirmTimer,
    StopConfirmTimer,
    StartMailboxPoll,
    StopMailboxPoll,
    /// Receive completed: seal + post the completion receipt (with retries).
    PostReceipt,
    /// D1: the peer explicitly cancelled — discard the partial + resume state.
    DiscardPartial,
    /// Delete a receive that was committed to staging but not yet published.
    DiscardStagedFile,
}

/// Monotone accomplishments: set-once, never cleared, serialized with the
/// record. State is DERIVED from facts, never the reverse (design addendum).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Facts {
    /// Receiver: the sender's proof is deliverably placed - the sealed receipt
    /// POST was acknowledged by the rdz (or, future: a clean close showed the
    /// ack arrived). Gates the "still on duty" UI and restore re-posting.
    pub proof_delivered: bool,
    /// Send: the mailbox slot held an authenticated receipt for DIFFERENT
    /// content (opened with this transfer's key; hash/size didn't match the
    /// committed sent facts). Stale news, not a verdict - the receiver
    /// overwrites the slot when it re-completes a resumed offer, so polling
    /// continues; recorded for diagnostics and the UI.
    #[serde(default)]
    pub receipt_mismatch: bool,
    /// SEND: the local source is complete and ready to send — true for a direct
    /// send from creation, set true when a `StageComplete` of the current
    /// generation is accepted, never cleared. A retry consults THIS (not
    /// state/origin): `false` re-stages (back to `Preparing`), `true` launches
    /// the network attempt. Monotone false→true.
    #[serde(default)]
    pub source_ready: bool,
}

/// One transfer session (one card): the machine state plus the observable
/// data a frontend renders. Serializable — this is the snapshot payload, and
/// the future durable TransferRecord.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Session {
    pub direction: TransferDirection,
    #[serde(flatten)]
    pub state: State,
    /// Monotonic attempt number; events from other attempts are ignored.
    pub attempt: u32,
    pub transfer_id: Option<String>,
    pub file_name: Option<String>,
    pub bytes: u64,
    pub total: u64,
    pub bytes_resumed: u64,
    pub path: Option<DataPath>,
    pub reason: Option<String>,
    pub reason_code: Option<FailureCode>,
    /// Full stable failure classification retained across app restarts. Older
    /// records omit it and fall back to `reason_code` + `reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TransferFailure>,
    /// Whether a receive must wait for an external platform publication.
    #[serde(default)]
    pub publication_required: bool,
    /// Core committed staging path, then the final published path/URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_file_path: Option<String>,
    /// Send: BLAKE3 hash of the bytes the current attempt actually sent
    /// (set on Confirming). The committed proof basis - mailbox receipts are
    /// verified against this fact, never against the mutable source path.
    #[serde(default)]
    pub sent_hash: Option<String>,
    #[serde(default)]
    pub facts: Facts,
}

impl AttemptEvent {
    /// The variant name, for the diagnostic timeline (docs/design/diagnostics.md).
    pub fn kind(&self) -> &'static str {
        match self {
            AttemptEvent::Advertised => "Advertised",
            AttemptEvent::Pairing => "Pairing",
            AttemptEvent::Connecting => "Connecting",
            AttemptEvent::Connected(_) => "Connected",
            AttemptEvent::PathChanged(_) => "PathChanged",
            AttemptEvent::Started { .. } => "Started",
            AttemptEvent::Progress { .. } => "Progress",
            AttemptEvent::Verifying { .. } => "Verifying",
            AttemptEvent::Verified => "Verified",
            AttemptEvent::Confirming { .. } => "Confirming",
            AttemptEvent::Completed { .. } => "Completed",
            AttemptEvent::Failed { .. } => "Failed",
            AttemptEvent::RunEnded { .. } => "RunEnded",
        }
    }
}

impl Input {
    /// A short label for the timeline's `machine.input` event. Core events fold
    /// to their [`AttemptEvent`] name so the input reads as the fact it carries.
    pub fn kind(&self) -> &'static str {
        match self {
            Input::Pause => "Pause",
            Input::Cancel => "Cancel",
            Input::Resume => "Resume",
            Input::StageProgress { .. } => "StageProgress",
            Input::StageComplete { .. } => "StageComplete",
            Input::StageFailed { .. } => "StageFailed",
            Input::Event { event, .. } => event.kind(),
            Input::ConfirmTimeout { .. } => "ConfirmTimeout",
            Input::ReceiptVerified => "ReceiptVerified",
            Input::ReceiptMismatch => "ReceiptMismatch",
            Input::StorageFailed => "StorageFailed",
            Input::ReceiptPosted => "ReceiptPosted",
            Input::Published { .. } => "Published",
        }
    }
}

impl Effect {
    /// The variant name, for the timeline's `effect.dispatched` event.
    pub fn kind(&self) -> &'static str {
        match self {
            Effect::StartAttempt { .. } => "StartAttempt",
            Effect::PauseToken => "PauseToken",
            Effect::CancelToken => "CancelToken",
            Effect::StartConfirmTimer => "StartConfirmTimer",
            Effect::StopConfirmTimer => "StopConfirmTimer",
            Effect::StartMailboxPoll => "StartMailboxPoll",
            Effect::StopMailboxPoll => "StopMailboxPoll",
            Effect::PostReceipt => "PostReceipt",
            Effect::DiscardPartial => "DiscardPartial",
            Effect::DiscardStagedFile => "DiscardStagedFile",
        }
    }
}

impl Session {
    /// A new session; the driver launches attempt 1 at construction.
    pub fn new(direction: TransferDirection) -> Self {
        Self {
            direction,
            state: State::Connecting,
            attempt: 1,
            transfer_id: None,
            file_name: None,
            bytes: 0,
            total: 0,
            bytes_resumed: 0,
            path: None,
            reason: None,
            reason_code: None,
            failure: None,
            publication_required: false,
            completed_file_path: None,
            sent_hash: None,
            // A session born in Connecting has its source in hand (a direct send,
            // or a receive with no send-source). `start_staging` overrides this
            // to false before any byte is copied.
            facts: Facts {
                source_ready: true,
                ..Facts::default()
            },
        }
    }

    /// Apply one input; returns the effects the driver must execute.
    /// Inputs that have no legal edge from the current state are dropped.
    pub fn reduce(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Pause => self.on_pause(),
            Input::Cancel => self.on_cancel(),
            Input::Resume => self.on_resume(),
            Input::Event { attempt, event } => {
                if attempt != self.attempt {
                    return Vec::new(); // stale attempt: dropped structurally
                }
                self.on_event(event)
            }
            Input::ConfirmTimeout { attempt } => {
                if attempt != self.attempt || self.state != State::Confirming {
                    return Vec::new(); // stale timer, or already resolved
                }
                // Stop waiting on the dying ack; the mailbox polls (already
                // running in parallel) continue as the remaining proof channel.
                self.state = State::Unconfirmed;
                vec![Effect::CancelToken, Effect::StartMailboxPoll]
            }
            Input::ReceiptPosted => {
                self.facts.proof_delivered = true;
                Vec::new()
            }
            Input::StageProgress { generation, bytes }
                if generation == self.attempt && self.state == State::Preparing =>
            {
                self.bytes = bytes;
                Vec::new()
            }
            Input::StageProgress { .. } => Vec::new(), // stale generation, or not Preparing
            Input::StageComplete { generation }
                if generation == self.attempt && self.state == State::Preparing =>
            {
                // Staging produced the source; mark it ready and launch the
                // attempt. `attempt` stays as-is — THIS is the attempt deferred
                // past staging — and it is fresh. Staging bytes are cleared: the
                // transfer owns the bar from here.
                self.state = State::Connecting;
                self.bytes = 0;
                self.bytes_resumed = 0;
                self.facts.source_ready = true;
                vec![Effect::StartAttempt { resume: false }]
            }
            Input::StageComplete { .. } => Vec::new(), // stale generation, or not Preparing
            Input::StageFailed { generation, reason }
                if generation == self.attempt && self.state == State::Preparing =>
            {
                self.state = State::Failed;
                self.reason = Some(reason);
                self.reason_code = Some(FailureCode::Other);
                Vec::new()
            }
            Input::StageFailed { .. } => Vec::new(), // stale generation, or not Preparing
            Input::StorageFailed
                if !matches!(
                    self.state,
                    State::Completed | State::Failed | State::Cancelled
                ) =>
            {
                let mut effects = self.exit_effects();
                effects.push(Effect::CancelToken);
                self.state = State::Failed;
                self.reason = Some("transfer record store is unwritable".into());
                self.reason_code = Some(FailureCode::Other);
                self.failure = None;
                effects
            }
            Input::StorageFailed => Vec::new(), // terminal states: nothing to end
            Input::ReceiptMismatch => {
                if matches!(self.state, State::Confirming | State::Unconfirmed) {
                    self.facts.receipt_mismatch = true;
                }
                // No transition, no effects: the bounded polls keep running -
                // a receiver that re-completes the resumed offer overwrites
                // the slot, and a later poll then verifies.
                Vec::new()
            }
            Input::ReceiptVerified => {
                let was_confirming = self.state == State::Confirming;
                if self.state != State::Unconfirmed && !was_confirming {
                    return Vec::new();
                }
                let mut effects = self.exit_effects();
                self.state = State::Completed;
                self.bytes = self.total;
                self.reason = None;
                self.reason_code = None;
                self.failure = None;
                if was_confirming {
                    // The receipt is sufficient proof; stop the doomed ack wait
                    // instead of letting it hang into the QUIC idle timeout.
                    effects.push(Effect::CancelToken);
                }
                effects
            }
            Input::Published { path } => {
                if self.state != State::AwaitingPublication
                    || self.direction != TransferDirection::Receive
                    || path.trim().is_empty()
                {
                    return Vec::new();
                }
                self.state = State::Completed;
                self.completed_file_path = Some(path);
                self.reason = None;
                self.reason_code = None;
                self.failure = None;
                Vec::new()
            }
        }
    }

    fn on_pause(&mut self) -> Vec<Effect> {
        if !self.state.is_active() {
            return Vec::new();
        }
        let mut effects = self.exit_effects();
        self.state = State::Paused(PauseOrigin::Local);
        self.failure = None;
        effects.push(Effect::PauseToken);
        effects
    }

    fn on_cancel(&mut self) -> Vec<Effect> {
        let effects = match self.state {
            s if s.is_active() => {
                let mut effects = self.exit_effects();
                self.state = State::Cancelled;
                effects.push(Effect::CancelToken);
                effects
            }
            // A resting-but-unfinished card can still be abandoned. Preparing
            // has no attempt/peer, so no CancelToken - just abandon.
            State::Preparing | State::Paused(_) | State::Unconfirmed => {
                let effects = self.exit_effects();
                self.state = State::Cancelled;
                effects
            }
            State::AwaitingPublication if self.direction == TransferDirection::Receive => {
                let mut effects = self.exit_effects();
                self.state = State::Cancelled;
                effects.push(Effect::DiscardStagedFile);
                effects
            }
            _ => return Vec::new(),
        };
        // A cancelled transfer is not "partway done" - it is abandoned. Clear
        // the progress at the source so every consumer agrees: the Cancelled
        // card reads 0, and a resume-from-cancelled (the ONLY fresh restart,
        // partial discarded) inherits 0 instead of the stale pre-cancel bar.
        self.bytes = 0;
        self.bytes_resumed = 0;
        self.failure = None;
        effects
    }

    fn on_resume(&mut self) -> Vec<Effect> {
        let resume = match self.state {
            State::Paused(_) | State::Unconfirmed | State::Failed => true,
            // D1: a cancelled transfer restarts FRESH (partials were discarded).
            State::Cancelled => false,
            // Completed is TERMINAL: serving a peer's re-verify is a courier-
            // tier service (driver ServeReverify), never a lifecycle transition
            // - so no state exists that can lose the completion fact.
            _ => return Vec::new(),
        };
        // A new generation for the retry, ALWAYS — including a retry back into
        // Preparing — so a stale staging result from the previous generation is
        // rejected by the reducer.
        self.attempt += 1;
        self.reason = None;
        self.reason_code = None;
        self.failure = None;
        let mut effects = self.exit_effects();
        if self.facts.source_ready {
            // The local source is complete: go straight to the wire.
            self.state = State::Connecting;
            effects.push(Effect::StartAttempt { resume });
        } else {
            // A staged send whose source is not yet ready (cancelled or failed
            // before StageComplete): re-stage under the new generation. No
            // StartAttempt until StageComplete{new}; the platform re-stages on
            // observing Preparing.
            self.state = State::Preparing;
            self.bytes = 0;
            self.bytes_resumed = 0;
        }
        effects
    }

    fn on_event(&mut self, event: AttemptEvent) -> Vec<Effect> {
        use AttemptEvent as E;
        use State as S;
        match event {
            // Data updates, legal in any active state.
            E::Connected(path) | E::PathChanged(path) if self.state.is_active() => {
                self.path = Some(path);
                Vec::new()
            }
            E::Advertised if matches!(self.state, S::Connecting) => {
                self.state = S::Waiting;
                Vec::new()
            }
            E::Pairing | E::Connecting if matches!(self.state, S::Waiting | S::Connecting) => {
                self.state = S::Connecting;
                Vec::new()
            }
            E::Started {
                transfer_id,
                file_name,
                total,
                bytes_resumed,
            } if matches!(self.state, S::Waiting | S::Connecting | S::Verifying) => {
                self.state = S::Transferring;
                self.transfer_id = Some(transfer_id);
                self.file_name = Some(file_name);
                self.total = total;
                // Stale bytes from a previous attempt cannot leak into this one.
                self.bytes = bytes_resumed;
                self.bytes_resumed = bytes_resumed;
                Vec::new()
            }
            E::Progress { bytes } if self.state == S::Transferring => {
                self.bytes = bytes;
                Vec::new()
            }
            E::Verifying {
                transfer_id,
                file_name,
            } if matches!(self.state, S::Waiting | S::Connecting) => {
                self.state = S::Verifying;
                // Identity facts arrive HERE on the short-circuit paths
                // (existing final / receipt re-confirm), which never emit
                // Started - without capture, the record has no name for
                // Remove to clean and the card shows null.
                self.transfer_id = Some(transfer_id);
                self.file_name = Some(file_name);
                Vec::new()
            }
            E::Verified if self.state == S::Verifying => {
                // Hashing happens pre-Started (resume prefix / existing final);
                // return to Connecting and let Started or Completed follow.
                self.state = S::Connecting;
                Vec::new()
            }
            E::Confirming { file_hash } if self.state == S::Transferring => {
                self.state = S::Confirming;
                self.sent_hash = Some(file_hash);
                // Parallel proofs (design review): the mailbox is polled WHILE
                // the in-band ack is awaited - whichever proof lands first wins.
                // On a healthy path the ack beats the first poll and no HTTP
                // fires; on a dead path the receipt confirms in seconds instead
                // of after the full confirm timeout.
                vec![Effect::StartConfirmTimer, Effect::StartMailboxPoll]
            }
            E::Completed {
                transfer_id,
                file_name,
                bytes,
                completed_file_path,
            } if matches!(
                self.state,
                S::Waiting | S::Connecting | S::Verifying | S::Transferring | S::Confirming
            ) =>
            {
                let completed_without_started = self.direction == TransferDirection::Receive
                    && matches!(self.state, S::Waiting | S::Connecting | S::Verifying);
                let mut effects = self.exit_effects();
                self.state =
                    if self.direction == TransferDirection::Receive && self.publication_required {
                        S::AwaitingPublication
                    } else {
                        S::Completed
                    };
                self.transfer_id = Some(transfer_id);
                self.file_name = Some(file_name);
                self.completed_file_path = completed_file_path;
                self.bytes = bytes;
                // Receive paths that complete without `Started` are the
                // existing-final and durable-receipt short circuits: all
                // bytes were already present before this attempt. Preserve
                // that fact in the canonical snapshot instead of letting the
                // raw verification event be overwritten with zero.
                if completed_without_started {
                    self.bytes_resumed = bytes;
                }
                if self.total == 0 {
                    self.total = bytes; // receipt/existing-final paths skip Started
                }
                self.reason = None;
                self.reason_code = None;
                self.failure = None;
                if self.direction == TransferDirection::Receive {
                    effects.push(Effect::PostReceipt);
                }
                effects
            }
            E::Failed {
                reason_code,
                reason,
            } if self.state.is_active() => self.classify(reason_code, reason),
            E::RunEnded { failure } if self.state.is_active() => {
                // Belt: a terminal event should already have moved us. Classify
                // from the run result; a clean end with no Completed observed is
                // treated as an unclassified failure, never silent success.
                match failure {
                    Some((code, reason)) => self.classify(code, reason),
                    None => self.classify(
                        FailureCode::Other,
                        "attempt ended without a terminal event".into(),
                    ),
                }
            }
            _ => Vec::new(), // no legal edge: dropped
        }
    }

    /// The one classification table (design doc §classify): typed code first,
    /// durable facts as fallback. Only called from an active state.
    fn classify(&mut self, code: FailureCode, reason: String) -> Vec<Effect> {
        let mut effects = self.exit_effects();
        let was_confirming = self.state == State::Confirming;
        let (state, extra): (State, Option<Effect>) = match code {
            // Echo of a local intent - unreachable normally (user inputs move
            // the state first); defensive keep.
            FailureCode::Paused | FailureCode::Cancelled => return effects,
            FailureCode::PeerPaused => (State::Paused(PauseOrigin::Peer), None),
            // D1: discard ONLY on the explicit typed peer cancel.
            FailureCode::PeerCancelled => (State::Cancelled, Some(Effect::DiscardPartial)),
            FailureCode::ConnectionLost if was_confirming => {
                (State::Unconfirmed, Some(Effect::StartMailboxPoll))
            }
            FailureCode::ConnectionLost if self.bytes > 0 => {
                (State::Paused(PauseOrigin::Lost), None)
            }
            _ => (State::Failed, None),
        };
        self.state = state;
        self.reason = Some(reason);
        self.reason_code = Some(code);
        self.failure = None;
        effects.extend(extra);
        effects
    }

    /// Effects owed when leaving the current state (timers, pollers).
    fn exit_effects(&self) -> Vec<Effect> {
        match self.state {
            State::Confirming => vec![Effect::StopConfirmTimer, Effect::StopMailboxPoll],
            State::Unconfirmed => vec![Effect::StopMailboxPoll],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "machine_serde_tests.rs"]
mod serde_tests;
