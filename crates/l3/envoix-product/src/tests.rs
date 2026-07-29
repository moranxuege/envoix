use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, ResumeIntent, RetirementAck, RetirementAckResult, RetirementIntent,
};
use envoix_capabilities::{
    Admission, DutyKind, DutyLedger, DutyProvenance, DutyResult, GenerationUpdate, Registration,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{
    ArtifactId, AttemptGen, ByteCount, CommandId, Direction, OfferedName, RecordId, RequestId,
    TransferId,
};

use crate::{
    CapabilityAction, Facts, IdentityError, IdentitySource, NewTransfer, PauseOrigin,
    ProductCommand, ProductEffect, ProductIdentity, ProductInput, ProductState, Quiescence,
    RecordCodecError, RecordDecode, SourceDecision, StorageAction, TransferRecord, WorkerKind,
    decode_record, encode_record, resolve_source,
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

struct UnavailableEntropy;

impl IdentitySource for UnavailableEntropy {
    fn fill(&mut self, _destination: &mut [u8]) -> Result<(), IdentityError> {
        Err(IdentityError::EntropyUnavailable)
    }
}

fn create(direction: Direction, source: SourceDecision) -> (TransferRecord, Vec<ProductEffect>) {
    TransferRecord::create(
        NewTransfer {
            direction,
            participation: crate::RoomParticipation::Minted,
            offered_name: OfferedName::from_untrusted("a.zip").unwrap(),
            total: ByteCount::new(100),
            source,
            pairing: None,
        },
        &mut DeterministicEntropy::default(),
    )
    .expect("deterministic identity source")
}

fn ready(direction: Direction) -> TransferRecord {
    create(direction, SourceDecision::Ready).0
}

fn preparing(direction: Direction, recoverable: bool) -> TransferRecord {
    create(direction, SourceDecision::Stage { recoverable }).0
}

fn event(record: &TransferRecord, kind: AttemptEventKind) -> ProductInput {
    admitted_event(record, record.stamp(), kind)
}

fn admitted_event(
    record: &TransferRecord,
    stamp: AttemptStamp,
    kind: AttemptEventKind,
) -> ProductInput {
    let mut supervisor = AttemptSupervisor::new();
    assert_eq!(
        supervisor.open(AttemptPlan {
            stamp,
            direction: record.direction,
            transfer: record.identity.transfer,
            artifact: record.identity.artifact,
            resume: ResumeIntent::Fresh,
        }),
        OpenResult::Opened
    );
    match supervisor.observe(AttemptEvent { stamp, kind }) {
        EventAdmission::Accepted(event) => ProductInput::AttemptObserved(event),
        other => panic!("freshly opened event was not admitted: {other:?}"),
    }
}

fn admitted_duty_result(
    provenance: DutyProvenance,
    outcome: OutcomeCode,
) -> envoix_capabilities::AdmittedDutyResult {
    let mut ledger = DutyLedger::new();
    assert_eq!(
        ledger.advance_generation(provenance.card, provenance.generation),
        GenerationUpdate::Initialized
    );
    let duty = envoix_capabilities::Duty {
        provenance,
        kind: DutyKind::Courier,
    };
    assert_eq!(ledger.register(duty), Registration::Registered);
    match ledger.admit(DutyResult {
        provenance,
        outcome,
    }) {
        Admission::Fresh(result) => result,
        other => panic!("registered duty result was not admitted: {other:?}"),
    }
}

fn stale_stamp(record: &TransferRecord) -> AttemptStamp {
    AttemptStamp {
        card: record.identity.card,
        generation: AttemptGen::new(record.generation.get().wrapping_add(17)),
    }
}

fn transfer(direction: Direction) -> TransferRecord {
    let mut record = ready(direction);
    assert!(
        record
            .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring),))
            .unwrap()
            .is_empty()
    );
    assert!(
        record
            .reduce(event(
                &record,
                AttemptEventKind::Progress {
                    transferred: ByteCount::new(40),
                },
            ))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record.state, ProductState::Transferring);
    record
}

fn start_plan(effects: &[ProductEffect]) -> AttemptPlan {
    effects
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::StartAttempt { plan } => Some(*plan),
            _ => None,
        })
        .expect("StartAttempt effect")
}

/// Mints a real C7 [`RetirementAck`] for the record's current generation by
/// driving an [`AttemptSupervisor`] — so tests exercise the ACTUAL cancel-vs-
/// commit linearization. `cross_first` = the attempt's commit crossed before
/// the retirement resolved (so a Cancel/Finalize resolves to `Completed`).
fn retirement_ack(
    record: &TransferRecord,
    intent: RetirementIntent,
    cross_first: bool,
) -> RetirementAck {
    let plan = AttemptPlan {
        stamp: record.stamp(),
        direction: record.direction,
        transfer: record.identity.transfer,
        artifact: record.identity.artifact,
        resume: ResumeIntent::Fresh,
    };
    let mut supervisor = AttemptSupervisor::new();
    assert!(matches!(supervisor.open(plan), OpenResult::Opened));
    if cross_first {
        supervisor.cross_commit_point(plan.stamp);
    }
    supervisor.request_retirement(plan.stamp, intent);
    match supervisor.acknowledge_retirement(plan.stamp) {
        RetirementAckResult::Acknowledged(ack) => ack,
        other => panic!("expected an acknowledged retirement, got {other:?}"),
    }
}

/// Drives the record to quiescence via a retirement ack of the given intent
/// (commit not crossed → the ack's outcome matches the intent). Returns the
/// released effects (e.g. the deferred discard on a Cancel).
fn quiesce(record: &mut TransferRecord, intent: RetirementIntent) -> Vec<ProductEffect> {
    let ack = retirement_ack(record, intent, false);
    record.reduce(ProductInput::AttemptRetired(ack)).unwrap()
}

fn assert_outcome(record: &TransferRecord, code: OutcomeCode) {
    assert_eq!(
        record.outcome.as_ref().map(|outcome| outcome.code),
        Some(code)
    );
}

#[test]
fn product_mints_all_identity_before_the_first_attempt() {
    let (record, effects) = create(Direction::Send, SourceDecision::Ready);
    let plan = start_plan(&effects);
    assert_ne!(record.identity.card.get(), 0);
    assert_ne!(record.identity.transfer.to_bytes(), [0; 16]);
    assert_ne!(record.identity.artifact.to_bytes(), [0; 16]);
    assert_ne!(record.generation.get(), 0);
    assert_eq!(plan.stamp, record.stamp());
    assert_eq!(plan.transfer, record.identity.transfer);
    assert_eq!(plan.artifact, record.identity.artifact);
    assert_eq!(plan.resume, ResumeIntent::Fresh);

    let error = TransferRecord::create(
        NewTransfer {
            direction: Direction::Send,
            participation: crate::RoomParticipation::Minted,
            offered_name: OfferedName::from_untrusted("a.zip").unwrap(),
            total: ByteCount::new(1),
            source: SourceDecision::Ready,
            pairing: None,
        },
        &mut UnavailableEntropy,
    )
    .expect_err("identity creation must fail closed");
    assert_eq!(error, IdentityError::EntropyUnavailable);
}

#[test]
fn receiver_adopts_authenticated_transfer_identity_and_mints_local_identity() {
    let transfer = TransferId::from_bytes([0xa1; 16]);
    let artifact = ArtifactId::from_bytes([0xb2; 16]);
    let (record, effects) = TransferRecord::create_with_identity(
        NewTransfer {
            direction: Direction::Receive,
            participation: crate::RoomParticipation::Minted,
            offered_name: OfferedName::from_untrusted("authenticated.zip").unwrap(),
            total: ByteCount::new(321),
            source: SourceDecision::Ready,
            pairing: None,
        },
        transfer,
        artifact,
        &mut DeterministicEntropy::default(),
    )
    .expect("local identity minting succeeds");

    let plan = start_plan(&effects);
    assert_eq!(record.identity.transfer, transfer);
    assert_eq!(record.identity.artifact, artifact);
    assert_ne!(record.identity.card.get(), 0);
    assert_ne!(record.generation.get(), 0);
    assert_eq!(plan.stamp, record.stamp());
    assert_eq!(plan.transfer, transfer);
    assert_eq!(plan.artifact, artifact);
}

#[test]
fn source_precedence_is_owned_by_product_policy() {
    assert_eq!(resolve_source(true, None), SourceDecision::Ready);
    assert_eq!(
        resolve_source(false, Some(true)),
        SourceDecision::Stage { recoverable: true }
    );
    assert_eq!(
        resolve_source(false, Some(false)),
        SourceDecision::Stage { recoverable: false }
    );
    assert_eq!(resolve_source(false, None), SourceDecision::NeedsRepick);
}

