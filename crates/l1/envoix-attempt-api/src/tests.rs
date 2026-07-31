use envoix_outcomes::{OutcomeCode, Phase};
use envoix_types::{ArtifactId, AttemptGen, ByteCount, Direction, RecordId, TransferId};

use crate::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor,
    CommitOperationResult, CommitPointResult, EventAdmission, OpenResult, ResumeIntent,
    RetirementAck, RetirementAckResult, RetirementIntent, RetirementRequestResult,
    TerminalResolutionResult,
};

fn stamp(card: u64, generation: u32) -> AttemptStamp {
    AttemptStamp {
        card: RecordId::new(card),
        generation: AttemptGen::new(generation),
    }
}

fn plan(card: u64, generation: u32) -> AttemptPlan {
    AttemptPlan {
        stamp: stamp(card, generation),
        direction: Direction::Receive,
        transfer: TransferId::from_bytes([card as u8; 16]),
        artifact: ArtifactId::from_bytes([generation as u8; 16]),
        resume: ResumeIntent::Allowed,
    }
}

fn event(stamp: AttemptStamp, kind: AttemptEventKind) -> AttemptEvent {
    AttemptEvent { stamp, kind }
}

fn acknowledged(result: RetirementAckResult) -> crate::RetirementAck {
    let RetirementAckResult::Acknowledged(ack) = result else {
        panic!("expected retirement acknowledgement");
    };
    ack
}

