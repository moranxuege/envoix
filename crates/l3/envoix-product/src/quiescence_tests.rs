use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptSupervisor, CommitPointResult,
    EventAdmission, OpenResult, RetirementAck, RetirementAckResult, RetirementIntent,
    RetirementRequestResult,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{AttemptGen, ByteCount, Direction};

use crate::test_support::{STAGED_NAME, STAGED_TOTAL, give_a_source, staged};
use crate::{
    CapabilityAction, IdentityError, IdentitySource, NewTransfer, ProductCommand, ProductEffect,
    ProductInput, ProductState, Quiescence, StorageAction, TransferRecord, WorkerKind,
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

fn create(direction: Direction) -> (TransferRecord, Vec<ProductEffect>) {
    TransferRecord::create(
        NewTransfer {
            direction,
            participation: crate::RoomParticipation::Minted,
            pairing: None,
        },
        &mut DeterministicEntropy::default(),
    )
    .expect("deterministic identity source")
}

/// A card whose source is established, walked through the real acquisition.
/// A receiver needs none; a sender is given a document, the platform acquires
/// it, staging reports what it read, and the staging worker retires.
fn ready(direction: Direction) -> (TransferRecord, Vec<ProductEffect>) {
    let (mut record, effects) = create(direction);
    if direction == Direction::Receive {
        return (record, effects);
    }
    give_a_source(&mut record);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, STAGED_TOTAL),
        })
        .unwrap();
    let launched = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    (record, launched)
}

fn open(record: &TransferRecord, effects: &[ProductEffect]) -> (AttemptSupervisor, AttemptPlan) {
    let plan = effects
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::StartAttempt { plan } => Some(*plan),
            _ => None,
        })
        .expect("attempt plan");
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(plan.stamp, record.stamp());
    (supervisor, plan)
}

fn observe(
    supervisor: &AttemptSupervisor,
    record: &mut TransferRecord,
    kind: AttemptEventKind,
) -> Vec<ProductEffect> {
    let event = AttemptEvent {
        stamp: record.stamp(),
        kind,
    };
    let EventAdmission::Accepted(event) = supervisor.observe(event) else {
        panic!("current live event must be admitted");
    };
    record
        .reduce(ProductInput::AttemptObserved(event))
        .expect("reduction")
}

fn acknowledged(result: RetirementAckResult) -> RetirementAck {
    let RetirementAckResult::Acknowledged(ack) = result else {
        panic!("expected retirement acknowledgement");
    };
    ack
}

fn transfer(
    direction: Direction,
) -> (
    TransferRecord,
    AttemptSupervisor,
    AttemptPlan,
    Vec<ProductEffect>,
) {
    let (mut record, create_effects) = ready(direction);
    let (supervisor, plan) = open(&record, &create_effects);
    assert!(
        observe(
            &supervisor,
            &mut record,
            AttemptEventKind::Phase(Phase::Transferring),
        )
        .is_empty()
    );
    assert!(
        observe(
            &supervisor,
            &mut record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(40),
            },
        )
        .is_empty()
    );
    (record, supervisor, plan, create_effects)
}

fn has_discard(effects: &[ProductEffect]) -> bool {
    effects.iter().any(|effect| {
        matches!(
            effect,
            ProductEffect::StorageIntent {
                action: StorageAction::DiscardPartial,
                ..
            }
        )
    })
}

fn has_start(effects: &[ProductEffect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, ProductEffect::StartAttempt { .. }))
}

