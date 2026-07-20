use super::super::machine::{AttemptEvent, Input, State};
use super::*;
use envoix_storage::{LocalFileStorage, TransferReceipt};

fn failing_params(direction: TransferDirection) -> SessionParams {
    SessionParams {
        direction,
        path: "nonexistent.bin".into(),
        sources: vec![PeerSource::Invite {
            invite: "not-an-invite".into(),
        }],
        options: TransferOptions::default(),
        publication_required: false,
    }
}

fn failing_context(direction: TransferDirection) -> SessionContext {
    SessionContext {
        client: ClientContext::default(),
        params: failing_params(direction),
    }
}

fn record(direction: TransferDirection, session: Session) -> TransferRecord {
    TransferRecord {
        version: super::super::record::RECORD_VERSION,
        id: 7,
        created_ms: 1,
        updated_ms: 1,
        context: failing_context(direction),
        session,
        platform_extras: None,
    }
}

fn actor_for_context(
    context: SessionContext,
) -> (Actor, mpsc::UnboundedReceiver<SessionNotice>) {
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    drop(cmds);
    let (notice_tx, notice_rx) = mpsc::unbounded_channel();
    (
        Actor {
            client: Client::new(),
            session: Session::new(context.params.direction),
            context,
            cmds: cmd_rx,
            notices: notice_tx,
            current: None,
            pending: None,
            pending_failure: None,
            seq: 0,
            apply_seq: 0,
            confirm_deadline: None,
            polls: Vec::new(),
            poll_key: None,
            rate: RateTracker::default(),
            last_progress_snapshot: None,
            created_ms: 1,
            record: None,
            platform_extras: None,
            staged: Vec::new(),
            commit_failures: 0,
            commit_retry_at: None,
            launch: false,
        },
        notice_rx,
    )
}

/// Drain notices until a snapshot in the wanted state arrives.
async fn wait_for_state(
    notices: &mut mpsc::UnboundedReceiver<SessionNotice>,
    wanted: State,
) -> SessionSnapshot {
    let mut last_seq = 0;
    loop {
        let notice = tokio::time::timeout(Duration::from_secs(10), notices.recv())
            .await
            .expect("notice within timeout")
            .expect("stream open");
        if let SessionNotice::Snapshot(snapshot) = notice {
            assert!(snapshot.seq > last_seq, "snapshot seq must be monotonic");
            last_seq = snapshot.seq;
            if snapshot.session.state == wanted {
                return snapshot;
            }
        }
    }
}

#[tokio::test]
async fn failing_attempt_reaches_failed_via_run_ended() {
    let (_session, mut notices) =
        TransferSession::start(failing_context(TransferDirection::Send), None, None).unwrap();
    let snapshot = wait_for_state(&mut notices, State::Failed).await;
    assert_eq!(snapshot.session.attempt, 1);
    assert!(snapshot.session.reason.is_some());
}

#[tokio::test]
async fn resume_launches_attempt_two() {
    let (session, mut notices) =
        TransferSession::start(failing_context(TransferDirection::Send), None, None).unwrap();
    wait_for_state(&mut notices, State::Failed).await;
    session.resume();
    let snapshot = wait_for_state(&mut notices, State::Connecting).await;
    assert_eq!(snapshot.session.attempt, 2);
    // …and the second attempt fails the same way.
    let snapshot = wait_for_state(&mut notices, State::Failed).await;
    assert_eq!(snapshot.session.attempt, 2);
}

#[tokio::test]
async fn restore_coerces_active_to_paused_lost() {
    let mut session = Session::new(TransferDirection::Receive);
    session.bytes = 40;
    assert_eq!(session.state, State::Connecting); // "active" when persisted
    let (_handle, mut notices) =
        TransferSession::restore(record(TransferDirection::Receive, session), None).unwrap();
    let snapshot = wait_for_state(
        &mut notices,
        State::Paused(super::super::machine::PauseOrigin::Lost),
    )
    .await;
    assert_eq!(snapshot.session.bytes, 40, "progress display survives");
    assert_eq!(snapshot.session.attempt, 1, "no attempt was launched");
}