#[test]
fn receive_happy_path_posts_receipt() {
    let mut record = ready(Direction::Receive);
    let stamp = record.stamp();
    record.reduce(ProductInput::Advertised { stamp }).unwrap();
    assert_eq!(record.state, ProductState::Waiting);
    record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Pairing)))
        .unwrap();
    record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(100),
            },
        ))
        .unwrap();
    let effects = record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();

    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(record.bytes, record.total);
    // The receipt duty is DEFERRED behind the attempt's retirement (committed
    // effects + quiescence): the terminal only asks the attempt to finalize.
    assert!(matches!(
        effects.as_slice(),
        [ProductEffect::RetireAttempt {
            intent: RetirementIntent::Finalize,
            ..
        }]
    ));
    // The attempt retires with its commit crossed → the receipt is now posted.
    let posted = record
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &record,
            RetirementIntent::Finalize,
            true,
        )))
        .unwrap();
    assert_eq!(record.quiescence, crate::Quiescence::Quiescent);
    assert!(matches!(
        posted.as_slice(),
        [ProductEffect::CapabilityDuty {
            duty,
            action: CapabilityAction::PostReceipt,
        }] if duty.kind == DutyKind::Courier
    ));
}

#[test]
fn storage_failed_ends_active_states_and_spares_terminal_ones() {
    let mut active = transfer(Direction::Send);
    let effects = active.reduce(ProductInput::StorageFailed).unwrap();
    assert_eq!(active.state, ProductState::Failed);
    assert_outcome(&active, OutcomeCode::StorageFault);
    assert!(effects.iter().any(|effect| matches!(
        effect,
        ProductEffect::RetireAttempt {
            intent: RetirementIntent::Cancel,
            ..
        }
    )));

    let mut completed = transfer(Direction::Send);
    completed
        .reduce(event(
            &completed,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    // Only a DURABLY at-rest terminal (the attempt has retired → Quiescent) is
    // spared; a still-Retiring terminal would escalate (that is how P2 surfaces
    // a failed destructive write).
    completed.quiescence = crate::Quiescence::Quiescent;
    let snapshot = completed.clone();
    assert!(
        completed
            .reduce(ProductInput::StorageFailed)
            .unwrap()
            .is_empty()
    );
    assert_eq!(completed, snapshot);
}

#[test]
fn stage_complete_launches_the_first_attempt_fresh() {
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(80),
        })
        .unwrap();
    // Staging COMPLETE records the source but does NOT launch the attempt yet:
    // the staging worker must retire (release its lease) first, so a fresh
    // attempt never races the staged prefix.
    let effects = record
        .reduce(ProductInput::StageComplete {
            stamp,
            total: ByteCount::new(80),
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert!(record.facts.source_ready);
    assert_eq!(
        record.total,
        ByteCount::new(80),
        "the staged artifact is the authoritative source length"
    );
    assert_eq!(effects, vec![ProductEffect::RetireStaging { stamp }]);

    // The staging worker retires; only now does the first attempt launch fresh.
    let launched = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert_eq!(record.bytes, ByteCount::new(0));
    assert_eq!(start_plan(&launched).resume, ResumeIntent::Fresh);
}

#[test]
fn stage_progress_only_moves_the_bar_in_preparing() {
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(80),
        })
        .unwrap();
    assert_eq!(record.bytes, ByteCount::new(80));

    let mut transferring = transfer(Direction::Send);
    let stamp = transferring.stamp();
    transferring
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(99),
        })
        .unwrap();
    assert_eq!(transferring.bytes, ByteCount::new(40));
}

#[test]
fn stage_failed_fails_with_typed_source_recovery() {
    let mut recoverable = preparing(Direction::Send, true);
    let stamp = recoverable.stamp();
    recoverable
        .reduce(ProductInput::StageFailed { stamp })
        .unwrap();
    assert_eq!(recoverable.state, ProductState::Failed);
    assert_outcome(&recoverable, OutcomeCode::SourceUnreadable);
    assert_eq!(
        recoverable
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.recovery),
        Some(Recovery::RetryLater)
    );

    let mut needs_repick = preparing(Direction::Send, false);
    let stamp = needs_repick.stamp();
    needs_repick
        .reduce(ProductInput::StageFailed { stamp })
        .unwrap();
    assert_eq!(
        needs_repick
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );
}

#[test]
fn cancel_during_preparing_retires_the_worker_before_discarding() {
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    // A Preparing cancel asks the staging WORKER to retire; the destructive
    // discard is deferred until the worker confirms it stopped — otherwise a
    // half-staged file could be shipped by a later send.
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    assert!(record.quiescence.is_retiring(), "not yet at rest");
    assert_eq!(effects, vec![ProductEffect::RetireStaging { stamp }]);
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            ProductEffect::StorageIntent {
                action: StorageAction::DiscardPartial,
                ..
            }
        )),
        "no discard until the worker retires"
    );

    // The worker retires; only now does the discard fire and the card quiesce.
    let released = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.quiescence, crate::Quiescence::Quiescent);
    assert_eq!(
        released,
        vec![ProductEffect::StorageIntent {
            identity: record.identity,
            action: StorageAction::DiscardPartial,
        }]
    );
}

#[test]
fn pause_during_preparing_is_a_noop() {
    let mut record = preparing(Direction::Send, true);
    assert!(
        record
            .reduce(ProductInput::Command(ProductCommand::Pause))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record.state, ProductState::Preparing);
}

#[test]
fn stage_inputs_off_preparing_are_dropped() {
    let mut record = transfer(Direction::Send);
    let stamp = record.stamp();
    let snapshot = record.clone();
    assert!(
        record
            .reduce(ProductInput::StageComplete {
                stamp,
                total: ByteCount::new(100),
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(record, snapshot);
}

#[test]
fn stale_generation_staging_inputs_are_rejected_after_retry() {
    let mut record = preparing(Direction::Send, true);
    let first = record.stamp();
    record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    // The staging worker retires before the card can re-stage under a new gen.
    record
        .reduce(ProductInput::StagingRetired { stamp: first })
        .unwrap();
    assert!(
        record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record.state, ProductState::Preparing);
    assert_ne!(record.stamp(), first);

    let snapshot = record.clone();
    for input in [
        ProductInput::StageComplete {
            stamp: first,
            total: ByteCount::new(100),
        },
        ProductInput::StageFailed { stamp: first },
        ProductInput::StageProgress {
            stamp: first,
            transferred: ByteCount::new(100),
        },
    ] {
        assert!(record.reduce(input).unwrap().is_empty());
        assert_eq!(record, snapshot);
    }

    let stamp = record.stamp();
    let staged = record
        .reduce(ProductInput::StageComplete {
            stamp,
            total: ByteCount::new(100),
        })
        .unwrap();
    assert_eq!(staged, vec![ProductEffect::RetireStaging { stamp }]);
    let launched = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert_eq!(start_plan(&launched).resume, ResumeIntent::Fresh);
}

#[test]
fn resume_from_failed_staging_re_stages_not_the_wire() {
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    record.reduce(ProductInput::StageFailed { stamp }).unwrap();
    // The staging worker must retire before the failed card is at rest.
    record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert!(effects.is_empty());
}

#[test]
fn resume_after_completed_staging_goes_to_the_wire() {
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    // Staging completes and its worker retires: the attempt goes to the wire.
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            total: ByteCount::new(100),
        })
        .unwrap();
    record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert!(record.facts.source_ready);
    // Pausing then resuming stays on the wire (the source is ready); it does
    // NOT drop back to re-staging.
    record
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    quiesce(&mut record, RetirementIntent::Pause);
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert_eq!(
        start_plan(&effects).resume,
        ResumeIntent::ResumeFrom {
            offset: ByteCount::new(0)
        }
    );
}

#[test]
fn cancel_clears_progress_and_the_fresh_resume_inherits_it() {
    let mut record = transfer(Direction::Send);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(50),
            },
        ))
        .unwrap();
    record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    // Optimistic cancel KEEPS the bytes: the discard (and the byte reset) is
    // deferred to the retirement ack, so a cancel that loses to a crossed
    // commit can still surface its real progress.
    assert!(record.quiescence.is_retiring());
    assert_eq!(record.bytes, ByteCount::new(50));
    // A fresh resume waits for the cancelled attempt to retire (quiescence);
    // only then are the bytes cleared and the partial discarded.
    let discard = quiesce(&mut record, RetirementIntent::Cancel);
    assert!(discard.iter().any(|e| matches!(
        e,
        ProductEffect::StorageIntent {
            action: StorageAction::DiscardPartial,
            ..
        }
    )));
    assert_eq!(record.bytes, ByteCount::new(0));
    assert_eq!(record.bytes_resumed, ByteCount::new(0));
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Fresh);
}

