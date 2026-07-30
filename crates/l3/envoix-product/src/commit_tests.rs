use std::num::NonZeroUsize;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptSupervisor, CommitPointResult,
    EventAdmission, OpenResult, ResumeIntent, RetirementAck, RetirementAckResult, RetirementIntent,
    RetirementRequestResult, TerminalResolutionResult,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{ArtifactId, ByteCount, Direction, OfferedName, TransferId};

use crate::test_support::{STAGED_NAME, STAGED_TOTAL, acquired, offer, settled, staged};
use crate::{
    CapabilityAction, CommitError, CommitFailure, CommitStatus, CommittedSession, IdentityError,
    IdentitySource, NewTransfer, ProductCommand, ProductEffect, ProductInput, ProductState,
    RecordDecode, RecordStore, StorageAction, decode_record,
};

#[derive(Default)]
struct DeterministicEntropy {
    next: u8,
}

impl IdentitySource for DeterministicEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        for byte in destination {
            self.next = self.next.wrapping_add(1);
            *byte = self.next;
        }
        Ok(())
    }
}

#[derive(Default)]
struct MemoryStore {
    calls: usize,
    revisions: Vec<Vec<u8>>,
    fail: bool,
}

impl RecordStore for MemoryStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        self.calls += 1;
        if self.fail {
            return Err(CommitError);
        }
        if self.revisions.last().is_none_or(|last| last != encoded) {
            self.revisions.push(encoded.to_vec());
        }
        Ok(())
    }
}

#[derive(Default)]
struct AlwaysFailStore {
    calls: usize,
}

impl RecordStore for AlwaysFailStore {
    fn commit(&mut self, _encoded: &[u8]) -> Result<(), CommitError> {
        self.calls += 1;
        Err(CommitError)
    }
}

#[derive(Default)]
struct AmbiguousFirstWriteStore {
    calls: usize,
    revisions: Vec<Vec<u8>>,
}

struct FailThenStore {
    failures_remaining: usize,
    calls: usize,
    revisions: Vec<Vec<u8>>,
}

impl RecordStore for FailThenStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        self.calls += 1;
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            return Err(CommitError);
        }
        self.revisions.push(encoded.to_vec());
        Ok(())
    }
}

impl RecordStore for AmbiguousFirstWriteStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        self.calls += 1;
        if self.revisions.last().is_none_or(|last| last != encoded) {
            self.revisions.push(encoded.to_vec());
        }
        if self.calls == 1 {
            Err(CommitError)
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct FailAtCallStore {
    fail_on: usize,
    calls: usize,
}

impl RecordStore for FailAtCallStore {
    fn commit(&mut self, _encoded: &[u8]) -> Result<(), CommitError> {
        self.calls += 1;
        if self.calls == self.fail_on {
            Err(CommitError)
        } else {
            Ok(())
        }
    }
}

fn attempts(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("test attempt bound is nonzero")
}

/// A card as a frontend states it: a direction, how it joined the room, and
/// nothing about a document. A sender still needs one; the difference is that
/// the authority now decides that from the direction rather than believing a
/// caller's `SourceDecision`.
fn transfer(direction: Direction) -> NewTransfer {
    NewTransfer {
        direction,
        participation: crate::RoomParticipation::Minted,
        pairing: None,
    }
}

/// A card that starts an ATTEMPT as soon as it is created. That is now a
/// property of RECEIVING: a sender must be given a document first, so it opens
/// in `Preparing` instead. The barrier tests that use this are about commit
/// ordering and release timing, not about which way the bytes go.
fn attempting() -> NewTransfer {
    transfer(Direction::Receive)
}

/// Walks a sending session all the way to a live attempt through the real
/// acquisition: the picker answers, the platform acquires, staging reports what
/// it read, and the staging worker retires. There is no shorter route now, and
/// the tests that need a SENDER on the wire have to take it.
fn stage_the_source(session: &mut CommittedSession<MemoryStore>) {
    let offered = ProductInput::SourceOffered {
        offer: offer(session.record(), STAGED_NAME, None),
    };
    session.apply(offered).unwrap();
    let settlement = settled(session.record(), acquired());
    session.apply(settlement).unwrap();
    let stamp = session.record().stamp();
    session
        .apply(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, STAGED_TOTAL),
        })
        .unwrap();
    session
        .apply(ProductInput::StagingRetired { stamp })
        .unwrap();
}