#[test]
fn pause_cancel_finalize_linearization() {
    // Cancel wins before the irreversible commit point.
    let (mut cancelled, mut cancel_supervisor, cancel_plan, _) = transfer(Direction::Receive);
    observe(
        &cancel_supervisor,
        &mut cancelled,
        AttemptEventKind::Progress {
            transferred: ByteCount::new(75),
        },
    );
    let cancel_effects = cancelled
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(cancelled.state, ProductState::Cancelled);
    assert_eq!(cancelled.bytes, ByteCount::new(75));
    assert_eq!(
        cancelled.quiescence,
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Cancel,
        }
    );
    assert!(!has_discard(&cancel_effects));
    assert_eq!(
        cancel_effects,
        vec![ProductEffect::RetireAttempt {
            stamp: cancel_plan.stamp,
            intent: RetirementIntent::Cancel,
        }]
    );
    assert_eq!(
        cancel_supervisor.request_retirement(cancel_plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    assert_eq!(
        cancel_supervisor.cross_commit_point(cancel_plan.stamp),
        CommitPointResult::RetirementWon
    );
    let cancelled_ack = acknowledged(cancel_supervisor.acknowledge_retirement(cancel_plan.stamp));
    let cancel_tail = cancelled
        .reduce(ProductInput::AttemptRetired(cancelled_ack))
        .unwrap();
    assert_eq!(cancelled.state, ProductState::Cancelled);
    assert_eq!(cancelled.quiescence, Quiescence::Quiescent);
    assert_eq!(cancelled.bytes, ByteCount::new(0));
    assert!(has_discard(&cancel_tail));

    // Cancel loses after the commit point. The optimistic cancel must not erase
    // progress needed by the adopted completion.
    let (mut completed, mut complete_supervisor, complete_plan, _) = transfer(Direction::Receive);
    observe(
        &complete_supervisor,
        &mut completed,
        AttemptEventKind::Progress {
            transferred: ByteCount::new(75),
        },
    );
    assert_eq!(
        complete_supervisor.cross_commit_point(complete_plan.stamp),
        CommitPointResult::Crossed
    );
    let effects = completed
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert!(!has_discard(&effects));
    assert_eq!(completed.bytes, ByteCount::new(75));
    assert_eq!(
        complete_supervisor.request_retirement(complete_plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    let completed_ack =
        acknowledged(complete_supervisor.acknowledge_retirement(complete_plan.stamp));
    let completed_tail = completed
        .reduce(ProductInput::AttemptRetired(completed_ack))
        .unwrap();
    assert_eq!(completed.state, ProductState::Completed);
    assert_eq!(completed.quiescence, Quiescence::Quiescent);
    assert_eq!(completed.bytes, completed.total());
    assert!(!has_discard(&completed_tail));
    assert!(matches!(
        completed_tail.as_slice(),
        [ProductEffect::CapabilityDuty {
            action: CapabilityAction::PostReceipt,
            ..
        }]
    ));

    // Pause becomes at-rest only after C7 acknowledges Paused.
    let (mut paused, mut pause_supervisor, pause_plan, _) = transfer(Direction::Send);
    let effects = paused
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    assert_eq!(
        paused.state,
        ProductState::Paused(crate::PauseOrigin::Local)
    );
    assert_eq!(
        paused.quiescence,
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Pause,
        }
    );
    assert_eq!(
        effects,
        vec![ProductEffect::RetireAttempt {
            stamp: pause_plan.stamp,
            intent: RetirementIntent::Pause,
        }]
    );
    assert_eq!(
        pause_supervisor.request_retirement(pause_plan.stamp, RetirementIntent::Pause),
        RetirementRequestResult::Requested
    );
    let paused_ack = acknowledged(pause_supervisor.acknowledge_retirement(pause_plan.stamp));
    assert!(
        paused
            .reduce(ProductInput::AttemptRetired(paused_ack))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        paused.state,
        ProductState::Paused(crate::PauseOrigin::Local)
    );
    assert_eq!(paused.quiescence, Quiescence::Quiescent);

    // A natural completion is still retiring until the finalized attempt
    // acknowledges. A second genuine C7 token for the same stamp is inert.
    let (mut natural, mut natural_supervisor, natural_plan, _) = transfer(Direction::Receive);
    assert_eq!(
        natural_supervisor.cross_commit_point(natural_plan.stamp),
        CommitPointResult::Crossed
    );
    let terminal_effects = observe(
        &natural_supervisor,
        &mut natural,
        AttemptEventKind::Terminal(OutcomeCode::Completed),
    );
    assert_eq!(natural.state, ProductState::Completed);
    assert_eq!(
        natural.quiescence,
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Finalize,
        }
    );
    assert!(
        !terminal_effects
            .iter()
            .any(|effect| matches!(effect, ProductEffect::CapabilityDuty { .. }))
    );
    assert_eq!(
        natural_supervisor.request_retirement(natural_plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let natural_ack = acknowledged(natural_supervisor.acknowledge_retirement(natural_plan.stamp));
    let natural_tail = natural
        .reduce(ProductInput::AttemptRetired(natural_ack))
        .unwrap();
    assert_eq!(natural.quiescence, Quiescence::Quiescent);
    assert!(matches!(
        natural_tail.as_slice(),
        [ProductEffect::CapabilityDuty {
            action: CapabilityAction::PostReceipt,
            ..
        }]
    ));

    let mut duplicate_source = AttemptSupervisor::new();
    assert_eq!(duplicate_source.open(natural_plan), OpenResult::Opened);
    assert_eq!(
        duplicate_source.cross_commit_point(natural_plan.stamp),
        CommitPointResult::Crossed
    );
    assert_eq!(
        duplicate_source.request_retirement(natural_plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let duplicate = acknowledged(duplicate_source.acknowledge_retirement(natural_plan.stamp));
    let snapshot = natural.clone();
    assert!(
        natural
            .reduce(ProductInput::AttemptRetired(duplicate))
            .unwrap()
            .is_empty()
    );
    assert_eq!(natural, snapshot);
}

#[test]
fn preparing_cancel_no_truncated_send() {
    let (mut record, _) = create(Direction::Send);
    give_a_source(&mut record);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(60),
        })
        .unwrap();

    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    assert_eq!(record.bytes, ByteCount::new(60));
    assert_eq!(
        record.quiescence,
        Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent: RetirementIntent::Cancel,
        }
    );
    assert_eq!(effects, vec![ProductEffect::RetireStaging { stamp }]);
    assert!(!has_discard(&effects));
    assert!(!has_start(&effects));

    let before_resume = record.clone();
    assert!(
        record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record, before_resume);

    let stale = envoix_attempt_api::AttemptStamp {
        card: stamp.card,
        generation: AttemptGen::new(stamp.generation.get() + 1),
    };
    assert!(
        record
            .reduce(ProductInput::StagingRetired { stamp: stale })
            .unwrap()
            .is_empty()
    );
    assert_eq!(record, before_resume);

    let tail = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    assert_eq!(record.quiescence, Quiescence::Quiescent);
    assert_eq!(record.bytes, ByteCount::new(0));
    assert!(has_discard(&tail));
    assert!(!has_start(&tail));
}

#[test]
fn completed_staging_launches_only_after_the_worker_retires() {
    let (mut record, _) = create(Direction::Send);
    give_a_source(&mut record);
    let stamp = record.stamp();
    let completion = record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 90),
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert_eq!(
        record.quiescence,
        Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent: RetirementIntent::Finalize,
        }
    );
    assert_eq!(completion, vec![ProductEffect::RetireStaging { stamp }]);
    assert!(!has_start(&completion));

    let launch = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert_eq!(
        record.quiescence,
        Quiescence::Running {
            worker: WorkerKind::Attempt,
        }
    );
    assert!(has_start(&launch));
}