#[test]
fn paused_resume_keeps_progress_until_phase_corrects_it() {
    let mut record = transfer(Direction::Send);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(50),
            },
        ))
        .unwrap();
    record
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    // The pause is not at rest until its attempt retires; only then can resume
    // launch a new generation. The partial progress is preserved throughout.
    quiesce(&mut record, RetirementIntent::Pause);
    assert_eq!(record.bytes, ByteCount::new(50));
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(record.bytes, ByteCount::new(50));
    assert_eq!(
        start_plan(&effects).resume,
        ResumeIntent::ResumeFrom {
            offset: ByteCount::new(50)
        }
    );
    record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    assert_eq!(record.bytes_resumed, ByteCount::new(50));
}

#[test]
fn verification_inputs_preserve_the_product_state() {
    let mut record = ready(Direction::Receive);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::VerificationStarted { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Verifying);
    record
        .reduce(ProductInput::VerificationFinished { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);

    record
        .reduce(ProductInput::VerificationStarted { stamp })
        .unwrap();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
}

#[test]
fn receipt_mismatch_is_a_monotone_fact_not_a_verdict() {
    let mut record = confirming_send();
    let stamp = record.stamp();
    assert!(
        record
            .reduce(ProductInput::ReceiptMismatch { stamp })
            .unwrap()
            .is_empty()
    );
    assert_eq!(record.state, ProductState::Confirming);
    assert!(record.facts.receipt_mismatch);

    record
        .reduce(ProductInput::ReceiptVerified { stamp })
        .unwrap();
    let snapshot = record.clone();
    record
        .reduce(ProductInput::ReceiptMismatch { stamp })
        .unwrap();
    assert_eq!(record, snapshot);
}

fn confirming_send() -> TransferRecord {
    let mut record = transfer(Direction::Send);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(100),
            },
        ))
        .unwrap();
    let effects = record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Confirming)))
        .unwrap();
    assert_eq!(
        effects,
        vec![
            ProductEffect::StartConfirmTimer {
                stamp: record.stamp(),
            },
            ProductEffect::StartMailboxPoll {
                stamp: record.stamp(),
            },
        ]
    );
    record
}

#[test]
fn send_happy_path_confirms_then_completes() {
    let mut record = confirming_send();
    assert!(record.facts.complete_sent);
    let effects = record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(
        effects,
        vec![
            ProductEffect::StopConfirmTimer {
                stamp: record.stamp(),
            },
            ProductEffect::StopMailboxPoll {
                stamp: record.stamp(),
            },
            ProductEffect::RetireAttempt {
                stamp: record.stamp(),
                intent: RetirementIntent::Finalize,
            },
        ]
    );
}

#[test]
fn pause_survives_the_failed_echo() {
    let mut record = transfer(Direction::Send);
    let stamp = record.stamp();
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    assert_eq!(record.state, ProductState::Paused(PauseOrigin::Local));
    assert_eq!(
        effects,
        vec![ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Pause,
        }]
    );
    for code in [OutcomeCode::Cancelled, OutcomeCode::PeerLost] {
        let input = admitted_event(&record, stamp, AttemptEventKind::Terminal(code));
        assert!(record.reduce(input).unwrap().is_empty());
    }
    assert_eq!(record.state, ProductState::Paused(PauseOrigin::Local));
}

#[test]
fn cancel_during_pairing_survives_late_events() {
    let mut record = ready(Direction::Receive);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    let snapshot = record.clone();
    for kind in [
        AttemptEventKind::Phase(Phase::Pairing),
        AttemptEventKind::Phase(Phase::Authenticating),
        AttemptEventKind::Terminal(OutcomeCode::Cancelled),
    ] {
        let input = admitted_event(&record, stamp, kind);
        assert!(record.reduce(input).unwrap().is_empty());
        assert_eq!(record, snapshot);
    }
}

#[test]
fn stale_bytes_cannot_fake_unconfirmed() {
    let mut record = confirming_send();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    let snapshot = record.clone();
    assert!(
        record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record, snapshot);
}

#[test]
fn completed_is_terminal_resume_is_a_noop() {
    for direction in [Direction::Send, Direction::Receive] {
        let mut record = transfer(direction);
        record
            .reduce(event(
                &record,
                AttemptEventKind::Terminal(OutcomeCode::Completed),
            ))
            .unwrap();
        let snapshot = record.clone();
        assert!(
            record
                .reduce(ProductInput::Command(ProductCommand::Resume))
                .unwrap()
                .is_empty()
        );
        assert_eq!(record, snapshot);
    }
}

#[test]
fn confirming_connection_lost_escalates_to_mailbox() {
    let mut record = confirming_send();
    let stamp = record.stamp();
    let effects = record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::PeerLost),
        ))
        .unwrap();
    assert_eq!(record.state, ProductState::Unconfirmed);
    assert_eq!(
        effects,
        vec![
            ProductEffect::StopConfirmTimer { stamp },
            ProductEffect::StopMailboxPoll { stamp },
            ProductEffect::StartMailboxPoll { stamp },
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Finalize,
            },
        ]
    );
    let effects = record
        .reduce(ProductInput::ReceiptVerified { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(effects, vec![ProductEffect::StopMailboxPoll { stamp }]);
}

#[test]
fn confirm_timeout_escalates_proactively_and_stale_timers_are_ignored() {
    let mut record = confirming_send();
    assert!(
        record
            .reduce(ProductInput::ConfirmTimeout {
                stamp: stale_stamp(&record),
            })
            .unwrap()
            .is_empty()
    );
    let stamp = record.stamp();
    let effects = record
        .reduce(ProductInput::ConfirmTimeout { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Unconfirmed);
    assert_eq!(
        effects,
        vec![
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            },
            ProductEffect::StartMailboxPoll { stamp },
        ]
    );
    assert!(
        record
            .reduce(ProductInput::ConfirmTimeout { stamp })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn confirm_timeout_stays_retiring_and_never_discards_a_delivered_send() {
    // A confirm-timeout hands authority to the mailbox but keeps the card
    // RETIRING until the attempt's ack: its quiescence must mirror the
    // executor's (C7 forbids opening a fresh generation while this one is live).
    let mut record = confirming_send();
    let stamp = record.stamp();
    record
        .reduce(ProductInput::ConfirmTimeout { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Unconfirmed);
    assert!(
        record.quiescence.is_retiring(),
        "not at rest until the attempt retires — else a resume would race C7"
    );

    // Resume BEFORE the ack is inert: no fresh generation can open yet.
    assert!(
        record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap()
            .is_empty()
    );
    assert_eq!(record.state, ProductState::Unconfirmed);

    // The attempt retires with a Cancelled outcome (its RetireAttempt took
    // effect). The send WAS transmitted — the mailbox is the authority — so the
    // ack must NOT discard it: the card stays Unconfirmed, and only now quiesces.
    let released = quiesce(&mut record, RetirementIntent::Cancel);
    assert_eq!(record.state, ProductState::Unconfirmed);
    assert_eq!(record.quiescence, crate::Quiescence::Quiescent);
    assert!(
        !released.iter().any(|e| matches!(
            e,
            ProductEffect::StorageIntent {
                action: StorageAction::DiscardPartial,
                ..
            }
        )),
        "a delivered-but-unconfirmed send is never discarded by its retirement ack"
    );

    // The mailbox then confirms → Completed.
    record
        .reduce(ProductInput::ReceiptVerified {
            stamp: record.stamp(),
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
}

#[test]
fn confirm_timeout_ack_that_crossed_commit_completes_the_send() {
    // If the CompleteAck actually landed (the retirement linearizes to
    // Completed), the ack finalizes the send rather than leaving it Unconfirmed.
    let mut record = confirming_send();
    let stamp = record.stamp();
    record
        .reduce(ProductInput::ConfirmTimeout { stamp })
        .unwrap();
    record
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &record,
            RetirementIntent::Cancel,
            true,
        )))
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(record.quiescence, crate::Quiescence::Quiescent);
}

#[test]
fn receipt_verified_during_confirming_completes_and_stops_the_wait() {
    let mut record = confirming_send();
    let stamp = record.stamp();
    let effects = record
        .reduce(ProductInput::ReceiptVerified { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(
        effects,
        vec![
            ProductEffect::StopConfirmTimer { stamp },
            ProductEffect::StopMailboxPoll { stamp },
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            },
        ]
    );
    assert!(
        record
            .reduce(ProductInput::AttemptEnded { stamp })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn receipt_verified_completion_survives_cancelled_retirement_ack() {
    let mut record = confirming_send();
    let stamp = record.stamp();
    record
        .reduce(ProductInput::ReceiptVerified { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Completed);
    assert!(record.quiescence.is_retiring());

    let effects = record
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &record,
            RetirementIntent::Cancel,
            false,
        )))
        .unwrap();

    assert_eq!(record.state, ProductState::Completed);
    assert_eq!(record.quiescence, crate::Quiescence::Quiescent);
    assert_eq!(record.bytes, record.total);
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        ProductEffect::StorageIntent {
            action: StorageAction::DiscardPartial,
            ..
        }
    )));
}

#[test]
fn peer_pause_and_lost_connection_classify_as_paused() {
    let mut peer_paused = transfer(Direction::Receive);
    peer_paused
        .reduce(event(
            &peer_paused,
            AttemptEventKind::Terminal(OutcomeCode::Paused),
        ))
        .unwrap();
    assert_eq!(peer_paused.state, ProductState::Paused(PauseOrigin::Peer));

    let mut lost = transfer(Direction::Receive);
    lost.reduce(event(
        &lost,
        AttemptEventKind::Terminal(OutcomeCode::PeerLost),
    ))
    .unwrap();
    assert_eq!(lost.state, ProductState::Paused(PauseOrigin::Lost));

    let mut no_progress = ready(Direction::Receive);
    no_progress
        .reduce(event(
            &no_progress,
            AttemptEventKind::Terminal(OutcomeCode::PeerLost),
        ))
        .unwrap();
    assert_eq!(no_progress.state, ProductState::Failed);
}

#[test]
fn peer_cancel_discards_and_restart_is_fresh() {
    let mut record = transfer(Direction::Receive);
    // A peer-cancel terminal moves the card to Cancelled optimistically and
    // requests the attempt finalize; the destructive discard is DEFERRED until
    // the attempt retires (Pillar 4 — no discard while the lease is held).
    let effects = record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Cancelled),
        ))
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    assert!(record.quiescence.is_retiring());
    // The attempt already observed the peer's terminal, so it is CLEAN-CLOSED
    // (Finalize); the destructive discard is enacted by the adopted ack, not the
    // retirement request's intent.
    assert_eq!(
        effects,
        vec![ProductEffect::RetireAttempt {
            stamp: record.stamp(),
            intent: RetirementIntent::Finalize,
        }]
    );
    // The attempt retires with the (peer-)Cancelled outcome: now the discard
    // fires and the card is quiescent.
    let discard = quiesce(&mut record, RetirementIntent::Cancel);
    assert_eq!(
        discard,
        vec![ProductEffect::StorageIntent {
            identity: record.identity,
            action: StorageAction::DiscardPartial,
        }]
    );
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Fresh);
}