#[test]
fn staging_completes_through_the_commit_barrier() {
    // Topology F1: the composed staging path (`StageComplete` → `Preparing +
    // source_ready + Retiring(Staging)`) must commit through the REAL store; the
    // codec previously rejected the handoff, forcing a spurious `StorageFault`
    // and never reaching `StartAttempt`.
    let (mut session, create_outcome) = CommittedSession::create(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    assert!(create_outcome.commit.authorizing_commit_succeeded());

    // The card is given a document and the platform acquires it, each through
    // the same barrier — a sender cannot stage bytes it was never handed.
    let offered = ProductInput::SourceOffered {
        offer: offer(session.record(), STAGED_NAME, None),
    };
    session.apply(offered).unwrap();
    let settlement = settled(session.record(), acquired());
    session.apply(settlement).unwrap();

    let stamp = session.record().stamp();
    let staged = session
        .apply(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 90),
        })
        .unwrap();
    assert!(
        matches!(staged.commit, CommitStatus::Committed { .. }),
        "the staging handoff must commit, not escalate: {:?}",
        staged.commit
    );
    assert_eq!(session.record().state, ProductState::Preparing);
    assert!(session.record().source_is_ready());

    // `StagingRetired` then launches the first attempt through the barrier.
    let launched = session
        .apply(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert!(matches!(launched.commit, CommitStatus::Committed { .. }));
    assert_eq!(session.record().state, ProductState::Connecting);
    assert!(
        launched
            .released_after_commit
            .iter()
            .any(|effect| matches!(effect, ProductEffect::StartAttempt { .. }))
    );
}

#[test]
fn adopted_identity_creation_crosses_the_commit_barrier() {
    let transfer_id = TransferId::from_bytes([0xc3; 16]);
    let artifact_id = ArtifactId::from_bytes([0xd4; 16]);
    let (session, outcome) = CommittedSession::create_with_identity(
        transfer(Direction::Receive),
        transfer_id,
        artifact_id,
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();

    assert_eq!(outcome.commit, CommitStatus::Committed { attempts: 1 });
    assert_eq!(session.record().identity.transfer, transfer_id);
    assert_eq!(session.record().identity.artifact, artifact_id);
    assert!(matches!(
        outcome.released_after_commit.as_slice(),
        [ProductEffect::StartAttempt { plan }]
            if plan.transfer == transfer_id && plan.artifact == artifact_id
    ));
}

fn admitted_event(record: &crate::TransferRecord, kind: AttemptEventKind) -> ProductInput {
    let plan = AttemptPlan {
        stamp: record.stamp(),
        direction: record.direction,
        transfer: record.identity.transfer,
        artifact: record.identity.artifact,
        resume: ResumeIntent::Fresh,
    };
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    match supervisor.observe(AttemptEvent {
        stamp: record.stamp(),
        kind,
    }) {
        EventAdmission::Accepted(event) => ProductInput::AttemptObserved(event),
        other => panic!("test event was not admitted: {other:?}"),
    }
}

fn completed_ack(record: &crate::TransferRecord) -> RetirementAck {
    let plan = AttemptPlan {
        stamp: record.stamp(),
        direction: record.direction,
        transfer: record.identity.transfer,
        artifact: record.identity.artifact,
        resume: ResumeIntent::Fresh,
    };
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(
        supervisor.cross_commit_point(plan.stamp),
        CommitPointResult::Crossed
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) = supervisor.acknowledge_retirement(plan.stamp)
    else {
        panic!("completed retirement must be acknowledgeable");
    };
    ack
}

fn cancelled_finalize_ack(record: &crate::TransferRecord) -> RetirementAck {
    let plan = AttemptPlan {
        stamp: record.stamp(),
        direction: record.direction,
        transfer: record.identity.transfer,
        artifact: record.identity.artifact,
        resume: ResumeIntent::Fresh,
    };
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(
        supervisor.resolve_terminal(plan.stamp, OutcomeCode::Cancelled),
        TerminalResolutionResult::Recorded
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) = supervisor.acknowledge_retirement(plan.stamp)
    else {
        panic!("cancelled retirement must be acknowledgeable");
    };
    ack
}

fn completed_finalize_ack(record: &crate::TransferRecord) -> RetirementAck {
    let plan = AttemptPlan {
        stamp: record.stamp(),
        direction: record.direction,
        transfer: record.identity.transfer,
        artifact: record.identity.artifact,
        resume: ResumeIntent::Fresh,
    };
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(
        supervisor.resolve_terminal(plan.stamp, OutcomeCode::Completed),
        TerminalResolutionResult::Recorded
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) = supervisor.acknowledge_retirement(plan.stamp)
    else {
        panic!("completed retirement must be acknowledgeable");
    };
    ack
}

#[test]
fn escalation_releases_the_receipt_when_the_failed_state_write_succeeds() {
    // Topology #3: a completed receive whose ack authorizing-commit fails once but
    // whose best-effort failed-state write SUCCEEDS must still release its
    // PostReceipt — the retained, durably-written Completed record authorizes it.
    // (create = commit 1, terminal = commit 2, ack authorizing = 3 [fails],
    // best-effort = 4 [succeeds].)
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Receive),
        &mut DeterministicEntropy::default(),
        FailAtCallStore {
            fail_on: 3,
            calls: 0,
        },
        attempts(1),
    )
    .unwrap();
    session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(session.record().state, ProductState::Completed);

    let ack = completed_finalize_ack(session.record());
    let outcome = session.apply(ProductInput::AttemptRetired(ack)).unwrap();

    assert!(matches!(
        outcome.commit,
        CommitStatus::Escalated {
            failed_state_persisted: true,
            ..
        }
    ));
    assert_eq!(session.record().state, ProductState::Completed);
    assert!(
        outcome.released_after_commit.iter().any(|effect| matches!(
            effect,
            ProductEffect::CapabilityDuty {
                action: CapabilityAction::PostReceipt,
                ..
            }
        )),
        "the receipt duty is released once its Completed record is durably written"
    );
}

