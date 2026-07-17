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

use super::event::FailureCode;

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
                if was_confirming {
                    // The receipt is sufficient proof; stop the doomed ack wait
                    // instead of letting it hang into the QUIC idle timeout.
                    effects.push(Effect::CancelToken);
                }
                effects
            }
        }
    }

    fn on_pause(&mut self) -> Vec<Effect> {
        if !self.state.is_active() {
            return Vec::new();
        }
        let mut effects = self.exit_effects();
        self.state = State::Paused(PauseOrigin::Local);
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
            _ => return Vec::new(),
        };
        // A cancelled transfer is not "partway done" - it is abandoned. Clear
        // the progress at the source so every consumer agrees: the Cancelled
        // card reads 0, and a resume-from-cancelled (the ONLY fresh restart,
        // partial discarded) inherits 0 instead of the stale pre-cancel bar.
        self.bytes = 0;
        self.bytes_resumed = 0;
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
            } if matches!(
                self.state,
                S::Waiting | S::Connecting | S::Verifying | S::Transferring | S::Confirming
            ) =>
            {
                let mut effects = self.exit_effects();
                self.state = S::Completed;
                self.transfer_id = Some(transfer_id);
                self.file_name = Some(file_name);
                self.bytes = bytes;
                if self.total == 0 {
                    self.total = bytes; // receipt/existing-final paths skip Started
                }
                self.reason = None;
                self.reason_code = None;
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
mod tests {
    use super::*;
    use AttemptEvent as E;
    use envoix_session::TransferDirection::{Receive, Send};

    fn ev(attempt: u32, event: AttemptEvent) -> Input {
        Input::Event { attempt, event }
    }

    fn started() -> AttemptEvent {
        E::Started {
            transfer_id: "transfer-t1".into(),
            file_name: "a.zip".into(),
            total: 100,
            bytes_resumed: 0,
        }
    }

    fn completed(bytes: u64) -> AttemptEvent {
        E::Completed {
            transfer_id: "transfer-t1".into(),
            file_name: "a.zip".into(),
            bytes,
        }
    }

    fn verifying() -> AttemptEvent {
        E::Verifying {
            transfer_id: "transfer-v".into(),
            file_name: "v.bin".into(),
        }
    }

    fn confirming() -> AttemptEvent {
        E::Confirming {
            file_hash: "hash-of-sent-bytes".into(),
        }
    }

    fn failed(code: FailureCode) -> AttemptEvent {
        E::Failed {
            reason_code: code,
            reason: "test".into(),
        }
    }

    /// Drive a session into Transferring with some progress.
    fn transferring(direction: envoix_session::TransferDirection) -> Session {
        let mut s = Session::new(direction);
        assert!(s.reduce(ev(1, started())).is_empty());
        assert!(s.reduce(ev(1, E::Progress { bytes: 40 })).is_empty());
        assert_eq!(s.state, State::Transferring);
        assert_eq!(s.bytes, 40);
        s
    }

    #[test]
    fn receive_happy_path_posts_receipt() {
        let mut s = Session::new(Receive);
        s.reduce(ev(1, E::Advertised));
        assert_eq!(s.state, State::Waiting);
        s.reduce(ev(1, E::Pairing));
        assert_eq!(s.state, State::Connecting);
        s.reduce(ev(1, started()));
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        let effects = s.reduce(ev(1, completed(100)));
        assert_eq!(s.state, State::Completed);
        assert_eq!(effects, vec![Effect::PostReceipt]);
    }

    #[test]
    fn storage_failed_ends_active_states_and_spares_terminal_ones() {
        let mut s = transferring(Send);
        let effects = s.reduce(Input::StorageFailed);
        assert_eq!(s.state, State::Failed);
        assert!(s.reason.as_deref().unwrap_or("").contains("record store"));
        assert!(
            effects.contains(&Effect::CancelToken),
            "the live attempt stops"
        );

        let mut done = transferring(Send);
        done.reduce(ev(1, E::Progress { bytes: 100 }));
        done.reduce(ev(1, confirming()));
        done.reduce(ev(1, completed(100)));
        assert!(done.reduce(Input::StorageFailed).is_empty());
        assert_eq!(
            done.state,
            State::Completed,
            "terminal states are not rewritten"
        );
    }

    fn preparing(direction: TransferDirection) -> Session {
        let mut s = Session::new(direction);
        s.state = State::Preparing;
        s.facts.source_ready = false; // mirrors driver `start_staging`
        s
    }

    #[test]
    fn stage_complete_launches_the_first_attempt_fresh() {
        let mut s = preparing(Send);
        // Staging progress is owned by the machine (single source of truth).
        s.reduce(Input::StageProgress {
            generation: 1,
            bytes: 200,
        });
        assert_eq!(s.bytes, 200);
        let effects = s.reduce(Input::StageComplete { generation: 1 });
        assert!(s.facts.source_ready, "StageComplete marks the source ready");
        assert_eq!(s.state, State::Connecting);
        assert_eq!(
            s.attempt, 1,
            "still the first attempt, deferred past staging"
        );
        assert_eq!(
            s.bytes, 0,
            "staging bytes cleared; the transfer owns the bar"
        );
        assert_eq!(effects, vec![Effect::StartAttempt { resume: false }]);
    }

    #[test]
    fn stage_progress_only_moves_the_bar_in_preparing() {
        let mut s = preparing(Send);
        s.reduce(Input::StageProgress {
            generation: 1,
            bytes: 100,
        });
        assert_eq!(s.bytes, 100);
        let mut t = transferring(Send);
        t.reduce(ev(1, E::Progress { bytes: 50 }));
        t.reduce(Input::StageProgress {
            generation: 1,
            bytes: 999,
        });
        assert_eq!(t.bytes, 50, "stage progress is ignored outside Preparing");
    }

    #[test]
    fn stage_failed_fails_the_transfer_with_its_reason() {
        let mut s = preparing(Send);
        let effects = s.reduce(Input::StageFailed {
            generation: 1,
            reason: "source vanished".into(),
        });
        assert_eq!(s.state, State::Failed);
        assert_eq!(s.reason.as_deref(), Some("source vanished"));
        assert!(effects.is_empty());
    }

    #[test]
    fn cancel_during_preparing_abandons_without_a_wire_effect() {
        let mut s = preparing(Send);
        let effects = s.reduce(Input::Cancel);
        assert_eq!(s.state, State::Cancelled);
        assert!(
            !effects.contains(&Effect::CancelToken),
            "no attempt/peer exists, so nothing to signal"
        );
    }

    #[test]
    fn pause_during_preparing_is_a_noop() {
        let mut s = preparing(Send);
        assert!(s.reduce(Input::Pause).is_empty());
        assert_eq!(s.state, State::Preparing, "nothing to pause");
    }

    #[test]
    fn stage_inputs_off_preparing_are_dropped() {
        let mut s = transferring(Send);
        assert!(s.reduce(Input::StageComplete { generation: 1 }).is_empty());
        assert_eq!(s.state, State::Transferring, "no legal edge");
    }

    #[test]
    fn stale_generation_staging_inputs_are_rejected_after_retry() {
        // A staged send cancelled during Preparing, then retried, is a NEW
        // generation; the old worker's callbacks must not touch it.
        let mut s = preparing(Send); // attempt 1, source_ready = false
        s.reduce(Input::Cancel); // -> Cancelled
        let effects = s.reduce(Input::Resume); // source not ready -> re-stage
        assert_eq!(s.state, State::Preparing);
        assert_eq!(s.attempt, 2, "retry bumped the generation");
        assert!(
            effects.is_empty(),
            "no StartAttempt until the source is ready"
        );

        // The dead generation's callbacks are dropped structurally.
        assert!(s.reduce(Input::StageComplete { generation: 1 }).is_empty());
        assert_eq!(s.state, State::Preparing, "stale StageComplete ignored");
        assert!(
            s.reduce(Input::StageFailed {
                generation: 1,
                reason: "old".into(),
            })
            .is_empty()
        );
        assert_eq!(
            s.state,
            State::Preparing,
            "stale StageFailed cannot fail gen 2"
        );

        // The current generation's StageComplete DOES launch the attempt.
        let effects = s.reduce(Input::StageComplete { generation: 2 });
        assert_eq!(s.state, State::Connecting);
        assert!(s.facts.source_ready);
        assert_eq!(effects, vec![Effect::StartAttempt { resume: false }]);
    }

    #[test]
    fn resume_from_failed_staging_re_stages_not_the_wire() {
        let mut s = preparing(Send); // source_ready = false
        s.reduce(Input::StageFailed {
            generation: 1,
            reason: "unreadable".into(),
        });
        assert_eq!(s.state, State::Failed);
        let effects = s.reduce(Input::Resume);
        assert_eq!(
            s.state,
            State::Preparing,
            "not ready -> re-stage, not the wire"
        );
        assert_eq!(s.attempt, 2);
        assert!(
            effects.is_empty(),
            "no StartAttempt before the source is ready"
        );
    }

    #[test]
    fn resume_after_completed_staging_goes_to_the_wire() {
        let mut s = preparing(Send);
        s.reduce(Input::StageComplete { generation: 1 }); // -> Connecting, source_ready = true
        assert!(s.facts.source_ready);
        s.reduce(Input::Cancel); // -> Cancelled; a complete source is preserved
        assert!(
            s.facts.source_ready,
            "an already-complete staged source is not discarded on cancel"
        );
        let effects = s.reduce(Input::Resume);
        assert_eq!(
            s.state,
            State::Connecting,
            "ready source -> straight to the wire, no re-copy"
        );
        assert_eq!(effects, vec![Effect::StartAttempt { resume: false }]);
    }

    #[test]
    fn cancel_clears_progress_and_the_fresh_resume_inherits_it() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 50 }));
        assert_eq!(s.bytes, 50);
        // Cancel abandons the progress: the Cancelled card reads 0, not 50.
        s.reduce(Input::Cancel);
        assert_eq!(s.state, State::Cancelled);
        assert_eq!(s.bytes, 0, "a cancelled transfer is not partway done");
        assert_eq!(s.bytes_resumed, 0);
        // A resume-from-cancelled is a FRESH restart and simply inherits 0 -
        // no special case in on_resume.
        let effects = s.reduce(Input::Resume);
        assert_eq!(s.state, State::Connecting);
        assert_eq!(s.bytes, 0);
        assert!(effects.contains(&Effect::StartAttempt { resume: false }));
    }

    #[test]
    fn paused_resume_keeps_progress_until_started_corrects_it() {
        // A resume=true (Paused) restart keeps the last known bytes - the
        // partial is real and Started will set the exact resumed offset.
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 50 }));
        s.reduce(Input::Pause);
        s.reduce(Input::Resume);
        assert_eq!(s.bytes, 50, "a genuine resume does not zero the bar");
    }

    #[test]
    fn verifying_captures_identity_without_started() {
        // The short-circuit receive paths (existing final / receipt) jump to
        // Verifying without ever emitting Started; the identity facts must
        // not be lost or Remove has no name to clean and the card shows null.
        let mut s = Session::new(Receive);
        s.reduce(ev(1, verifying()));
        assert_eq!(s.state, State::Verifying);
        assert_eq!(s.transfer_id.as_deref(), Some("transfer-v"));
        assert_eq!(s.file_name.as_deref(), Some("v.bin"));
    }

    #[test]
    fn receipt_mismatch_is_a_fact_not_a_verdict() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, confirming()));
        s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Unconfirmed);

        let effects = s.reduce(Input::ReceiptMismatch);
        assert_eq!(
            s.state,
            State::Unconfirmed,
            "no transition: stale news, not a verdict"
        );
        assert!(
            effects.is_empty(),
            "polls keep running (the slot can be overwritten)"
        );
        assert!(s.facts.receipt_mismatch, "but the fact is recorded");

        // Terminal states ignore it entirely.
        let mut done = transferring(Send);
        done.reduce(ev(1, E::Progress { bytes: 100 }));
        done.reduce(ev(1, confirming()));
        done.reduce(ev(1, completed(100)));
        done.reduce(Input::ReceiptMismatch);
        assert!(!done.facts.receipt_mismatch);
    }

    #[test]
    fn send_happy_path_confirms_then_completes() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        let effects = s.reduce(ev(1, confirming()));
        assert_eq!(s.state, State::Confirming);
        assert_eq!(
            effects,
            vec![Effect::StartConfirmTimer, Effect::StartMailboxPoll]
        );
        // The committed proof basis: receipts are verified against this fact.
        assert_eq!(s.sent_hash.as_deref(), Some("hash-of-sent-bytes"));
        let effects = s.reduce(ev(1, completed(100)));
        assert_eq!(s.state, State::Completed);
        assert_eq!(
            effects,
            vec![Effect::StopConfirmTimer, Effect::StopMailboxPoll] // no receipt on send
        );
    }

    /// THE July regression: pause must never flip to Failed when the attempt's
    /// cancel echo lands.
    #[test]
    fn pause_survives_the_failed_echo() {
        let mut s = transferring(Send);
        let effects = s.reduce(Input::Pause);
        assert_eq!(s.state, State::Paused(PauseOrigin::Local));
        assert_eq!(effects, vec![Effect::PauseToken]);
        // The attempt's own Failed echo (attempt-current!) has no edge out.
        assert!(s.reduce(ev(1, failed(FailureCode::Cancelled))).is_empty());
        assert!(
            s.reduce(ev(1, failed(FailureCode::ConnectionLost)))
                .is_empty()
        );
        assert_eq!(s.state, State::Paused(PauseOrigin::Local));
    }

    /// THE second July regression: cancel during pairing must never be revived
    /// by the connecting burst or relabeled by the abort's Failed.
    #[test]
    fn cancel_during_pairing_survives_late_events() {
        let mut s = Session::new(Receive);
        let effects = s.reduce(Input::Cancel);
        assert_eq!(s.state, State::Cancelled);
        assert_eq!(effects, vec![Effect::CancelToken]);
        assert!(s.reduce(ev(1, E::Connecting)).is_empty());
        assert!(s.reduce(ev(1, E::Pairing)).is_empty());
        assert!(s.reduce(ev(1, failed(FailureCode::Cancelled))).is_empty());
        assert_eq!(s.state, State::Cancelled);
    }

    /// THE third July regression: stale bytes from a finished attempt must not
    /// fake an Unconfirmed after a re-join times out.
    #[test]
    fn stale_bytes_cannot_fake_unconfirmed() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, confirming()));
        s.reduce(ev(1, completed(100)));
        assert_eq!(s.state, State::Completed);
        // A send's Resume from Completed is ignored (nothing to re-join)…
        assert!(s.reduce(Input::Resume).is_empty());
        assert_eq!(s.state, State::Completed);
    }

    /// Completed is terminal for BOTH directions: re-verify is a courier-tier
    /// service, not a resurrection (design addendum 2026-07-09).
    #[test]
    fn completed_is_terminal_resume_is_a_noop() {
        for direction in [Send, Receive] {
            let mut s = transferring(direction);
            s.reduce(ev(1, completed(100)));
            assert!(s.reduce(Input::Resume).is_empty(), "{direction:?}");
            assert_eq!(s.state, State::Completed);
            assert_eq!(s.attempt, 1);
        }
    }

    #[test]
    fn confirming_connection_lost_escalates_to_mailbox() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, confirming()));
        let effects = s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Unconfirmed);
        assert_eq!(
            effects,
            vec![
                Effect::StopConfirmTimer,
                Effect::StopMailboxPoll,
                Effect::StartMailboxPoll
            ]
        );
        let effects = s.reduce(Input::ReceiptVerified);
        assert_eq!(s.state, State::Completed);
        assert_eq!(s.bytes, s.total);
        assert_eq!(effects, vec![Effect::StopMailboxPoll]);
    }

    #[test]
    fn confirm_timeout_escalates_proactively_and_stale_timers_are_ignored() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, confirming()));
        // A stale timer from another attempt does nothing.
        assert!(s.reduce(Input::ConfirmTimeout { attempt: 7 }).is_empty());
        let effects = s.reduce(Input::ConfirmTimeout { attempt: 1 });
        assert_eq!(s.state, State::Unconfirmed);
        assert_eq!(effects, vec![Effect::CancelToken, Effect::StartMailboxPoll]);
        // A second timeout is a no-op (state already resolved).
        assert!(s.reduce(Input::ConfirmTimeout { attempt: 1 }).is_empty());
    }

    /// Parallel proofs: the receipt can win WHILE the ack is still awaited.
    #[test]
    fn receipt_verified_during_confirming_completes_and_stops_the_wait() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, confirming()));
        let effects = s.reduce(Input::ReceiptVerified);
        assert_eq!(s.state, State::Completed);
        assert_eq!(
            effects,
            vec![
                Effect::StopConfirmTimer,
                Effect::StopMailboxPoll,
                Effect::CancelToken
            ]
        );
        // The attempt's late echoes land on a resting state and are dropped.
        assert!(s.reduce(ev(1, completed(100))).is_empty());
        assert!(s.reduce(ev(1, E::RunEnded { failure: None })).is_empty());
        assert_eq!(s.state, State::Completed);
    }

    #[test]
    fn peer_pause_and_lost_connection_classify_as_paused() {
        let mut s = transferring(Receive);
        s.reduce(ev(1, failed(FailureCode::PeerPaused)));
        assert_eq!(s.state, State::Paused(PauseOrigin::Peer));

        let mut s = transferring(Receive);
        s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Paused(PauseOrigin::Lost));

        // No progress on disk: a lost connection is a plain failure.
        let mut s = Session::new(Receive);
        s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Failed);
    }

    /// D1: discard fires ONLY on the explicit typed peer cancel.
    #[test]
    fn peer_cancel_discards_and_restart_is_fresh() {
        let mut s = transferring(Receive);
        let effects = s.reduce(ev(1, failed(FailureCode::PeerCancelled)));
        assert_eq!(s.state, State::Cancelled);
        assert_eq!(effects, vec![Effect::DiscardPartial]);
        let effects = s.reduce(Input::Resume);
        assert_eq!(s.attempt, 2);
        assert_eq!(effects, vec![Effect::StartAttempt { resume: false }]);
    }

    #[test]
    fn resume_from_paused_failed_unconfirmed_uses_resume_semantics() {
        for setup in [
            State::Paused(PauseOrigin::Local),
            State::Paused(PauseOrigin::Lost),
            State::Failed,
        ] {
            let mut s = transferring(Send);
            s.state = setup;
            let effects = s.reduce(Input::Resume);
            assert_eq!(s.state, State::Connecting, "from {setup:?}");
            assert_eq!(effects, vec![Effect::StartAttempt { resume: true }]);
        }
        // Unconfirmed also stops its poller on the way out.
        let mut s = transferring(Send);
        s.state = State::Unconfirmed;
        let effects = s.reduce(Input::Resume);
        assert_eq!(
            effects,
            vec![
                Effect::StopMailboxPoll,
                Effect::StartAttempt { resume: true }
            ]
        );
    }

    #[test]
    fn started_resets_bytes_on_resumed_attempt() {
        let mut s = transferring(Receive); // bytes = 40 from attempt 1
        s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        s.reduce(Input::Resume);
        let effects = s.reduce(ev(
            2,
            E::Started {
                transfer_id: "transfer-t2".into(),
                file_name: "a.zip".into(),
                total: 100,
                bytes_resumed: 40,
            },
        ));
        assert!(effects.is_empty());
        assert_eq!((s.bytes, s.bytes_resumed, s.attempt), (40, 40, 2));
    }

    #[test]
    fn completed_without_started_fills_total() {
        // receipt / existing-final short-circuits skip Started entirely.
        let mut s = Session::new(Receive);
        s.reduce(ev(1, completed(77)));
        assert_eq!(s.state, State::Completed);
        assert_eq!((s.bytes, s.total), (77, 77));
        assert_eq!(s.transfer_id.as_deref(), Some("transfer-t1"));
        assert_eq!(s.file_name.as_deref(), Some("a.zip"));
    }

    #[test]
    fn verifying_returns_to_connecting() {
        let mut s = Session::new(Receive);
        s.reduce(ev(1, verifying()));
        assert_eq!(s.state, State::Verifying);
        s.reduce(ev(1, E::Verified));
        assert_eq!(s.state, State::Connecting);
        // Completed straight from Verifying is also legal (existing-final path).
        let mut s = Session::new(Receive);
        s.reduce(ev(1, verifying()));
        s.reduce(ev(1, completed(10)));
        assert_eq!(s.state, State::Completed);
    }

    #[test]
    fn run_ended_is_a_belt_never_a_silent_success() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::RunEnded { failure: None }));
        assert_eq!(
            s.state,
            State::Failed,
            "clean end without Completed is a failure"
        );

        let mut s = transferring(Receive);
        s.reduce(ev(
            1,
            E::RunEnded {
                failure: Some((FailureCode::ConnectionLost, "gone".into())),
            },
        ));
        assert_eq!(s.state, State::Paused(PauseOrigin::Lost));

        // In a resting state, RunEnded is a no-op.
        let mut s = transferring(Send);
        s.reduce(Input::Pause);
        assert!(s.reduce(ev(1, E::RunEnded { failure: None })).is_empty());
        assert_eq!(s.state, State::Paused(PauseOrigin::Local));
    }

    #[test]
    fn cancel_from_resting_states_and_terminal_inputs_ignored() {
        // Cancel works from Paused/Unconfirmed without a token (attempt is dead).
        let mut s = transferring(Send);
        s.reduce(Input::Pause);
        let effects = s.reduce(Input::Cancel);
        assert_eq!(s.state, State::Cancelled);
        assert_eq!(effects, Vec::new());
        // …but not from Completed/Failed/Cancelled.
        for setup in [State::Completed, State::Failed, State::Cancelled] {
            let mut s = transferring(Send);
            s.state = setup;
            assert!(s.reduce(Input::Cancel).is_empty());
            assert_eq!(s.state, setup);
        }
        // Pause is meaningless at rest.
        let mut s = transferring(Send);
        s.state = State::Unconfirmed;
        assert!(s.reduce(Input::Pause).is_empty());
    }

    /// Property: replaying any events of a superseded attempt never changes
    /// anything, whatever they are.
    #[test]
    fn stale_attempt_events_are_inert() {
        let mut s = transferring(Receive);
        s.reduce(ev(1, failed(FailureCode::ConnectionLost)));
        s.reduce(Input::Resume); // attempt 2
        let snapshot = s.clone();
        for event in [
            E::Advertised,
            E::Pairing,
            E::Connecting,
            E::Started {
                transfer_id: "x".into(),
                file_name: "x".into(),
                total: 9,
                bytes_resumed: 0,
            },
            E::Progress { bytes: 999 },
            verifying(),
            E::Verified,
            confirming(),
            completed(999),
            failed(FailureCode::PeerCancelled),
            E::RunEnded { failure: None },
        ] {
            assert!(s.reduce(ev(1, event.clone())).is_empty(), "{event:?}");
            assert_eq!(s, snapshot, "{event:?}");
        }
    }

    /// Fuzz-lite: arbitrary interleavings never panic and always keep the
    /// bytes/total invariant.
    #[test]
    fn random_interleavings_hold_invariants() {
        let inputs = |attempt: u32| {
            vec![
                Input::Pause,
                Input::Cancel,
                Input::Resume,
                Input::ReceiptVerified,
                Input::ConfirmTimeout { attempt },
                ev(attempt, E::Advertised),
                ev(attempt, E::Connecting),
                ev(attempt, started()),
                ev(attempt, E::Progress { bytes: 50 }),
                ev(attempt, confirming()),
                ev(attempt, completed(100)),
                ev(attempt, failed(FailureCode::ConnectionLost)),
                ev(attempt, failed(FailureCode::PeerCancelled)),
                ev(attempt, E::RunEnded { failure: None }),
            ]
        };
        // Deterministic pseudo-random walk (no Date/rand in tests needed).
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        for direction in [Send, Receive] {
            let mut s = Session::new(direction);
            for _ in 0..5000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let menu = inputs(s.attempt.saturating_sub(seed as u32 % 2)); // current or stale
                let pick = (seed >> 33) as usize % menu.len();
                s.reduce(menu[pick].clone());
                assert!(s.bytes <= s.total || s.total == 0, "bytes>total: {s:?}");
            }
        }
    }
}