#[test]
fn resume_from_resting_retryable_states_uses_resume_semantics() {
    for origin in [PauseOrigin::Local, PauseOrigin::Peer, PauseOrigin::Lost] {
        let mut record = transfer(Direction::Send);
        record.state = ProductState::Paused(origin);
        // A genuinely at-rest paused card: its worker has retired (Quiescent),
        // which is the precondition for a fresh attempt to launch.
        record.quiescence = crate::Quiescence::Quiescent;
        let offset = record.bytes;
        let effects = record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap();
        assert_eq!(
            start_plan(&effects).resume,
            ResumeIntent::ResumeFrom { offset }
        );
    }

    let mut record = confirming_send();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::PeerLost),
        ))
        .unwrap();
    // The lost attempt has retired (mailbox is now the resting card's channel).
    record.quiescence = crate::Quiescence::Quiescent;
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert!(matches!(
        effects.as_slice(),
        [
            ProductEffect::StopMailboxPoll { .. },
            ProductEffect::StartAttempt {
                plan: AttemptPlan {
                    resume: ResumeIntent::ResumeFrom { .. },
                    ..
                }
            }
        ]
    ));
}

#[test]
fn completed_short_path_uses_product_owned_identity_and_total() {
    let mut record = ready(Direction::Receive);
    let identity = record.identity;
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(record.identity, identity);
    assert_eq!(record.bytes, record.total);
}

#[test]
fn run_ended_is_a_belt_never_a_silent_success() {
    let mut record = transfer(Direction::Send);
    let stamp = record.stamp();
    record.reduce(ProductInput::AttemptEnded { stamp }).unwrap();
    assert_eq!(record.state, ProductState::Failed);
    assert_outcome(&record, OutcomeCode::Internal);

    let mut paused = transfer(Direction::Send);
    paused
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    let snapshot = paused.clone();
    assert!(
        paused
            .reduce(ProductInput::AttemptEnded {
                stamp: paused.stamp(),
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(paused, snapshot);
}

#[test]
fn cancel_from_resting_states_and_terminal_inputs_are_ignored() {
    let mut paused = transfer(Direction::Send);
    paused
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    // The pause must retire to rest before it can be cancelled; a cancel while
    // the pause is still in flight is dropped.
    assert!(
        paused
            .reduce(ProductInput::Command(ProductCommand::Cancel))
            .unwrap()
            .is_empty(),
        "a cancel is dropped while a retirement is in flight"
    );
    assert_eq!(paused.state, ProductState::Paused(PauseOrigin::Local));
    quiesce(&mut paused, RetirementIntent::Pause);
    // Now quiescent: the cancel takes effect and discards the partial.
    let effects = paused
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert!(effects.iter().any(|e| matches!(
        e,
        ProductEffect::StorageIntent {
            action: StorageAction::DiscardPartial,
            ..
        }
    )));
    assert_eq!(paused.state, ProductState::Cancelled);

    for state in [
        ProductState::Completed,
        ProductState::Failed,
        ProductState::Cancelled,
    ] {
        let mut record = transfer(Direction::Send);
        record.state = state;
        let snapshot = record.clone();
        assert!(
            record
                .reduce(ProductInput::Command(ProductCommand::Cancel))
                .unwrap()
                .is_empty()
        );
        assert_eq!(record, snapshot);
    }
}

#[test]
fn restore_derives_state_from_durable_facts() {
    let mut confirming = confirming_send();
    let stamp = confirming.stamp();
    let effects = confirming.reduce(ProductInput::Restore).unwrap();
    assert_eq!(confirming.state, ProductState::Unconfirmed);
    assert_eq!(confirming.quiescence, crate::Quiescence::Quiescent);
    // Process teardown proves the attempt is gone; restore only needs to resume
    // the durable proof channel.
    assert_eq!(effects, vec![ProductEffect::StartMailboxPoll { stamp }]);

    let mut active = transfer(Direction::Receive);
    active.reduce(ProductInput::Restore).unwrap();
    assert_eq!(active.state, ProductState::Paused(PauseOrigin::Lost));
    assert_eq!(active.quiescence, crate::Quiescence::Quiescent);

    let mut preparing = preparing(Direction::Send, false);
    preparing.reduce(ProductInput::Restore).unwrap();
    assert_eq!(preparing.state, ProductState::Failed);
    assert_eq!(preparing.quiescence, crate::Quiescence::Quiescent);
    assert_eq!(
        preparing
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );

    let mut completed = transfer(Direction::Send);
    completed
        .reduce(event(
            &completed,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    let phase = completed.phase;
    completed.reduce(ProductInput::Restore).unwrap();
    assert_eq!(completed.state, ProductState::Completed);
    assert_eq!(completed.quiescence, crate::Quiescence::Quiescent);
    assert_eq!(
        completed.phase, phase,
        "restore preserves terminal evidence"
    );
}

#[test]
fn restore_reconciles_a_durable_retiring_cancel_without_discarding() {
    let mut record = transfer(Direction::Receive);
    let bytes = record.bytes;
    record
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(record.state, ProductState::Cancelled);
    assert!(record.quiescence.is_retiring());

    let encoded = encode_record(&record).unwrap();
    let RecordDecode::Loaded(mut restored) = decode_record(&encoded).unwrap() else {
        panic!("current product record must load");
    };
    let effects = restored.reduce(ProductInput::Restore).unwrap();

    assert!(effects.is_empty());
    assert_eq!(restored.state, ProductState::Paused(PauseOrigin::Lost));
    assert_eq!(restored.quiescence, crate::Quiescence::Quiescent);
    assert_eq!(restored.bytes, bytes);
    assert_eq!(
        restored.allowed_commands(),
        vec![
            ProductCommand::Resume,
            ProductCommand::Cancel,
            ProductCommand::Remove,
        ]
    );
}

#[test]
fn staging_handoff_state_round_trips_through_the_codec() {
    // Topology F1: `StageComplete` leaves the card `Preparing + source_ready +
    // Retiring(Staging, Finalize)` until `StagingRetired`. The commit barrier must
    // persist that durable handoff, so it MUST encode/decode — but ANY other
    // `Preparing + source_ready` is still invalid.
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            total: ByteCount::new(90),
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert!(record.facts.source_ready);
    assert!(matches!(
        record.quiescence,
        crate::Quiescence::Retiring {
            worker: crate::WorkerKind::Staging,
            ..
        }
    ));

    let encoded = encode_record(&record).expect("the staging handoff must be encodable");
    let RecordDecode::Loaded(decoded) = decode_record(&encoded).expect("and decodable") else {
        panic!("the staging handoff must load");
    };
    assert_eq!(*decoded, record);

    // The same state without the staging retirement is still rejected.
    let mut bogus = record.clone();
    bogus.quiescence = crate::Quiescence::Quiescent;
    assert!(
        encode_record(&bogus).is_err(),
        "a ready source outside the handoff must not be Preparing"
    );
    // Only the Finalize handoff is valid; a Cancel-intent staging retirement in a
    // Preparing + source_ready record is not reducer-reachable and must not decode
    // (else restore would turn a cancelled staging into a StartAttempt).
    let mut cancel_handoff = record.clone();
    cancel_handoff.quiescence = crate::Quiescence::Retiring {
        worker: crate::WorkerKind::Staging,
        intent: RetirementIntent::Cancel,
    };
    assert!(encode_record(&cancel_handoff).is_err());
}

#[test]
fn stage_complete_clamps_progress_to_the_authoritative_total() {
    // A completion total smaller than an over-reported prior StageProgress must
    // not leave bytes > total — a state the record codec rejects. The reducer
    // must never author a record its own codec refuses.
    let mut record = preparing(Direction::Send, true);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(80),
        })
        .unwrap();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            total: ByteCount::new(50),
        })
        .unwrap();
    assert_eq!(record.total, ByteCount::new(50));
    assert_eq!(
        record.bytes,
        ByteCount::new(50),
        "progress is clamped to the authoritative completion total"
    );
    assert!(
        encode_record(&record).is_ok(),
        "the reduced staging record must be encodable by its own codec"
    );
}

#[test]
fn admitted_progress_is_monotone_within_a_generation() {
    // An untrusted executor event must not move the bar — and thus the next
    // ResumeFrom offset — backward, which would make a valid larger durable peer
    // prefix look like a protocol violation on resume.
    let mut record = transfer(Direction::Send);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(80),
            },
        ))
        .unwrap();
    assert_eq!(record.bytes, ByteCount::new(80));
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(20),
            },
        ))
        .unwrap();
    assert_eq!(
        record.bytes,
        ByteCount::new(80),
        "backward progress ignored"
    );

    record
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    quiesce(&mut record, RetirementIntent::Pause);
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(
        start_plan(&effects).resume,
        ResumeIntent::ResumeFrom {
            offset: ByteCount::new(80)
        }
    );
}