#[test]
fn restore_replay_survives_a_failed_authorizing_commit() {
    // A Restore that replays an undelivered receive receipt must still release it
    // when the authorizing commit fails once but the best-effort write of the
    // UNCHANGED Completed record succeeds — the durable record still authorizes
    // the receipt (keying the release on `monotone_completion`/`worker_gone_proven`
    // alone would strand it, since a restore of a quiescent card is neither).
    let (mut setup, _) = CommittedSession::create(
        transfer(Direction::Receive),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    setup
        .apply(admitted_event(
            setup.record(),
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    setup
        .apply(ProductInput::AttemptRetired(completed_finalize_ack(
            setup.record(),
        )))
        .unwrap();
    assert_eq!(setup.record().state, ProductState::Completed);
    assert!(!setup.record().facts.proof_delivered);
    let (record, _) = setup.into_parts();

    // The receipt was never confirmed; a restart re-issues it, but the authorizing
    // commit fails once before the best-effort write succeeds.
    let mut session = CommittedSession::from_record(
        record,
        FailAtCallStore {
            fail_on: 1,
            calls: 0,
        },
        attempts(1),
    );
    let outcome = session.apply(ProductInput::Restore).unwrap();

    assert!(matches!(
        outcome.commit,
        CommitStatus::Escalated {
            failed_state_persisted: true,
            ..
        }
    ));
    assert!(
        outcome.released_after_commit.iter().any(|effect| matches!(
            effect,
            ProductEffect::CapabilityDuty {
                action: CapabilityAction::PostReceipt,
                ..
            }
        )),
        "the replayed receipt is released once its unchanged record is durably written"
    );
}

#[test]
fn failed_revision_dispatches_no_effect() {
    let (failed, outcome) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        AlwaysFailStore::default(),
        attempts(2),
    )
    .unwrap();

    assert_eq!(failed.record().state, ProductState::Failed);
    assert_eq!(
        failed.record().outcome.as_ref().map(|outcome| outcome.code),
        Some(OutcomeCode::StorageFault)
    );
    assert!(outcome.released_after_commit.is_empty());
    assert!(
        !outcome
            .released_immediately
            .iter()
            .any(|effect| matches!(effect, ProductEffect::StartAttempt { .. }))
    );
    assert_eq!(
        outcome.commit,
        CommitStatus::Escalated {
            attempts: 2,
            failure: CommitFailure::Store(CommitError),
            failed_state_persisted: false,
        }
    );
    assert_eq!(
        failed.store().calls,
        3,
        "two bounded authorizing attempts plus one best-effort failed-state write"
    );

    let (healthy, outcome) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(2),
    )
    .unwrap();
    assert_eq!(outcome.commit, CommitStatus::Committed { attempts: 1 });
    assert!(outcome.released_immediately.is_empty());
    assert!(matches!(
        outcome.released_after_commit.as_slice(),
        [ProductEffect::StartAttempt { .. }]
    ));
    assert_eq!(healthy.store().calls, 1);
    assert_eq!(healthy.store().revisions.len(), 1);
    assert!(matches!(
        decode_record(&healthy.store().revisions[0]).unwrap(),
        RecordDecode::Loaded(record) if record.state == ProductState::Connecting
    ));
}

#[test]
fn immediate_retirement_does_not_wait_for_a_writable_store() {
    let (mut session, _) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(2),
    )
    .unwrap();
    session.store_mut().fail = true;
    let stamp = session.record().stamp();

    let outcome = session
        .apply(ProductInput::Command(ProductCommand::Pause))
        .unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
    assert_eq!(
        outcome.released_immediately,
        vec![
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Pause,
            },
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            },
        ]
    );
    assert!(outcome.released_after_commit.is_empty());
    assert!(matches!(
        outcome.commit,
        CommitStatus::Escalated {
            attempts: 2,
            failed_state_persisted: false,
            ..
        }
    ));
}