/// The commit barrier under a dead store: no snapshot may leak an
/// uncommitted state, the attempt never launches, and after the bounded
/// retries the session fails VISIBLY (never a silent stall).
#[tokio::test(start_paused = true)]
async fn unwritable_store_escalates_to_a_visible_failure() {
    let root = std::env::temp_dir().join(format!("envoix-deadstore-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root).await.unwrap();
    // A FILE where the store wants a directory: every save fails.
    tokio::fs::write(root.join("blocker"), b"x").await.unwrap();
    let store = RecordStore::new(root.join("blocker").join("records"));

    let (_handle, mut notices) = TransferSession::start(
        failing_context(TransferDirection::Send),
        Some((store, 5)),
        None,
    )
    .unwrap();

    // The paused clock drives the retry backoff instantly; the FIRST
    // snapshot ever observed must already be the terminal escalation -
    // anything earlier would be a view of uncommitted state.
    let snapshot = loop {
        match notices.recv().await.expect("stream open") {
            SessionNotice::Snapshot(s) => break s,
            _ => continue,
        }
    };
    assert_eq!(snapshot.session.state, State::Failed);
    assert!(
        snapshot
            .session
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("record store"),
        "the failure names the store, not the transfer"
    );
    let _ = tokio::fs::remove_dir_all(&root).await;
}

#[tokio::test]
async fn start_staging_waits_in_preparing_and_persists_before_any_attempt() {
    let dir = std::env::temp_dir().join(format!("envoix-staging-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let store = RecordStore::new(&dir);
    let (_session, mut notices) = TransferSession::start_staging(
        failing_context(TransferDirection::Send),
        Some((store.clone(), 8)),
        None,
    )
    .unwrap();

    // The FIRST snapshot is Preparing - no attempt was launched (the
    // failing context would otherwise drive it straight to Failed).
    let snapshot = wait_for_state(&mut notices, State::Preparing).await;
    assert_eq!(snapshot.session.state, State::Preparing);
    // And the record committed as Preparing BEFORE the copy would start.
    let persisted = store.load(8).await.expect("record persisted");
    assert_eq!(persisted.session.state, State::Preparing);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn stage_progress_moves_the_bar_but_is_not_persisted() {
    let dir = std::env::temp_dir().join(format!("envoix-stageprog-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let store = RecordStore::new(&dir);
    let (session, mut notices) = TransferSession::start_staging(
        failing_context(TransferDirection::Send),
        Some((store.clone(), 12)),
        None,
    )
    .unwrap();
    wait_for_state(&mut notices, State::Preparing).await;

    session.stage_progress(1, 200);
    // The snapshot shows the staging bar move...
    let snapshot = loop {
        match notices.recv().await.expect("stream open") {
            SessionNotice::Snapshot(s) if s.session.bytes == 200 => break s,
            _ => continue,
        }
    };
    assert_eq!(snapshot.session.bytes, 200);
    // ...but the record is NOT rewritten for progress (would be churn).
    let persisted = store.load(12).await.expect("record exists");
    assert_eq!(
        persisted.session.bytes, 0,
        "staging progress is snapshot-only"
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn stage_complete_leaves_preparing_and_launches() {
    let (session, mut notices) =
        TransferSession::start_staging(failing_context(TransferDirection::Send), None, None)
            .unwrap();
    wait_for_state(&mut notices, State::Preparing).await;
    session.stage_complete(1);
    // The attempt launches (and fails, via the failing context) - the
    // point is it LEFT Preparing rather than staying stuck.
    wait_for_state(&mut notices, State::Failed).await;
}

#[tokio::test]
async fn stage_failed_fails_the_preparing_transfer() {
    let (session, mut notices) =
        TransferSession::start_staging(failing_context(TransferDirection::Send), None, None)
            .unwrap();
    wait_for_state(&mut notices, State::Preparing).await;
    session.stage_failed(1, "source could not be read".into());
    let snapshot = wait_for_state(&mut notices, State::Failed).await;
    assert_eq!(
        snapshot.session.reason.as_deref(),
        Some("source could not be read")
    );
}

#[tokio::test]
async fn restored_preparing_stays_preparing_for_the_platform() {
    let mut session = Session::new(TransferDirection::Send);
    session.state = State::Preparing;
    let (_handle, mut notices) =
        TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
    // Not coerced to Paused(Lost): the platform decides re-stage vs fail.
    let snapshot = wait_for_state(&mut notices, State::Preparing).await;
    assert_eq!(snapshot.session.state, State::Preparing);
}

#[tokio::test]
async fn restored_confirming_with_sent_hash_is_unconfirmed() {
    // sent_hash is recorded exactly at Confirming: every byte + the
    // Complete frame went out, only the ack died with the process. The
    // honest restored state is Unconfirmed (poll resumes), not Paused.
    let mut session = Session::new(TransferDirection::Send);
    session.state = State::Confirming;
    session.transfer_id = Some("transfer-confirming".into());
    session.sent_hash = Some("committed-hash".into());
    let (_handle, mut notices) =
        TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
    let snapshot = wait_for_state(&mut notices, State::Unconfirmed).await;
    assert_eq!(snapshot.session.state, State::Unconfirmed);

    // Without the committed fact (a pre-fact record), Paused(Lost) stands.
    let mut session = Session::new(TransferDirection::Send);
    session.state = State::Confirming;
    let (_handle, mut notices) =
        TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
    wait_for_state(
        &mut notices,
        State::Paused(super::super::machine::PauseOrigin::Lost),
    )
    .await;
}

#[tokio::test]
async fn sync_launch_failure_is_persisted() {
    // Client::run fails synchronously on empty sources; the failure must
    // take the same apply path as any input - the old inline shortcut
    // skipped persist(), so the Failed state vanished on restore.
    let dir = std::env::temp_dir().join(format!("envoix-launchfail-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let store = RecordStore::new(&dir);
    let mut context = failing_context(TransferDirection::Send);
    context.params.sources = Vec::new();
    let (_handle, mut notices) =
        TransferSession::start(context, Some((store.clone(), 3)), None).unwrap();
    wait_for_state(&mut notices, State::Failed).await;

    // Persist happens-before emit: by the time any snapshot is observed,
    // the record already holds that state. No polling.
    let persisted = store.load(3).await.expect("record exists");
    assert_eq!(
        persisted.session.state,
        State::Failed,
        "the record commits before the snapshot goes out"
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn restore_unconfirmed_resumes_the_mailbox_poll() {
    let mut session = Session::new(TransferDirection::Send);
    session.state = State::Unconfirmed;
    session.transfer_id = Some("transfer-restored".into());
    let (_handle, mut notices) =
        TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
    // Receipt confirmation must survive restarts: the restored session
    // re-derives its standing effect and asks the courier to fetch.
    loop {
        let notice = tokio::time::timeout(Duration::from_secs(20), notices.recv())
            .await
            .expect("courier request within the poll schedule")
            .expect("stream open");
        if let SessionNotice::FetchReceipt { key, .. } = notice {
            assert_eq!(key.len(), 64);
            break;
        }
    }
}

#[test]
fn restore_validates_persisted_client_context() {
    let mut record = record(
        TransferDirection::Receive,
        Session::new(TransferDirection::Receive),
    );
    record.context.client.chunk_size = Some("not-a-size".into());

    assert!(TransferSession::restore(record, None).is_err());
}

#[tokio::test]
async fn completed_without_started_still_posts_receipt() {
    let dir =
        std::env::temp_dir().join(format!("envoix-driver-receipt-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    LocalFileStorage::write_receipt(
        &dir,
        &TransferReceipt {
            transfer_id: envoix_types::TransferId::new("transfer-fast"),
            file_name: "done.bin".into(),
            file_size: 77,
            file_hash: "hash".into(),
        },
    )
    .await
    .unwrap();

    let mut context = failing_context(TransferDirection::Receive);
    context.params.path = dir.clone();
    context.params.sources = vec![PeerSource::Room {
        code: "123456-kelp-coral".into(),
        broker: "id@1.2.3.4:5".into(),
    }];
    let (mut actor, mut notices) = actor_for_context(context);

    actor
        .apply(Input::Event {
            attempt: 1,
            event: AttemptEvent::Completed {
                transfer_id: "transfer-fast".into(),
                file_name: "done.bin".into(),
                bytes: 77,
                completed_file_path: Some(dir.join("done.bin").to_string_lossy().into_owned()),
            },
        })
        .await;

    match notices.recv().await.expect("receipt posted") {
        SessionNotice::PostReceipt { key, blob, .. } => {
            assert_eq!(key.len(), 64);
            assert!(!blob.is_empty());
        }
        notice => panic!("expected receipt post, got {notice:?}"),
    }
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn stale_receipt_responses_are_dropped_by_key() {
    let mut context = failing_context(TransferDirection::Send);
    context.params.sources = vec![PeerSource::Room {
        code: "123456-kelp-coral".into(),
        broker: "id@1.2.3.4:5".into(),
    }];
    let (mut actor, _notices) = actor_for_context(context);
    actor.session.state = State::Unconfirmed;
    actor.session.transfer_id = Some("transfer-current".into());
    actor.session.sent_hash = Some("sent".into());
    let current_key = receipt::receipt_mailbox_key("transfer-current");
    actor.poll_key = Some(current_key.clone());
    actor.polls = vec![Instant::now() + Duration::from_secs(60)];

    // A late answer for a superseded attempt's slot changes nothing.
    actor
        .on_cmd(Cmd::ReceiptResponse {
            key: receipt::receipt_mailbox_key("transfer-previous"),
            blob: Some(vec![1, 2, 3]),
        })
        .await;
    assert_eq!(actor.polls.len(), 1, "stale response must not clear polls");

    // The current slot with a mismatching blob records the fact and keeps
    // the bounded polls alive: the receiver overwrites the slot if it
    // re-completes our offer, so a later poll can still verify.
    actor
        .on_cmd(Cmd::ReceiptResponse {
            key: current_key,
            blob: Some(vec![1, 2, 3]),
        })
        .await;
    assert!(
        actor.session.facts.receipt_mismatch,
        "the mismatch is a recorded machine fact"
    );
    assert_eq!(actor.polls.len(), 1, "polling continues after a mismatch");
}

#[tokio::test]
async fn cancel_wins_over_the_attempt() {
    let (session, mut notices) =
        TransferSession::start(failing_context(TransferDirection::Receive), None, None)
            .unwrap();
    session.cancel();
    let snapshot = wait_for_state(&mut notices, State::Cancelled).await;
    // Whatever the racing attempt reported, the user's cancel is final.
    assert_eq!(snapshot.session.state, State::Cancelled);
}