#[test]
fn restore_does_not_affirm_an_ambiguous_pause() {
    // A pause requested while an attempt was live may have LOST to a crossed
    // commit and actually completed. Restore must not affirm it as a clean local
    // pause: a fully-sent send goes to the mailbox; otherwise Paused(Lost).
    let mut receive = transfer(Direction::Receive);
    receive
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    let encoded = encode_record(&receive).unwrap();
    let RecordDecode::Loaded(mut restored) = decode_record(&encoded).unwrap() else {
        panic!("paused record must load");
    };
    assert!(restored.reduce(ProductInput::Restore).unwrap().is_empty());
    assert_eq!(restored.state, ProductState::Paused(PauseOrigin::Lost));
    assert_eq!(restored.quiescence, crate::Quiescence::Quiescent);

    // A send that had already sent Complete instead defers to the mailbox proof.
    let mut send = confirming_send();
    send.reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    assert!(send.facts.complete_sent);
    let effects = send.reduce(ProductInput::Restore).unwrap();
    assert_eq!(send.state, ProductState::Unconfirmed);
    assert_eq!(send.quiescence, crate::Quiescence::Quiescent);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, ProductEffect::StartMailboxPoll { .. }))
    );
}

#[test]
fn restore_reissues_the_tombstone_for_a_removed_record() {
    // Topology F2: a crash after committing a removal but before dispatching the
    // `TombstoneCard` leaves a `remove_requested + Quiescent` record. Restore must
    // re-issue the tombstone idempotently, not leave a command-less zombie.
    let mut record = transfer(Direction::Send);
    record
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    quiesce(&mut record, RetirementIntent::Pause);
    let removed = record
        .reduce(ProductInput::Command(ProductCommand::Remove))
        .unwrap();
    assert!(record.facts.remove_requested);
    assert!(removed.iter().any(|e| matches!(
        e,
        ProductEffect::StorageIntent {
            action: StorageAction::TombstoneCard,
            ..
        }
    )));
    assert!(record.allowed_commands().is_empty());

    // The tombstone effect was lost to a crash; restore re-issues it.
    let replay = record.reduce(ProductInput::Restore).unwrap();
    assert!(
        replay.iter().any(|e| matches!(
            e,
            ProductEffect::StorageIntent {
                action: StorageAction::TombstoneCard,
                ..
            }
        )),
        "restore re-issues the tombstone for an extant removed record"
    );
}

#[test]
fn legal_commands_come_only_from_product_state() {
    let active = ready(Direction::Send);
    assert_eq!(
        active.allowed_commands(),
        vec![
            ProductCommand::Pause,
            ProductCommand::Cancel,
            ProductCommand::Remove,
        ]
    );

    let mut needs_repick = preparing(Direction::Send, false);
    let stamp = needs_repick.stamp();
    needs_repick
        .reduce(ProductInput::StageFailed { stamp })
        .unwrap();
    // The staging worker must retire before the failed card is at rest and can
    // offer its recovery commands.
    needs_repick
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(
        needs_repick.allowed_commands(),
        vec![ProductCommand::RePickSource, ProductCommand::Remove]
    );

    let mut removed = ready(Direction::Send);
    removed
        .reduce(ProductInput::Command(ProductCommand::Remove))
        .unwrap();
    assert!(removed.allowed_commands().is_empty());
}

/// The offer is DERIVED from the handlers, so the biconditional sweep below
/// cannot catch a handler whose guard is simply wrong — a broken `on_pause`
/// would withhold Pause and stay perfectly self-consistent. This pins the
/// policy itself: the exact list, in order, for every distinct offer the
/// authority makes, on records built by real reductions.
#[test]
fn the_offer_at_each_resting_state_is_pinned() {
    let mut needs_repick = preparing(Direction::Send, false);
    let repick_stamp = needs_repick.stamp();
    needs_repick
        .reduce(ProductInput::StageFailed {
            stamp: repick_stamp,
        })
        .unwrap();
    needs_repick
        .reduce(ProductInput::StagingRetired {
            stamp: repick_stamp,
        })
        .unwrap();

    let mut paused = ready(Direction::Send);
    paused
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    quiesce(&mut paused, RetirementIntent::Pause);

    let mut completed = ready(Direction::Receive);
    completed
        .reduce(event(
            &completed,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    completed
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &completed,
            RetirementIntent::Finalize,
            true,
        )))
        .unwrap();

    let mut cancelled = ready(Direction::Send);
    cancelled
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    quiesce(&mut cancelled, RetirementIntent::Cancel);

    let mut retiring = ready(Direction::Send);
    retiring
        .reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();

    let mut removed = ready(Direction::Send);
    removed
        .reduce(ProductInput::Command(ProductCommand::Remove))
        .unwrap();

    let expected: [(&str, TransferRecord, Vec<ProductCommand>); 8] = [
        (
            "connecting",
            ready(Direction::Send),
            vec![
                ProductCommand::Pause,
                ProductCommand::Cancel,
                ProductCommand::Remove,
            ],
        ),
        (
            "preparing",
            preparing(Direction::Send, true),
            vec![ProductCommand::Cancel, ProductCommand::Remove],
        ),
        (
            "needs a re-pick",
            needs_repick,
            vec![ProductCommand::RePickSource, ProductCommand::Remove],
        ),
        (
            "paused",
            paused,
            vec![
                ProductCommand::Resume,
                ProductCommand::Cancel,
                ProductCommand::Remove,
            ],
        ),
        ("completed", completed, vec![ProductCommand::Remove]),
        (
            "cancelled",
            cancelled,
            vec![ProductCommand::Resume, ProductCommand::Remove],
        ),
        ("retiring", retiring, Vec::new()),
        ("removed", removed, Vec::new()),
    ];

    for (what, record, offer) in &expected {
        assert_eq!(record.allowed_commands(), *offer, "the offer for {what}");
    }
}