#[test]
fn timer_bookkeeping_releases_and_is_unwound_on_escalation() {
    // A SENDER: the mailbox poll below is the sender's confirm-wait, so this
    // one cannot borrow a receiver's shortcut to the wire.
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    stage_the_source(&mut session);
    session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    session.store_mut().fail = true;
    let stamp = session.record().stamp();

    let outcome = session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Phase(Phase::Confirming),
        ))
        .unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
    assert_eq!(
        outcome.released_immediately,
        vec![
            ProductEffect::StartConfirmTimer { stamp },
            ProductEffect::StartMailboxPoll { stamp },
            ProductEffect::StopConfirmTimer { stamp },
            ProductEffect::StopMailboxPoll { stamp },
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            },
        ]
    );
    assert!(outcome.released_after_commit.is_empty());
}

#[test]
fn never_committed_mixed_batch_drops_destructive_storage_intent() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Receive),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(2),
    )
    .unwrap();
    let stamp = session.record().stamp();
    let input = admitted_event(
        session.record(),
        AttemptEventKind::Terminal(OutcomeCode::Cancelled),
    );
    let terminal = session.apply(input).unwrap();
    assert_eq!(terminal.commit, CommitStatus::Committed { attempts: 1 });
    assert_eq!(
        terminal.released_immediately,
        vec![ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Finalize,
        }]
    );
    assert!(terminal.released_after_commit.is_empty());

    let ack = cancelled_finalize_ack(session.record());
    session.store_mut().fail = true;
    let outcome = session.apply(ProductInput::AttemptRetired(ack)).unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
    assert!(outcome.released_after_commit.is_empty());
    assert!(outcome.released_immediately.is_empty());
    assert!(!outcome.released_immediately.iter().any(|effect| matches!(
        effect,
        ProductEffect::StorageIntent {
            action: StorageAction::DiscardPartial,
            ..
        }
    )));
    assert!(matches!(
        outcome.commit,
        CommitStatus::Escalated { attempts: 2, .. }
    ));
}

#[test]
fn consumed_retirement_ack_survives_a_failed_authorizing_commit() {
    let (mut session, created) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        FailThenStore {
            failures_remaining: 0,
            calls: 0,
            revisions: Vec::new(),
        },
        attempts(1),
    )
    .unwrap();
    let plan = created
        .released_after_commit
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::StartAttempt { plan } => Some(*plan),
            _ => None,
        })
        .expect("created attempt plan");
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);

    session
        .apply(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Pause),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) = supervisor.acknowledge_retirement(plan.stamp)
    else {
        panic!("paused attempt must acknowledge retirement");
    };

    session.store_mut().failures_remaining = 1;
    let outcome = session.apply(ProductInput::AttemptRetired(ack)).unwrap();

    assert!(matches!(
        outcome.commit,
        CommitStatus::Escalated {
            attempts: 1,
            failed_state_persisted: true,
            ..
        }
    ));
    assert_eq!(session.record().state, ProductState::Failed);
    assert_eq!(session.record().quiescence, crate::Quiescence::Quiescent);
    assert_eq!(
        session
            .record()
            .outcome
            .as_ref()
            .map(|outcome| outcome.code),
        Some(OutcomeCode::StorageFault)
    );
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::AlreadyAcknowledged
    );
    assert_eq!(
        session.record().allowed_commands(),
        vec![ProductCommand::Resume, ProductCommand::Remove]
    );
    assert!(matches!(
        decode_record(session.store().revisions.last().unwrap()).unwrap(),
        RecordDecode::Loaded(record)
            if record.state == ProductState::Failed
                && record.quiescence == crate::Quiescence::Quiescent
    ));
}

