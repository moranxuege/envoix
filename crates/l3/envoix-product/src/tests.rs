use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, ResumeIntent, RetirementAck, RetirementAckResult, RetirementIntent,
};
use envoix_capabilities::{
    Admission, DutyKind, DutyLedger, DutyProvenance, DutyResult, GenerationUpdate, Registration,
};
use envoix_outcomes::{OutcomeCode, Phase, Recovery};
use envoix_types::{
    ArtifactId, AttemptGen, ByteCount, Direction, OfferedName, RecordId, RequestId, TransferId,
};

use crate::{
    CapabilityAction, IdentityError, IdentitySource, NewTransfer, PauseOrigin, ProductCommand,
    ProductEffect, ProductIdentity, ProductInput, ProductState, RecordCodecError, RecordDecode,
    SourceDecision, StorageAction, TransferRecord, decode_record, encode_record, resolve_source,
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
            offered_name: OfferedName::from_untrusted("a.zip"),
            total: ByteCount::new(100),
            source,
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
            offered_name: OfferedName::from_untrusted("a.zip"),
            total: ByteCount::new(1),
            source: SourceDecision::Ready,
        },
        &mut UnavailableEntropy,
    )
    .expect_err("identity creation must fail closed");
    assert_eq!(error, IdentityError::EntropyUnavailable);
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
    // Restore asks the (now-defunct) attempt to retire AND starts the mailbox
    // poll: the in-band confirmation is abandoned for the durable proof channel.
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

    let mut active = transfer(Direction::Receive);
    active.reduce(ProductInput::Restore).unwrap();
    assert_eq!(active.state, ProductState::Paused(PauseOrigin::Lost));

    let mut preparing = preparing(Direction::Send, false);
    preparing.reduce(ProductInput::Restore).unwrap();
    assert_eq!(preparing.state, ProductState::Failed);
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
    assert_eq!(
        completed.phase, phase,
        "restore preserves terminal evidence"
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
            let input = match (seed >> 33) as usize % 14 {
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

fn fixture_record() -> TransferRecord {
    TransferRecord {
        identity: ProductIdentity {
            card: RecordId::new(1),
            transfer: TransferId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            artifact: ArtifactId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
        },
        direction: Direction::Send,
        offered_name: OfferedName::from_untrusted("a.txt"),
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
        receipt_request: RequestId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
    }
}

#[test]
fn product_record_v1_roundtrips() {
    let record = fixture_record();
    let encoded = encode_record(&record).unwrap();
    assert_eq!(
        decode_record(&encoded).unwrap(),
        RecordDecode::Loaded(record)
    );
}

#[test]
fn product_record_v1_has_a_byte_exact_fixture() {
    let body = br#"{"identity":{"card":1,"transfer":"00000000000000000000000000000002","artifact":"00000000000000000000000000000003"},"direction":"send","offered_name":"a.txt","total":10,"state":{"state":"paused","origin":"local"},"quiescence":{"status":"quiescent"},"generation":7,"phase":"transferring","bytes":4,"bytes_resumed":2,"outcome":null,"facts":{"source_ready":true,"complete_sent":false,"proof_delivered":false,"receipt_mismatch":false,"remove_requested":false},"source_recoverable":true,"receipt_request":"00000000000000000000000000000004"}"#;
    let mut expected = Vec::new();
    expected.extend_from_slice(&23_u16.to_be_bytes());
    expected.extend_from_slice(b"envoix/product-record/1");
    expected.extend_from_slice(&1_u32.to_be_bytes());
    expected.extend_from_slice(&(body.len() as u32).to_be_bytes());
    expected.extend_from_slice(body);
    assert_eq!(encode_record(&fixture_record()).unwrap(), expected);
}

#[test]
fn product_record_future_version_is_quarantinable() {
    let mut encoded = encode_record(&fixture_record()).unwrap();
    let version_offset = 2 + b"envoix/product-record/1".len();
    encoded[version_offset..version_offset + 4].copy_from_slice(&2_u32.to_be_bytes());
    assert_eq!(
        decode_record(&encoded).unwrap(),
        RecordDecode::UnsupportedFuture { version: 2 }
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