/// F2a publishes `allowed_commands` in the read contract so a frontend renders
/// the authority's own legality instead of re-deriving one (R0). That makes
/// this list a PROMISE rather than a hint, and a promise has two halves: a
/// command it offers must do something, and one it withholds must do nothing.
///
/// Without the first half a frontend draws a button whose command is accepted,
/// committed, and changes nothing — the most expensive way to say no. Without
/// the second half the offer is not the boundary it claims to be.
///
/// The sweep is exhaustive over the CONSTRUCTIBLE record space — every field a
/// command handler reads, in every combination, not only the ones a reduction
/// can reach. That matters because `decode_record` validates three invariants
/// and none of them constrains `state` against `quiescence`, so every shape
/// below can arrive from durable storage. Against the hand-written offer this
/// list replaced, 17,056 of these checks disagreed; the offer is now derived
/// from the handlers, so agreement is by construction and what this sweep
/// still proves is that the derivation is total (no record makes it panic),
/// bounded, order-stable, and non-vacuous.
#[test]
fn every_published_command_moves_the_card_and_the_rest_are_inert() {
    let commands = [
        ProductCommand::Pause,
        ProductCommand::Resume,
        ProductCommand::RePickSource,
        ProductCommand::Cancel,
        ProductCommand::Remove,
    ];
    // The offer filters `ALL_COMMANDS`, so a command missing from it could
    // never be offered however its handler behaves — and the published order
    // is the order a frontend renders.
    assert_eq!(crate::reducer::ALL_COMMANDS, commands);

    let states = [
        ProductState::Preparing,
        ProductState::Waiting,
        ProductState::Connecting,
        ProductState::Verifying,
        ProductState::Transferring,
        ProductState::Confirming,
        ProductState::Paused(PauseOrigin::Local),
        ProductState::Paused(PauseOrigin::Peer),
        ProductState::Paused(PauseOrigin::Lost),
        ProductState::Unconfirmed,
        ProductState::Completed,
        ProductState::Failed,
        ProductState::Cancelled,
    ];
    let quiescences = [
        Quiescence::Running {
            worker: WorkerKind::Attempt,
        },
        Quiescence::Running {
            worker: WorkerKind::Staging,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Pause,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Cancel,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Finalize,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent: RetirementIntent::Pause,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent: RetirementIntent::Cancel,
        },
        Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent: RetirementIntent::Finalize,
        },
        Quiescence::Quiescent,
    ];
    // Legality reads `retry` and `recovery`; the rest of an outcome is display.
    let mut outcomes: Vec<Option<Outcome>> = vec![None];
    for retry in [
        Retryability::Retryable,
        Retryability::Terminal,
        Retryability::NeedsUser,
    ] {
        for recovery in [
            None,
            Some(Recovery::RePickSource),
            Some(Recovery::RetryLater),
            Some(Recovery::ReconnectPeer),
        ] {
            let outcome = Outcome::new(
                OutcomeCode::Internal,
                Phase::Transferring,
                retry,
                SafeDisplay::new("swept"),
            );
            outcomes.push(Some(match recovery {
                Some(recovery) => outcome.with_recovery(recovery),
                None => outcome,
            }));
        }
    }

    let base = ready(Direction::Send);
    let mut swept = 0usize;
    let mut offered = [0usize; 5];
    let mut withheld = [0usize; 5];
    for state in states {
        for quiescence in quiescences {
            // All five facts, so a fact that starts gating a command tomorrow is
            // already inside the sweep.
            for bits in 0..32u8 {
                for source_recoverable in [false, true] {
                    for outcome in &outcomes {
                        let mut record = base.clone();
                        record.state = state;
                        record.quiescence = quiescence;
                        record.facts = Facts {
                            source_ready: bits & 1 != 0,
                            complete_sent: bits & 2 != 0,
                            proof_delivered: bits & 4 != 0,
                            receipt_mismatch: bits & 8 != 0,
                            remove_requested: bits & 16 != 0,
                        };
                        record.source_recoverable = source_recoverable;
                        record.outcome = outcome.clone();
                        swept += 1;

                        let allowed = record.allowed_commands();
                        // The published field is `list(CommandKindView, 5)`, and
                        // a repeated affordance would draw twice.
                        assert!(allowed.len() <= commands.len());
                        assert!(
                            allowed.is_sorted_by_key(|command| commands
                                .iter()
                                .position(|published| published == command)),
                            "the offer is not in published order: {allowed:?}"
                        );

                        for (slot, command) in commands.into_iter().enumerate() {
                            let mut candidate = record.clone();
                            let effects = candidate
                                .reduce(ProductInput::Command(command))
                                .expect("a command reduces");
                            let moved = candidate != record || !effects.is_empty();
                            assert_eq!(
                                allowed.contains(&command),
                                moved,
                                "{command:?} is offered={} but moves the card={moved} for \
                                 {state:?} / {quiescence:?} / {:?} / {outcome:?}",
                                allowed.contains(&command),
                                record.facts,
                            );
                            if moved {
                                offered[slot] += 1;
                            } else {
                                withheld[slot] += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    // Vacuity: a sweep that never offers a command, or never withholds one,
    // satisfies that command's half of the biconditional while proving nothing
    // about it. Every command must appear on both sides.
    assert_eq!(swept, 97_344, "the constructible space changed shape");
    for (slot, command) in commands.into_iter().enumerate() {
        assert!(offered[slot] > 0, "{command:?} is never offered");
        assert!(withheld[slot] > 0, "{command:?} is never withheld");
    }
}

#[test]
fn receipt_delivery_result_requires_exact_provenance() {
    let mut record = transfer(Direction::Receive);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    // The receipt duty is posted once the attempt retires (its commit crossed).
    let posted = record
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &record,
            RetirementIntent::Finalize,
            true,
        )))
        .unwrap();
    let provenance = posted
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::CapabilityDuty { duty, .. } => Some(duty.provenance),
            _ => None,
        })
        .expect("receipt duty");
    let stale = DutyProvenance {
        generation: AttemptGen::new(provenance.generation.get() + 1),
        ..provenance
    };
    record
        .reduce(ProductInput::ReceiptPosted(admitted_duty_result(
            stale,
            OutcomeCode::Completed,
        )))
        .unwrap();
    assert!(!record.facts.proof_delivered);
    record
        .reduce(ProductInput::ReceiptPosted(admitted_duty_result(
            provenance,
            OutcomeCode::NetworkUnreachable,
        )))
        .unwrap();
    assert!(!record.facts.proof_delivered);
    record
        .reduce(ProductInput::ReceiptPosted(admitted_duty_result(
            provenance,
            OutcomeCode::Completed,
        )))
        .unwrap();
    assert!(record.facts.proof_delivered);
    record.reduce(ProductInput::Restore).unwrap();
    assert!(record.facts.proof_delivered);
}

#[test]
fn stale_attempt_events_are_inert() {
    let mut record = transfer(Direction::Receive);
    record
        .reduce(event(
            &record,
            AttemptEventKind::Terminal(OutcomeCode::PeerLost),
        ))
        .unwrap();
    record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    let stale = AttemptStamp {
        card: record.identity.card,
        generation: AttemptGen::new(record.generation.get() - 1),
    };
    let snapshot = record.clone();
    for kind in [
        AttemptEventKind::Phase(Phase::Pairing),
        AttemptEventKind::Phase(Phase::Authenticating),
        AttemptEventKind::Phase(Phase::Transferring),
        AttemptEventKind::Progress {
            transferred: ByteCount::new(99),
        },
        AttemptEventKind::Phase(Phase::Confirming),
        AttemptEventKind::Terminal(OutcomeCode::Completed),
        AttemptEventKind::Terminal(OutcomeCode::Cancelled),
        AttemptEventKind::Terminal(OutcomeCode::PeerLost),
    ] {
        let input = admitted_event(&record, stale, kind);
        assert!(record.reduce(input).unwrap().is_empty());
        assert_eq!(record, snapshot);
    }
    for input in [
        ProductInput::Advertised { stamp: stale },
        ProductInput::VerificationStarted { stamp: stale },
        ProductInput::VerificationFinished { stamp: stale },
        ProductInput::AttemptEnded { stamp: stale },
        ProductInput::ConfirmTimeout { stamp: stale },
        ProductInput::ReceiptVerified { stamp: stale },
        ProductInput::ReceiptMismatch { stamp: stale },
    ] {
        assert!(record.reduce(input).unwrap().is_empty());
        assert_eq!(record, snapshot);
    }
}

#[test]
fn random_interleavings_hold_invariants() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    for direction in [Direction::Send, Direction::Receive] {
        let mut record = ready(direction);
        for _ in 0..5_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let current = record.stamp();
            let stamp = if seed & 1 == 0 {
                current
            } else {
                stale_stamp(&record)
            };
            let input = match (seed >> 33) as usize % 16 {
                0 => ProductInput::Command(ProductCommand::Pause),
                1 => ProductInput::Command(ProductCommand::Cancel),
                2 => ProductInput::Command(ProductCommand::Resume),
                3 => ProductInput::ConfirmTimeout { stamp },
                4 => ProductInput::ReceiptVerified { stamp },
                5 => ProductInput::ReceiptMismatch { stamp },
                6 => ProductInput::Advertised { stamp },
                7 => admitted_event(&record, stamp, AttemptEventKind::Phase(Phase::Transferring)),
                8 => admitted_event(
                    &record,
                    stamp,
                    AttemptEventKind::Progress {
                        transferred: ByteCount::new(50),
                    },
                ),
                9 => admitted_event(&record, stamp, AttemptEventKind::Phase(Phase::Confirming)),
                10 => admitted_event(
                    &record,
                    stamp,
                    AttemptEventKind::Terminal(OutcomeCode::Completed),
                ),
                11 => admitted_event(
                    &record,
                    stamp,
                    AttemptEventKind::Terminal(OutcomeCode::PeerLost),
                ),
                12 => ProductInput::AttemptEnded { stamp },
                13 => ProductInput::StorageFailed,
                14 => ProductInput::AttemptRetired(retirement_ack(
                    &record,
                    RetirementIntent::Cancel,
                    false,
                )),
                15 => ProductInput::StagingRetired { stamp },
                _ => unreachable!(),
            };
            record.reduce(input).unwrap();
            assert!(
                record.total.get() == 0 || record.bytes.get() <= record.total.get(),
                "bytes exceed total: {record:?}"
            );
        }
    }
}