#[test]
fn uncommitted_completion_rolls_back_before_visible_storage_failure() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Receive),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    session.store_mut().fail = true;
    let stamp = session.record().stamp();

    let outcome = session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
    assert_eq!(outcome.state, ProductState::Failed);
    assert!(outcome.released_after_commit.is_empty());
    assert!(
        outcome
            .released_immediately
            .contains(&ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Finalize,
            })
    );
    assert!(
        outcome
            .released_immediately
            .contains(&ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            })
    );
    assert!(
        !outcome
            .released_immediately
            .iter()
            .any(|effect| matches!(effect, ProductEffect::CapabilityDuty { .. }))
    );
}

#[test]
fn uncommitted_remove_cannot_fence_off_storage_failure() {
    let (mut session, _) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    session.store_mut().fail = true;
    let stamp = session.record().stamp();

    let outcome = session
        .apply(ProductInput::Command(ProductCommand::Remove))
        .unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
    assert!(!session.record().facts.remove_requested);
    assert_eq!(
        outcome.released_immediately,
        vec![ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Cancel,
        }]
    );
    assert!(outcome.released_after_commit.is_empty());
    assert!(!outcome.released_immediately.iter().any(|effect| matches!(
        effect,
        ProductEffect::StorageIntent {
            action: StorageAction::TombstoneCard,
            ..
        }
    )));
}

#[test]
fn receipt_duty_releases_only_after_completed_record_commits() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Receive),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    let stamp = session.record().stamp();
    session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    let outcome = session
        .apply(admitted_event(
            session.record(),
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();

    assert_eq!(outcome.commit, CommitStatus::Committed { attempts: 1 });
    assert_eq!(
        outcome.released_immediately,
        vec![ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Finalize,
        }]
    );
    assert!(outcome.released_after_commit.is_empty());

    let ack = completed_ack(session.record());
    let outcome = session.apply(ProductInput::AttemptRetired(ack)).unwrap();
    assert_eq!(outcome.commit, CommitStatus::Committed { attempts: 1 });
    assert!(outcome.released_immediately.is_empty());
    assert!(matches!(
        outcome.released_after_commit.as_slice(),
        [ProductEffect::CapabilityDuty {
            action: CapabilityAction::PostReceipt,
            ..
        }]
    ));
}

#[test]
fn no_store_makes_the_barrier_vacuous() {
    let (session, outcome) =
        CommittedSession::create_without_store(attempting(), &mut DeterministicEntropy::default())
            .unwrap();

    assert_eq!(session.record().state, ProductState::Connecting);
    assert_eq!(outcome.commit, CommitStatus::Vacuous);
    assert!(matches!(
        outcome.released_after_commit.as_slice(),
        [ProductEffect::StartAttempt { .. }]
    ));
}

#[test]
fn ambiguous_retry_is_idempotent_and_releases_once() {
    let (session, outcome) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        AmbiguousFirstWriteStore::default(),
        attempts(2),
    )
    .unwrap();

    assert_eq!(outcome.commit, CommitStatus::Committed { attempts: 2 });
    assert_eq!(session.store().calls, 2);
    assert_eq!(
        session.store().revisions.len(),
        1,
        "repeated identical bytes are one durable revision"
    );
    assert_eq!(
        outcome
            .released_after_commit
            .iter()
            .filter(|effect| matches!(effect, ProductEffect::StartAttempt { .. }))
            .count(),
        1
    );
}