#[cfg(test)]
mod serde_tests {
    use super::*;
    use envoix_session::TransferDirection::Send;

    /// The snapshot JSON shape is the FFI contract: state/origin flatten to
    /// top-level keys the frontend reads with optString.
    #[test]
    fn session_serializes_flat_state_and_origin() {
        let mut s = Session::new(Send);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "connecting");
        assert!(v["origin"].is_null() || v.get("origin").is_none());

        s.state = State::Paused(PauseOrigin::Peer);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "paused");
        assert_eq!(v["origin"], "peer");
        // Matches the existing public event stream shape (no rename_all on the
        // wire-adjacent TransferDirection); the app reads direction from its own
        // Spec, not the snapshot.
        assert_eq!(v["direction"], "Send");
    }

    /// Pins every `State` to the exact wire string the Android `Status` enum
    /// maps (`Status.fromWire`, `Transfer.kt`). A rename here breaks this test
    /// instead of silently freezing a card on the unmapped string. Keep in sync
    /// with the Kotlin `every_wire_string_maps_to_a_status` test.
    #[test]
    fn every_state_serializes_to_its_wire_string() {
        let cases: &[(State, &str)] = &[
            (State::Preparing, "preparing"),
            (State::Waiting, "waiting"),
            (State::Connecting, "connecting"),
            (State::Verifying, "verifying"),
            (State::Transferring, "transferring"),
            (State::Confirming, "confirming"),
            (State::Paused(PauseOrigin::Local), "paused"),
            (State::Unconfirmed, "unconfirmed"),
            (State::Completed, "completed"),
            (State::Failed, "failed"),
            (State::Cancelled, "cancelled"),
        ];
        for (state, expected) in cases {
            let v = serde_json::to_value(state).unwrap();
            assert_eq!(v["state"].as_str(), Some(*expected), "{state:?}");
        }
    }

    #[test]
    fn kind_labels_name_variants_and_fold_events() {
        assert_eq!(Input::Cancel.kind(), "Cancel");
        assert_eq!(
            Input::StageComplete { generation: 1 }.kind(),
            "StageComplete"
        );
        assert_eq!(Input::ReceiptPosted.kind(), "ReceiptPosted");
        // a core event folds to its AttemptEvent name — the input reads as the
        // fact it carries, not the generic "Event".
        let verified = Input::Event {
            attempt: 1,
            event: AttemptEvent::Verified,
        };
        assert_eq!(verified.kind(), "Verified");
        let progress = Input::Event {
            attempt: 1,
            event: AttemptEvent::Progress { bytes: 0 },
        };
        assert_eq!(progress.kind(), "Progress");
        assert_eq!(Effect::PostReceipt.kind(), "PostReceipt");
        assert_eq!(Effect::StartAttempt { resume: true }.kind(), "StartAttempt");
    }
}