#[test]
fn product_model_scenario_trace() {
    let (mut send, create_effects) = create(Direction::Send, SourceDecision::Ready);
    assert_eq!(start_plan(&create_effects).resume, ResumeIntent::Fresh);
    send.reduce(event(&send, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    send.reduce(event(
        &send,
        AttemptEventKind::Progress {
            transferred: ByteCount::new(60),
        },
    ))
    .unwrap();
    send.reduce(ProductInput::Command(ProductCommand::Pause))
        .unwrap();
    assert_eq!(send.state, ProductState::Paused(PauseOrigin::Local));
    // The pause reaches rest only when its attempt acks the retirement; only
    // then can resume launch the next generation.
    quiesce(&mut send, RetirementIntent::Pause);
    let effects = send
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(
        start_plan(&effects).resume,
        ResumeIntent::ResumeFrom {
            offset: ByteCount::new(60),
        }
    );
    send.reduce(event(&send, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    let effects = send
        .reduce(event(&send, AttemptEventKind::Phase(Phase::Confirming)))
        .unwrap();
    assert_eq!(effects.len(), 2, "timer and mailbox wait in parallel");
    let effects = send
        .reduce(ProductInput::ReceiptVerified {
            stamp: send.stamp(),
        })
        .unwrap();
    assert_eq!(send.state, ProductState::Completed);
    assert_eq!(effects.len(), 3, "stop both waits and retire the attempt");

    let mut receive = transfer(Direction::Receive);
    let effects = receive
        .reduce(event(
            &receive,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(receive.state, ProductState::Completed);
    // The terminal only finalizes the attempt; the receipt duty is DEFERRED
    // behind the retirement ack (committed effects + quiescence).
    assert_eq!(
        effects,
        vec![ProductEffect::RetireAttempt {
            stamp: receive.stamp(),
            intent: RetirementIntent::Finalize,
        }]
    );
    let posted = receive
        .reduce(ProductInput::AttemptRetired(retirement_ack(
            &receive,
            RetirementIntent::Finalize,
            true,
        )))
        .unwrap();
    assert!(
        posted
            .iter()
            .any(|effect| matches!(effect, ProductEffect::CapabilityDuty { .. }))
    );

    let mut cancelled = transfer(Direction::Send);
    cancelled
        .reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    // Optimistically Cancelled but still Retiring: bytes are kept until the ack.
    assert_eq!(cancelled.state, ProductState::Cancelled);
    assert!(cancelled.quiescence.is_retiring());
    assert_eq!(cancelled.bytes, ByteCount::new(40));
    // The retirement ack confirms the cancel: now the bytes clear and discard.
    quiesce(&mut cancelled, RetirementIntent::Cancel);
    assert_eq!(cancelled.bytes, ByteCount::new(0));

    let mut storage_failed = transfer(Direction::Receive);
    let effects = storage_failed.reduce(ProductInput::StorageFailed).unwrap();
    assert_eq!(storage_failed.state, ProductState::Failed);
    assert!(matches!(
        effects.as_slice(),
        [ProductEffect::RetireAttempt {
            intent: RetirementIntent::Cancel,
            ..
        }]
    ));
}

/// Creating a card that still needs a source ASKS for one, and the ask is a
/// post-commit effect: the record exists before the picker is ever consulted
/// (`SF02`). A card whose source is already ready asks for nothing, because
/// there is nothing to ask for.
#[test]
fn a_card_that_needs_a_source_asks_the_platform_for_one() {
    let (record, effects) = create(
        Direction::Send,
        SourceDecision::Stage { recoverable: false },
    );
    assert_eq!(record.state, ProductState::Preparing);
    let [ProductEffect::CapabilityDuty { duty, action }] = effects.as_slice() else {
        panic!("a staged create asks for a source, got {effects:?}");
    };
    assert_eq!(*action, CapabilityAction::SelectSource);
    assert_eq!(duty.kind, DutyKind::SourceHandle);
    assert_eq!(duty.provenance.card, record.identity.card);
    assert_eq!(duty.provenance.generation, record.generation);

    // A duty is world-facing, so the commit barrier holds it until the record
    // is durable — the whole reason the picker cannot be what decides a
    // transfer exists.
    let (session, outcome) = crate::CommittedSession::create_without_store(
        NewTransfer {
            direction: Direction::Send,
            participation: crate::RoomParticipation::Minted,
            offered_name: OfferedName::from_untrusted("a.zip").unwrap(),
            total: ByteCount::new(100),
            source: SourceDecision::Stage { recoverable: false },
            pairing: None,
        },
        &mut DeterministicEntropy::default(),
    )
    .expect("deterministic identity source");
    assert_eq!(session.record().state, ProductState::Preparing);
    assert!(outcome.released_immediately.is_empty());
    assert_eq!(outcome.released_after_commit.len(), 1);

    // A ready source has nothing to pick, and a card created needing a re-pick
    // is already failed — asking would be asking on behalf of a dead card.
    for source in [SourceDecision::Ready, SourceDecision::NeedsRepick] {
        let (_, effects) = create(Direction::Send, source);
        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                ProductEffect::CapabilityDuty {
                    action: CapabilityAction::SelectSource,
                    ..
                }
            )),
            "{source:?} asked for a source it does not need"
        );
    }
}

/// The re-pick command is the recovery `RS04` says the old app stranded users
/// without. It now actually asks, under a FRESH generation — so the C6 ledger
/// sees a new duty rather than one it has already discharged.
#[test]
fn re_picking_a_source_asks_again_under_a_new_generation() {
    let mut record = preparing(Direction::Send, false);
    let stamp = record.stamp();
    let _ = record.reduce(ProductInput::StageFailed { stamp });
    let _ = record.reduce(ProductInput::StagingRetired { stamp });
    assert_eq!(record.state, ProductState::Failed);
    assert_eq!(
        record.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );

    let effects = record
        .reduce(ProductInput::Command(ProductCommand::RePickSource))
        .expect("the re-pick reduces");
    let [ProductEffect::CapabilityDuty { duty, action }] = effects.as_slice() else {
        panic!("a re-pick asks for a source, got {effects:?}");
    };
    assert_eq!(*action, CapabilityAction::SelectSource);
    assert_eq!(duty.provenance.generation, record.generation);
    assert_ne!(duty.provenance.generation, stamp.generation);
    assert_eq!(record.state, ProductState::Preparing);
}

/// The two duties one card can raise must never share a provenance: the C6
/// ledger keys discharge by provenance, so a collision would make one admitted
/// result answer for the other. The tag is a constant, so this is exhaustive
/// over every identity the minter can produce.
#[test]
fn the_source_and_receipt_duties_never_share_an_identity() {
    let mut seen = std::collections::HashSet::new();
    for seed in 0..64u8 {
        let mut record = ready(Direction::Send);
        record.receipt_request = RequestId::from_bytes([seed; 16]);
        let source = record.source_request();
        assert_ne!(
            source, record.receipt_request,
            "the source duty reused the receipt's identity"
        );
        assert!(seen.insert(source), "two receipts produced one source id");
    }
}

/// A card's channel survives the record codec and re-encodes to the invite it
/// came from, so a restarted app publishes the same invite it published before.
#[test]
fn a_cards_channel_survives_the_record_and_re_encodes_to_its_invite() {
    let invite = envoix_invite::Invite::new(
        "000123-amber-brass",
        "rendezvous.example",
        "relay.example",
        envoix_invite::Role::Send,
    )
    .expect("a well-formed invite");
    let link = envoix_invite::encode_deep_link(&invite).expect("the invite encodes");

    let (record, _) = TransferRecord::create(
        NewTransfer {
            direction: Direction::Send,
            participation: crate::RoomParticipation::Minted,
            offered_name: OfferedName::from_untrusted("a.zip").unwrap(),
            total: ByteCount::new(100),
            source: SourceDecision::Stage { recoverable: false },
            pairing: Some(Box::new(crate::PairingChannel::from_invite(&invite))),
        },
        &mut DeterministicEntropy::default(),
    )
    .expect("deterministic identity source");

    let encoded = encode_record(&record).expect("the record encodes");
    let RecordDecode::Loaded(restored) = decode_record(&encoded).expect("the record decodes")
    else {
        panic!("a freshly written record is not a future version");
    };
    let channel = restored.pairing.expect("the channel survived");
    assert_eq!(channel.code(), "000123-amber-brass");
    assert_eq!(channel.shareable(), Some(link));
    // The invite is re-derived, never stored twice: parsing the published text
    // yields the channel it was published from.
    let round_tripped = envoix_invite::route_invite(&channel.shareable().unwrap())
        .expect("the published invite parses");
    assert_eq!(round_tripped, invite);
}

fn fixture_record() -> TransferRecord {
    TransferRecord {
        identity: ProductIdentity {
            card: RecordId::new(1),
            transfer: TransferId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            artifact: ArtifactId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
        },
        direction: Direction::Send,
        source: crate::SourceLifecycle::initial(Direction::Send),
        participation: crate::RoomParticipation::Minted,
        offered_name: OfferedName::from_untrusted("a.txt").unwrap(),
        total: ByteCount::new(10),
        state: ProductState::Paused(PauseOrigin::Local),
        quiescence: crate::Quiescence::Quiescent,
        generation: AttemptGen::new(7),
        phase: Phase::Transferring,
        bytes: ByteCount::new(4),
        bytes_resumed: ByteCount::new(2),
        outcome: None,
        facts: crate::Facts {
            source_ready: true,
            complete_sent: false,
            proof_delivered: false,
            receipt_mismatch: false,
            remove_requested: false,
        },
        source_recoverable: true,
        pairing: None,
        create_request_id: None,
        receipt_request: RequestId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
        command_ledger: crate::CommandLedger::default(),
    }
}

#[test]
fn product_record_roundtrips() {
    let record = fixture_record();
    let encoded = encode_record(&record).unwrap();
    assert_eq!(
        decode_record(&encoded).unwrap(),
        RecordDecode::Loaded(Box::new(record))
    );
}

#[test]
fn product_record_v5_has_a_byte_exact_fixture() {
    let body = br#"{"identity":{"card":1,"transfer":"00000000000000000000000000000002","artifact":"00000000000000000000000000000003"},"direction":"send","offered_name":"a.txt","total":10,"state":{"state":"paused","origin":"local"},"quiescence":{"status":"quiescent"},"generation":7,"phase":"transferring","bytes":4,"bytes_resumed":2,"outcome":null,"facts":{"source_ready":true,"complete_sent":false,"proof_delivered":false,"receipt_mismatch":false,"remove_requested":false},"source_recoverable":true,"source":{"awaiting_selection":{"gate":{"selectable":{"reason":"initial"}}}},"participation":"minted","pairing":null,"create_request_id":null,"receipt_request":"00000000000000000000000000000004","command_ledger":[]}"#;
    let mut expected = Vec::new();
    expected.extend_from_slice(&23_u16.to_be_bytes());
    expected.extend_from_slice(b"envoix/product-record/1");
    expected.extend_from_slice(&5_u32.to_be_bytes());
    expected.extend_from_slice(&(body.len() as u32).to_be_bytes());
    expected.extend_from_slice(body);
    assert_eq!(encode_record(&fixture_record()).unwrap(), expected);
}

/// Every pre-v5 record is REFUSED, not migrated.
///
/// v1-v4 predate `TransferRecord::source`, and there is no honest default for
/// it: a receiver decoded as `AwaitingSelection` would ask for a document it
/// must never have, and a sender defaulted to `NotRequired` would claim it
/// needs none. A defaulted field that changes what a card IS is a fabrication,
/// not a migration — so the honest answer is a typed refusal the caller
/// quarantines. Nothing has ever been released, so no real record is affected.
#[test]
fn every_pre_v5_record_is_refused_rather_than_defaulted() {
    let body = br#"{"identity":{"card":1,"transfer":"00000000000000000000000000000002","artifact":"00000000000000000000000000000003"},"direction":"send","offered_name":"a.txt","total":10,"state":{"state":"paused","origin":"local"},"quiescence":{"status":"quiescent"},"generation":7,"phase":"transferring","bytes":4,"bytes_resumed":2,"outcome":null,"facts":{"source_ready":true,"complete_sent":false,"proof_delivered":false,"receipt_mismatch":false,"remove_requested":false},"source_recoverable":true,"receipt_request":"00000000000000000000000000000004"}"#;
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&23_u16.to_be_bytes());
    encoded.extend_from_slice(b"envoix/product-record/1");
    for version in 1_u32..crate::record::PRODUCT_RECORD_VERSION {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&23_u16.to_be_bytes());
        encoded.extend_from_slice(b"envoix/product-record/1");
        encoded.extend_from_slice(&version.to_be_bytes());
        encoded.extend_from_slice(&(body.len() as u32).to_be_bytes());
        encoded.extend_from_slice(body);
        assert_eq!(
            decode_record(&encoded),
            Err(RecordCodecError::UnsupportedVersion { actual: version }),
            "v{version} must be refused, never defaulted into a v5 shape"
        );
    }
}

#[test]
fn product_record_future_version_is_quarantinable() {
    let mut encoded = encode_record(&fixture_record()).unwrap();
    let version_offset = 2 + b"envoix/product-record/1".len();
    let future = crate::PRODUCT_RECORD_VERSION + 1;
    encoded[version_offset..version_offset + 4].copy_from_slice(&future.to_be_bytes());
    assert_eq!(
        decode_record(&encoded).unwrap(),
        RecordDecode::UnsupportedFuture { version: future }
    );
}

#[test]
fn product_record_rejects_corruption_and_v0() {
    let encoded = encode_record(&fixture_record()).unwrap();
    assert_eq!(
        decode_record(&encoded[..encoded.len() - 1]),
        Err(RecordCodecError::LengthMismatch)
    );

    let mut malformed = encoded.clone();
    let body_offset = 2 + b"envoix/product-record/1".len() + 4 + 4;
    malformed[body_offset] = b'[';
    assert_eq!(
        decode_record(&malformed),
        Err(RecordCodecError::MalformedBody)
    );

    let mut v0 = encoded;
    let version_offset = 2 + b"envoix/product-record/1".len();
    v0[version_offset..version_offset + 4].copy_from_slice(&0_u32.to_be_bytes());
    assert_eq!(
        decode_record(&v0),
        Err(RecordCodecError::UnsupportedVersion { actual: 0 })
    );
}

#[test]
fn product_record_contains_its_schema_identifier() {
    let encoded = encode_record(&fixture_record()).unwrap();
    assert!(
        encoded
            .windows(23)
            .any(|window| window == b"envoix/product-record/1")
    );
}

#[test]
fn state_json_names_cover_the_product_vocabulary() {
    let cases = [
        (ProductState::Preparing, "preparing"),
        (ProductState::Waiting, "waiting"),
        (ProductState::Connecting, "connecting"),
        (ProductState::Verifying, "verifying"),
        (ProductState::Transferring, "transferring"),
        (ProductState::Confirming, "confirming"),
        (ProductState::Unconfirmed, "unconfirmed"),
        (ProductState::Completed, "completed"),
        (ProductState::Failed, "failed"),
        (ProductState::Cancelled, "cancelled"),
    ];
    for (state, expected) in cases {
        let value = serde_json::to_value(state).unwrap();
        assert_eq!(value["state"], expected, "{state:?}");
    }
    let paused = serde_json::to_value(ProductState::Paused(PauseOrigin::Peer)).unwrap();
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["origin"], "peer");
}