#[test]
fn create_time_commit_failure_does_not_retire_an_unopened_attempt() {
    let (session, outcome) = CommittedSession::create(
        attempting(),
        &mut DeterministicEntropy::default(),
        FailThenStore {
            failures_remaining: 2,
            calls: 0,
            revisions: Vec::new(),
        },
        attempts(2),
    )
    .unwrap();

    assert_eq!(
        outcome.commit,
        CommitStatus::Escalated {
            attempts: 2,
            failure: CommitFailure::Store(CommitError),
            failed_state_persisted: true,
        }
    );
    assert!(outcome.released_immediately.is_empty());
    assert!(outcome.released_after_commit.is_empty());
    assert_eq!(session.record().quiescence, crate::Quiescence::Quiescent);
    assert_eq!(
        session.record().allowed_commands(),
        vec![ProductCommand::Resume, ProductCommand::Remove]
    );
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(
        supervisor.request_retirement(session.record().stamp(), RetirementIntent::Cancel),
        RetirementRequestResult::Unknown
    );
    assert_eq!(session.store().calls, 3);
    assert!(matches!(
        decode_record(&session.store().revisions[0]).unwrap(),
        RecordDecode::Loaded(record)
            if record.state == ProductState::Failed
                && record.quiescence == crate::Quiescence::Quiescent
                && record
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.code == OutcomeCode::StorageFault)
    ));
}

#[test]
fn ignored_input_does_not_write_or_release_again() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    session
        .apply(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    let calls = session.store().calls;

    let outcome = session
        .apply(ProductInput::Command(ProductCommand::Pause))
        .unwrap();

    assert_eq!(outcome.commit, CommitStatus::NotRequired);
    assert!(outcome.released_immediately.is_empty());
    assert!(outcome.released_after_commit.is_empty());
    assert_eq!(session.store().calls, calls);
}

// ---- the source-offer edge ----

/// A minted send asks for a document, and the offer that names its exact
/// acquisition binds. The expected key is DERIVED by the authority — card,
/// generation and the source request this record mints — so a frontend cannot
/// name an acquisition the authority is not asking for.
#[test]
fn an_offer_for_the_asked_acquisition_binds_the_document() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    assert!(matches!(
        session.record().source,
        crate::SourceLifecycle::AwaitingSelection(_)
    ));

    let offered = crate::AcceptedSourceOffer::new(
        expected_key(session.record()),
        OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
        Some(ByteCount::new(4096)),
    );
    session
        .apply(ProductInput::SourceOffered {
            offer: offered.clone(),
        })
        .expect("the offer applies");

    let crate::SourceLifecycle::Acquiring(bound) = &session.record().source else {
        panic!("an accepted offer moves the card to acquiring");
    };
    assert_eq!(bound, &offered);
    // And the name the card will show comes from the offer, not from create.
    assert_eq!(bound.display_name().as_str(), "report.pdf");
}

/// An offer naming a different acquisition changes NOTHING. It is the caller's
/// to be told about — the authority answers it — but the record does not absorb
/// a document it never asked for, which is the ownership defect this whole
/// design exists to close.
#[test]
fn an_offer_for_another_acquisition_leaves_the_record_alone() {
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
        MemoryStore::default(),
        attempts(1),
    )
    .unwrap();
    let before = session.record().source.clone();

    let wrong = crate::AcceptedSourceOffer::new(
        envoix_capabilities::SourceAcquisitionKey::of(envoix_capabilities::DutyProvenance {
            card: envoix_types::RecordId::new(0xdead),
            generation: session.record().generation,
            request: envoix_types::RequestId::from_bytes([9; 16]),
        }),
        OfferedName::from_untrusted("someone-elses.pdf").expect("a bounded name"),
        None,
    );
    session
        .apply(ProductInput::SourceOffered { offer: wrong })
        .expect("a refused offer is not an error");
    assert_eq!(session.record().source, before);
}

fn expected_key(record: &crate::TransferRecord) -> envoix_capabilities::SourceAcquisitionKey {
    let crate::SourceLifecycle::AwaitingSelection(_) = &record.source else {
        panic!("only an awaiting card has an acquisition to name");
    };
    // Mirrors the authority's own derivation.
    envoix_capabilities::SourceAcquisitionKey::of(envoix_capabilities::DutyProvenance {
        card: record.identity.card,
        generation: record.generation,
        request: record.source_request(),
    })
}
