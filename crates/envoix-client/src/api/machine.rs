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

use super::event::SessionFailureCode;

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
        matches!(
            self,
            State::Waiting
                | State::Connecting
                | State::Verifying
                | State::Transferring
                | State::Confirming
        )
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
    Verifying,
    Verified,
    Confirming,
    Completed {
        bytes: u64,
    },
    Failed {
        reason_code: SessionFailureCode,
        reason: String,
    },
    /// The attempt future returned. The belt behind the "every failed run ends
    /// its stream with a typed Failed" contract: normally a no-op because a
    /// terminal event already moved the machine to a resting state.
    RunEnded {
        failure: Option<(SessionFailureCode, String)>,
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
    /// A core event from attempt `attempt`.
    Event { attempt: u32, event: AttemptEvent },
    /// The driver's confirm timer for attempt `attempt` expired.
    ConfirmTimeout { attempt: u32 },
    /// The mailbox receipt was fetched and VERIFIED against the local file.
    ReceiptVerified,
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
    pub reason_code: Option<SessionFailureCode>,
    #[serde(default)]
    pub facts: Facts,
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
            facts: Facts::default(),
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
        match self.state {
            s if s.is_active() => {
                let mut effects = self.exit_effects();
                self.state = State::Cancelled;
                effects.push(Effect::CancelToken);
                effects
            }
            // A resting-but-unfinished card can still be abandoned.
            State::Paused(_) | State::Unconfirmed => {
                let effects = self.exit_effects();
                self.state = State::Cancelled;
                effects
            }
            _ => Vec::new(),
        }
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
        let mut effects = self.exit_effects();
        self.state = State::Connecting;
        self.attempt += 1;
        self.reason = None;
        self.reason_code = None;
        effects.push(Effect::StartAttempt { resume });
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
            E::Verifying if matches!(self.state, S::Waiting | S::Connecting) => {
                self.state = S::Verifying;
                Vec::new()
            }
            E::Verified if self.state == S::Verifying => {
                // Hashing happens pre-Started (resume prefix / existing final);
                // return to Connecting and let Started or Completed follow.
                self.state = S::Connecting;
                Vec::new()
            }
            E::Confirming if self.state == S::Transferring => {
                self.state = S::Confirming;
                // Parallel proofs (design review): the mailbox is polled WHILE
                // the in-band ack is awaited - whichever proof lands first wins.
                // On a healthy path the ack beats the first poll and no HTTP
                // fires; on a dead path the receipt confirms in seconds instead
                // of after the full confirm timeout.
                vec![Effect::StartConfirmTimer, Effect::StartMailboxPoll]
            }
            E::Completed { bytes }
                if matches!(
                    self.state,
                    S::Waiting | S::Connecting | S::Verifying | S::Transferring | S::Confirming
                ) =>
            {
                let mut effects = self.exit_effects();
                self.state = S::Completed;
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
                        SessionFailureCode::Other,
                        "attempt ended without a terminal event".into(),
                    ),
                }
            }
            _ => Vec::new(), // no legal edge: dropped
        }
    }

    /// The one classification table (design doc §classify): typed code first,
    /// durable facts as fallback. Only called from an active state.
    fn classify(&mut self, code: SessionFailureCode, reason: String) -> Vec<Effect> {
        let mut effects = self.exit_effects();
        let was_confirming = self.state == State::Confirming;
        let (state, extra): (State, Option<Effect>) = match code {
            // Echo of a local intent - unreachable normally (user inputs move
            // the state first); defensive keep.
            SessionFailureCode::Paused | SessionFailureCode::Cancelled => return effects,
            SessionFailureCode::PeerPaused => (State::Paused(PauseOrigin::Peer), None),
            // D1: discard ONLY on the explicit typed peer cancel.
            SessionFailureCode::PeerCancelled => (State::Cancelled, Some(Effect::DiscardPartial)),
            SessionFailureCode::ConnectionLost if was_confirming => {
                (State::Unconfirmed, Some(Effect::StartMailboxPoll))
            }
            SessionFailureCode::ConnectionLost if self.bytes > 0 => {
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

    fn failed(code: SessionFailureCode) -> AttemptEvent {
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
        let effects = s.reduce(ev(1, E::Completed { bytes: 100 }));
        assert_eq!(s.state, State::Completed);
        assert_eq!(effects, vec![Effect::PostReceipt]);
    }

    #[test]
    fn send_happy_path_confirms_then_completes() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        let effects = s.reduce(ev(1, E::Confirming));
        assert_eq!(s.state, State::Confirming);
        assert_eq!(
            effects,
            vec![Effect::StartConfirmTimer, Effect::StartMailboxPoll]
        );
        let effects = s.reduce(ev(1, E::Completed { bytes: 100 }));
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
        assert!(
            s.reduce(ev(1, failed(SessionFailureCode::Cancelled)))
                .is_empty()
        );
        assert!(
            s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)))
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
        assert!(
            s.reduce(ev(1, failed(SessionFailureCode::Cancelled)))
                .is_empty()
        );
        assert_eq!(s.state, State::Cancelled);
    }

    /// THE third July regression: stale bytes from a finished attempt must not
    /// fake an Unconfirmed after a re-join times out.
    #[test]
    fn stale_bytes_cannot_fake_unconfirmed() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, E::Confirming));
        s.reduce(ev(1, E::Completed { bytes: 100 }));
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
            s.reduce(ev(1, E::Completed { bytes: 100 }));
            assert!(s.reduce(Input::Resume).is_empty(), "{direction:?}");
            assert_eq!(s.state, State::Completed);
            assert_eq!(s.attempt, 1);
        }
    }

    #[test]
    fn confirming_connection_lost_escalates_to_mailbox() {
        let mut s = transferring(Send);
        s.reduce(ev(1, E::Progress { bytes: 100 }));
        s.reduce(ev(1, E::Confirming));
        let effects = s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)));
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
        s.reduce(ev(1, E::Confirming));
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
        s.reduce(ev(1, E::Confirming));
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
        assert!(s.reduce(ev(1, E::Completed { bytes: 100 })).is_empty());
        assert!(s.reduce(ev(1, E::RunEnded { failure: None })).is_empty());
        assert_eq!(s.state, State::Completed);
    }

    #[test]
    fn peer_pause_and_lost_connection_classify_as_paused() {
        let mut s = transferring(Receive);
        s.reduce(ev(1, failed(SessionFailureCode::PeerPaused)));
        assert_eq!(s.state, State::Paused(PauseOrigin::Peer));

        let mut s = transferring(Receive);
        s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Paused(PauseOrigin::Lost));

        // No progress on disk: a lost connection is a plain failure.
        let mut s = Session::new(Receive);
        s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)));
        assert_eq!(s.state, State::Failed);
    }

    /// D1: discard fires ONLY on the explicit typed peer cancel.
    #[test]
    fn peer_cancel_discards_and_restart_is_fresh() {
        let mut s = transferring(Receive);
        let effects = s.reduce(ev(1, failed(SessionFailureCode::PeerCancelled)));
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
        s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)));
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
        s.reduce(ev(1, E::Completed { bytes: 77 }));
        assert_eq!(s.state, State::Completed);
        assert_eq!((s.bytes, s.total), (77, 77));
    }

    #[test]
    fn verifying_returns_to_connecting() {
        let mut s = Session::new(Receive);
        s.reduce(ev(1, E::Verifying));
        assert_eq!(s.state, State::Verifying);
        s.reduce(ev(1, E::Verified));
        assert_eq!(s.state, State::Connecting);
        // Completed straight from Verifying is also legal (existing-final path).
        let mut s = Session::new(Receive);
        s.reduce(ev(1, E::Verifying));
        s.reduce(ev(1, E::Completed { bytes: 10 }));
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
                failure: Some((SessionFailureCode::ConnectionLost, "gone".into())),
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
        s.reduce(ev(1, failed(SessionFailureCode::ConnectionLost)));
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
            E::Verifying,
            E::Verified,
            E::Confirming,
            E::Completed { bytes: 999 },
            failed(SessionFailureCode::PeerCancelled),
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
                ev(attempt, E::Confirming),
                ev(attempt, E::Completed { bytes: 100 }),
                ev(attempt, failed(SessionFailureCode::ConnectionLost)),
                ev(attempt, failed(SessionFailureCode::PeerCancelled)),
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
}
