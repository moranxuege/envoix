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
        completed_file_path: None,
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
fn staged_receive_completes_only_after_native_publication() {
    let mut session = Session::new(Receive);
    session.publication_required = true;
    session.reduce(ev(1, started()));
    let mut completed = completed(100);
    if let E::Completed {
        completed_file_path,
        ..
    } = &mut completed
    {
        *completed_file_path = Some("/private/staging/a.zip".into());
    }

    assert_eq!(session.reduce(ev(1, completed)), vec![Effect::PostReceipt]);
    assert_eq!(session.state, State::AwaitingPublication);
    assert_eq!(
        session.completed_file_path.as_deref(),
        Some("/private/staging/a.zip")
    );
    assert!(
        session
            .reduce(Input::Published {
                path: "file:///Downloads/a.zip".into(),
            })
            .is_empty()
    );
    assert_eq!(session.state, State::Completed);
    assert_eq!(
        session.completed_file_path.as_deref(),
        Some("file:///Downloads/a.zip")
    );
}

#[test]
fn canceling_staged_receive_discards_only_the_staged_final() {
    let mut session = transferring(Receive);
    session.publication_required = true;
    session.reduce(ev(1, completed(100)));

    assert_eq!(
        session.reduce(Input::Cancel),
        vec![Effect::DiscardStagedFile]
    );
    assert_eq!(session.state, State::Cancelled);
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
    assert_eq!((s.bytes, s.total, s.bytes_resumed), (77, 77, 77));
    assert_eq!(s.transfer_id.as_deref(), Some("transfer-t1"));
    assert_eq!(s.file_name.as_deref(), Some("a.zip"));
}

#[test]
fn completed_after_started_preserves_partial_resume_count() {
    let mut s = Session::new(Receive);
    s.reduce(ev(
        1,
        E::Started {
            transfer_id: "transfer-t1".into(),
            file_name: "a.zip".into(),
            total: 100,
            bytes_resumed: 40,
        },
    ));
    s.reduce(ev(1, completed(100)));

    assert_eq!(s.state, State::Completed);
    assert_eq!((s.bytes, s.total, s.bytes_resumed), (100, 100, 40));
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