/// A reused identity answers its disposition only for the SAME command; a
/// different command is a typed conflict, and pruning keeps the newest
/// [`CommandLedger::RETENTION`] entries.
#[test]
fn command_ledger_conflicts_and_prunes() {
    let mut ledger = crate::CommandLedger::default();
    let reused = CommandId::from_bytes([0xAA; 16]);
    ledger.record(
        reused,
        ProductCommand::Pause,
        ProductState::Paused(PauseOrigin::Local),
    );
    assert_eq!(
        ledger.disposition(reused, ProductCommand::Pause),
        Some(crate::LedgerHit::Duplicate {
            state: ProductState::Paused(PauseOrigin::Local)
        })
    );
    assert_eq!(
        ledger.disposition(reused, ProductCommand::Cancel),
        Some(crate::LedgerHit::Conflict {
            applied: ProductCommand::Pause
        })
    );

    for n in 0..crate::CommandLedger::RETENTION {
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
        ledger.record(
            CommandId::from_bytes(id),
            ProductCommand::Resume,
            ProductState::Waiting,
        );
    }
    assert_eq!(ledger.len(), crate::CommandLedger::RETENTION);
    // The oldest entry (the reused id) was pruned; a re-issue is fresh again.
    assert_eq!(ledger.disposition(reused, ProductCommand::Pause), None);
    let mut newest = [0u8; 16];
    newest[..8].copy_from_slice(&(crate::CommandLedger::RETENTION as u64).to_be_bytes());
    assert!(
        ledger
            .disposition(CommandId::from_bytes(newest), ProductCommand::Resume)
            .is_some()
    );
}
