use std::num::NonZeroUsize;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptSupervisor, EventAdmission, OpenResult,
    ResumeIntent, RetirementIntent,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{ByteCount, Direction, OfferedName};

use crate::{
    CapabilityAction, CommitError, CommitFailure, CommitStatus, CommittedSession, IdentityError,
    IdentitySource, NewTransfer, ProductCommand, ProductEffect, ProductInput, ProductState,
    RecordDecode, RecordStore, SourceDecision, StorageAction, decode_record,
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

fn attempts(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("test attempt bound is nonzero")
}

fn transfer(direction: Direction) -> NewTransfer {
    NewTransfer {
        direction,
        offered_name: OfferedName::from_untrusted("barrier.bin"),
        total: ByteCount::new(100),
        source: SourceDecision::Ready,
    }
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

#[test]
fn failed_revision_dispatches_no_effect() {
    let (failed, outcome) = CommittedSession::create(
        transfer(Direction::Send),
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
        transfer(Direction::Send),
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
        transfer(Direction::Send),
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
    let (mut session, _) = CommittedSession::create(
        transfer(Direction::Send),
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
    session.store_mut().fail = true;
    let stamp = session.record().stamp();
    let input = admitted_event(
        session.record(),
        AttemptEventKind::Terminal(OutcomeCode::Cancelled),
    );

    let outcome = session.apply(input).unwrap();

    assert_eq!(session.record().state, ProductState::Failed);
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
        transfer(Direction::Send),
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
    let (session, outcome) = CommittedSession::create_without_store(
        transfer(Direction::Send),
        &mut DeterministicEntropy::default(),
    )
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
        transfer(Direction::Send),
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
fn escalation_makes_one_best_effort_failed_state_commit() {
    let (session, outcome) = CommittedSession::create(
        transfer(Direction::Send),
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
    assert!(outcome.released_after_commit.is_empty());
    assert_eq!(session.store().calls, 3);
    assert!(matches!(
        decode_record(&session.store().revisions[0]).unwrap(),
        RecordDecode::Loaded(record)
            if record.state == ProductState::Failed
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