#[test]
fn retirement_proofs_are_worker_and_generation_scoped() {
    let (mut staging, _) = create(Direction::Send);
    give_a_source(&mut staging);
    let staging_stamp = staging.stamp();
    staging
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();

    let staging_as_attempt = AttemptPlan {
        stamp: staging_stamp,
        direction: staging.direction,
        transfer: staging.identity.transfer,
        artifact: staging.identity.artifact,
        resume: envoix_attempt_api::ResumeIntent::Fresh,
    };
    let mut wrong_worker = AttemptSupervisor::new();
    assert_eq!(wrong_worker.open(staging_as_attempt), OpenResult::Opened);
    assert_eq!(
        wrong_worker.request_retirement(staging_stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    let wrong_worker_ack = acknowledged(wrong_worker.acknowledge_retirement(staging_stamp));
    let snapshot = staging.clone();
    assert!(
        staging
            .reduce(ProductInput::AttemptRetired(wrong_worker_ack))
            .unwrap()
            .is_empty()
    );
    assert_eq!(staging, snapshot);
    staging
        .reduce(ProductInput::StagingRetired {
            stamp: staging_stamp,
        })
        .unwrap();

    let (mut attempt, mut supervisor, plan, _) = transfer(Direction::Send);
    attempt
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    let snapshot = attempt.clone();
    assert!(
        attempt
            .reduce(ProductInput::StagingRetired { stamp: plan.stamp })
            .unwrap()
            .is_empty()
    );
    assert_eq!(attempt, snapshot);

    let wrong_stamp = envoix_attempt_api::AttemptStamp {
        card: plan.stamp.card,
        generation: AttemptGen::new(plan.stamp.generation.get() + 1),
    };
    let wrong_plan = AttemptPlan {
        stamp: wrong_stamp,
        ..plan
    };
    let mut wrong_generation = AttemptSupervisor::new();
    assert_eq!(wrong_generation.open(wrong_plan), OpenResult::Opened);
    assert_eq!(
        wrong_generation.request_retirement(wrong_stamp, RetirementIntent::Pause),
        RetirementRequestResult::Requested
    );
    let wrong_generation_ack = acknowledged(wrong_generation.acknowledge_retirement(wrong_stamp));
    assert!(
        attempt
            .reduce(ProductInput::AttemptRetired(wrong_generation_ack))
            .unwrap()
            .is_empty()
    );
    assert_eq!(attempt, snapshot);

    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Pause),
        RetirementRequestResult::Requested
    );
    let ack = acknowledged(supervisor.acknowledge_retirement(plan.stamp));
    attempt.reduce(ProductInput::AttemptRetired(ack)).unwrap();
    assert_eq!(attempt.quiescence, Quiescence::Quiescent);
}

#[test]
fn remove_tombstone_waits_for_attempt_retirement() {
    let (mut record, mut supervisor, plan, _) = transfer(Direction::Receive);
    let request = record
        .reduce(ProductInput::Command(ProductCommand::Remove))
        .unwrap();
    assert!(record.facts.remove_requested);
    assert_eq!(
        request,
        vec![ProductEffect::RetireAttempt {
            stamp: plan.stamp,
            intent: RetirementIntent::Cancel,
        }]
    );
    assert!(!request.iter().any(|effect| matches!(
        effect,
        ProductEffect::StorageIntent {
            action: StorageAction::TombstoneCard,
            ..
        }
    )));

    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    let ack = acknowledged(supervisor.acknowledge_retirement(plan.stamp));
    let tail = record.reduce(ProductInput::AttemptRetired(ack)).unwrap();
    assert_eq!(record.quiescence, Quiescence::Quiescent);
    assert!(matches!(
        tail.as_slice(),
        [ProductEffect::StorageIntent {
            action: StorageAction::TombstoneCard,
            ..
        }]
    ));
}