#[test]
fn attempt_contract_retirement_ack() {
    let mut supervisor = AttemptSupervisor::new();
    let first_plan = plan(7, 1);
    assert_eq!(supervisor.open(first_plan), OpenResult::Opened);

    let live_event = event(
        first_plan.stamp,
        AttemptEventKind::Progress {
            transferred: ByteCount::new(8192),
        },
    );
    let EventAdmission::Accepted(admitted) = supervisor.observe(live_event) else {
        panic!("current live event must be accepted");
    };
    assert_eq!(admitted.event(), live_event);
    assert_eq!(
        supervisor.observe(event(
            stamp(7, 2),
            AttemptEventKind::Phase(Phase::Transferring),
        )),
        EventAdmission::Unknown
    );

    assert_eq!(
        supervisor.request_retirement(first_plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    assert!(!supervisor.is_quiesced(first_plan.stamp));
    assert_eq!(
        supervisor.cross_commit_point(first_plan.stamp),
        CommitPointResult::RetirementWon
    );

    let cancelled = acknowledged(supervisor.acknowledge_retirement(first_plan.stamp));
    assert_eq!(cancelled.stamp(), first_plan.stamp);
    assert_eq!(cancelled.outcome(), OutcomeCode::Cancelled);
    assert!(supervisor.is_quiesced(first_plan.stamp));
    assert_eq!(supervisor.observe(live_event), EventAdmission::Retired);

    let second_plan = plan(7, 2);
    assert_eq!(supervisor.open(second_plan), OpenResult::Superseded);
    assert_eq!(supervisor.observe(live_event), EventAdmission::Stale);
    assert_eq!(
        supervisor.cross_commit_point(second_plan.stamp),
        CommitPointResult::Crossed
    );
    assert_eq!(
        supervisor.request_retirement(second_plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    assert!(!supervisor.is_quiesced(second_plan.stamp));

    let completed = acknowledged(supervisor.acknowledge_retirement(second_plan.stamp));
    assert_eq!(completed.outcome(), OutcomeCode::Completed);
    assert!(supervisor.is_quiesced(second_plan.stamp));
}

#[test]
fn finalize_ack_waits_for_the_commit_point() {
    let mut supervisor = AttemptSupervisor::new();
    let plan = plan(10, 4);
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::NotRequested
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::NotReady
    );
    assert!(!supervisor.is_quiesced(plan.stamp));

    assert_eq!(
        supervisor.cross_commit_point(plan.stamp),
        CommitPointResult::Crossed
    );
    let ack = acknowledged(supervisor.acknowledge_retirement(plan.stamp));
    assert_eq!(ack.outcome(), OutcomeCode::Completed);
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::AlreadyAcknowledged
    );
}

#[test]
fn newer_generation_cannot_overlap_a_live_attempt() {
    let mut supervisor = AttemptSupervisor::new();
    let first = plan(3, 8);
    let second = plan(3, 9);
    assert_eq!(supervisor.open(first), OpenResult::Opened);
    assert_eq!(supervisor.open(second), OpenResult::PreviousAttemptLive);
    assert!(matches!(
        supervisor.observe(event(
            first.stamp,
            AttemptEventKind::Phase(Phase::Authenticating),
        )),
        EventAdmission::Accepted(_)
    ));

    supervisor.request_retirement(first.stamp, RetirementIntent::Pause);
    let ack = acknowledged(supervisor.acknowledge_retirement(first.stamp));
    assert_eq!(ack.outcome(), OutcomeCode::Paused);
    assert_eq!(supervisor.open(second), OpenResult::Superseded);
    assert_eq!(
        supervisor.observe(event(
            first.stamp,
            AttemptEventKind::Phase(Phase::Transferring),
        )),
        EventAdmission::Stale
    );
}

#[test]
fn terminal_event_alone_does_not_claim_quiescence() {
    let mut supervisor = AttemptSupervisor::new();
    let plan = plan(5, 1);
    supervisor.open(plan);

    assert!(matches!(
        supervisor.observe(event(
            plan.stamp,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        )),
        EventAdmission::Accepted(_)
    ));
    assert!(!supervisor.is_quiesced(plan.stamp));
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::NotRequested
    );
}

#[test]
fn failed_attempt_resolves_only_through_finalize() {
    let mut supervisor = AttemptSupervisor::new();
    let plan = plan(6, 1);
    supervisor.open(plan);

    assert_eq!(
        supervisor.resolve_terminal(plan.stamp, OutcomeCode::PeerLost),
        crate::TerminalResolutionResult::Recorded
    );
    assert!(!supervisor.is_quiesced(plan.stamp));
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::NotRequested
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let ack = acknowledged(supervisor.acknowledge_retirement(plan.stamp));
    assert_eq!(ack.outcome(), OutcomeCode::PeerLost);
}

#[test]
fn failed_commit_operation_does_not_cross_the_commit_point() {
    let plan = plan(1, 1);
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);

    assert_eq!(
        supervisor.cross_commit_point_with(plan.stamp, || Err::<(), _>("seal failed")),
        CommitOperationResult::OperationFailed("seal failed")
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    assert_eq!(
        supervisor.cross_commit_point_with(plan.stamp, || Ok::<_, ()>(())),
        CommitOperationResult::RetirementWon
    );
}

#[test]
fn retirement_after_terminal_preserves_the_terminal_outcome() {
    let plan = plan(1, 1);
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(supervisor.open(plan), OpenResult::Opened);
    assert_eq!(
        supervisor.resolve_terminal(plan.stamp, OutcomeCode::PeerLost),
        TerminalResolutionResult::Recorded
    );
    assert_eq!(
        supervisor.request_retirement(plan.stamp, RetirementIntent::Cancel),
        RetirementRequestResult::Requested
    );
    assert_eq!(
        supervisor.acknowledge_retirement(plan.stamp),
        RetirementAckResult::Acknowledged(RetirementAck {
            stamp: plan.stamp,
            outcome: OutcomeCode::PeerLost,
        })
    );
}

#[test]
fn cards_and_generation_failures_are_independent() {
    let mut supervisor = AttemptSupervisor::new();
    let first = plan(1, 2);
    let second = plan(2, 5);
    supervisor.open(first);
    supervisor.open(second);

    assert_eq!(
        supervisor.request_retirement(stamp(1, 1), RetirementIntent::Cancel),
        RetirementRequestResult::Stale
    );
    assert_eq!(
        supervisor.request_retirement(stamp(1, 3), RetirementIntent::Cancel),
        RetirementRequestResult::Unknown
    );
    assert_eq!(
        supervisor.request_retirement(stamp(99, 1), RetirementIntent::Cancel),
        RetirementRequestResult::Unknown
    );
    assert!(matches!(
        supervisor.observe(event(second.stamp, AttemptEventKind::Phase(Phase::Pairing),)),
        EventAdmission::Accepted(_)
    ));
}

#[test]
fn plan_and_event_serde_round_trip() {
    let plan = plan(11, 6);
    let encoded_plan = serde_json::to_vec(&plan).expect("plan should serialize");
    let decoded_plan: AttemptPlan =
        serde_json::from_slice(&encoded_plan).expect("plan should deserialize");
    assert_eq!(decoded_plan, plan);

    let event = event(
        plan.stamp,
        AttemptEventKind::Progress {
            transferred: ByteCount::new(123_456),
        },
    );
    let encoded_event = serde_json::to_vec(&event).expect("event should serialize");
    let decoded_event: AttemptEvent =
        serde_json::from_slice(&encoded_event).expect("event should deserialize");
    assert_eq!(decoded_event, event);
}
