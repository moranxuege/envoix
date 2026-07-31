use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, ResumeIntent, RetirementAck, RetirementAckResult, RetirementIntent,
};
use envoix_capabilities::{
    AcquiredItem, AcquiredSelection, Admission, DutyKind, DutyLedger, DutyProvenance, DutyReport,
    DutyResult, GenerationUpdate, Registration, SourceAcquisitionFailure, SourceAcquisitionKey,
    SourceReport, SourceRetention, SourceSeekability,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_protocol::ContentHash;
use envoix_types::{
    ArchivePath, ArtifactId, AttemptGen, ByteCount, CommandId, Direction, OfferedName,
    PeerContentDeclaration, RecordId, RequestId, SourceItemId, TransferId,
};

use crate::record::RecordInvariant;
use crate::test_support::{
    STAGED_NAME, STAGED_TOTAL, acquired, give_a_source, offer, sealed_artifact, settled, staged,
};
use crate::{
    AcceptedSourceOffer, CapabilityAction, Facts, IdentityError, IdentitySource, NewTransfer,
    PauseOrigin, PeerContentDecision, ProductCommand, ProductEffect, ProductIdentity, ProductInput,
    ProductState, Quiescence, RecordCodecError, RecordDecode, StorageAction, TransferRecord,
    WorkerKind, decode_record, encode_record,
};
use crate::{DerivationSpec, Selection, SourcePossession, StagingPlan};

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

/// A sending card that has been given a document and is staging it.
fn staging(direction: Direction) -> TransferRecord {
    let (mut record, _) = create(direction);
    give_a_source(&mut record);
    record
}

/// A sending card whose acquisition failed, so only a re-pick reopens it.
fn needs_repick() -> TransferRecord {
    let (mut record, _) = create(Direction::Send);
    let offered = ProductInput::SourceOffered {
        offer: offer(&record, STAGED_NAME, None),
    };
    record.reduce(offered).unwrap();
    let settlement = settled(
        &record,
        SourceReport::Failed(SourceAcquisitionFailure::Unreadable),
    );
    record.reduce(settlement).unwrap();
    record
}

/// A card whose source is established, reached the way the product reaches one.
///
/// A receiver needs none and is ready at creation. A sender is walked through
/// the whole acquisition — the picker answers, the platform acquires, staging
/// reports what it read, and the staging worker retires — because there is no
/// longer a shortcut that declares a source ready without one.
fn ready(direction: Direction) -> TransferRecord {
    launched(direction).0
}

/// As [`ready`], and also the effects that launched the first attempt — for
/// the tests that assert on the plan itself.
fn launched(direction: Direction) -> (TransferRecord, Vec<ProductEffect>) {
    if direction == Direction::Receive {
        return create(direction);
    }
    let mut record = staging(direction);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, STAGED_TOTAL),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    let effects = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    (record, effects)
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
        report: DutyReport::Outcome(outcome),
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

/// Every identity exists before ANY world-facing work is authorized.
///
/// A receiver's first work is the attempt. A sender commissions none at all —
/// it publishes an acquisition and waits — so what must already exist for it is
/// the acquisition's own identity, which is minted from the card and generation
/// the record was just given (`SF02`).
#[test]
fn product_mints_all_identity_before_the_first_work() {
    let (sender, effects) = create(Direction::Send);
    assert!(
        effects.is_empty(),
        "a sender commissioned work: {effects:?}"
    );
    assert_ne!(sender.identity.card.get(), 0);
    assert_ne!(sender.identity.transfer.to_bytes(), [0; 16]);
    assert_ne!(sender.identity.artifact.to_bytes(), [0; 16]);
    assert_ne!(sender.generation.get(), 0);
    let acquisition = sender.current_acquisition();
    assert_eq!(acquisition.card(), sender.identity.card);
    assert_eq!(acquisition.generation(), sender.generation);

    let (record, effects) = create(Direction::Receive);
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

/// The policy that replaced `resolve_source`: it reads what the PLATFORM
/// answered instead of what a caller decided, and it is total over every fact
/// it consults — how many documents, whether the grant survives, whether the
/// source can seek.
#[test]
fn a_send_streams_unless_something_forces_it_to_be_produced() {
    use crate::StagingPlan;
    let one = Selection::of_one(
        OfferedName::from_untrusted("a.txt").expect("a bounded name"),
        None,
    );
    assert_eq!(
        StagingPlan::for_selection(
            &one,
            &AcquiredSelection::of_one(SourceRetention::Persisted, SourceSeekability::Seekable)
        ),
        Some(StagingPlan::ProviderStream {
            item: SourceItemId::new(0)
        }),
        "the default: nothing produced, so a multi-gigabyte send does not double disk"
    );
    for (retention, seekability) in [
        (
            SourceRetention::Persisted,
            SourceSeekability::SequentialOnly,
        ),
        (SourceRetention::Process, SourceSeekability::Seekable),
        (SourceRetention::Process, SourceSeekability::SequentialOnly),
    ] {
        assert_eq!(
            StagingPlan::for_selection(&one, &AcquiredSelection::of_one(retention, seekability)),
            Some(StagingPlan::ProduceOwnedArtifact {
                derivation: DerivationSpec::VerbatimV1 {
                    item: SourceItemId::new(0)
                }
            }),
            "a grant a restart loses, or a source resume cannot re-read, must be produced"
        );
    }

    // Several documents have to be produced into ONE thing, and no archive
    // derivation exists — so there is no plan, rather than a plan that means
    // something else. The intake refuses such an offer for the same reason.
    let several = Selection::accept(vec![
        (
            ArchivePath::from_untrusted(["a.txt"]).expect("a path"),
            None,
        ),
        (
            ArchivePath::from_untrusted(["b.txt"]).expect("a path"),
            None,
        ),
    ])
    .expect("a selection");
    assert_eq!(
        StagingPlan::for_selection(
            &several,
            &AcquiredSelection::of(
                vec![
                    AcquiredItem {
                        item: SourceItemId::new(0),
                        retention: SourceRetention::Persisted,
                        seekability: SourceSeekability::Seekable,
                    },
                    AcquiredItem {
                        item: SourceItemId::new(1),
                        retention: SourceRetention::Persisted,
                        seekability: SourceSeekability::Seekable,
                    },
                ],
                2
            )
            .expect("the answers describe the selection")
        ),
        None,
        "a selection of several documents was given a plan that sends one"
    );
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
    assert_eq!(record.bytes, record.total());
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
    let mut record = staging(Direction::Send);
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
            content: staged(STAGED_NAME, 80),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert!(record.source_is_ready());
    assert_eq!(
        record.total(),
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
    let mut record = staging(Direction::Send);
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

/// Staging failure always needs the user, and says which failure it was.
///
/// There used to be a second, "retryable" flavour selected by a stored
/// `source_recoverable` boolean. It was unreachable truth: the card lands in
/// `RePickRequired`, which cannot accept an offer under the discharged key, so
/// "retry later" named a retry that could not work.
#[test]
fn stage_failed_fails_with_typed_source_recovery() {
    let mut recoverable = staging(Direction::Send);
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
        Some(Recovery::RePickSource)
    );
    // And the lifecycle says acquisition succeeded while READING failed, which
    // no acquisition failure can claim.
    let crate::SourceLifecycle::AwaitingSelection(gate) = &recoverable.source else {
        panic!("a failed staging returns the card to awaiting selection");
    };
    assert_eq!(gate.reason(), crate::SourcePromptReason::StagingFailed);
    assert!(!gate.accepts_an_offer());

    let mut needs_repick = needs_repick();
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
    let mut record = staging(Direction::Send);
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
    let mut record = staging(Direction::Send);
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
                content: staged(STAGED_NAME, 100),
                possession: SourcePossession::Streamed,
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(record, snapshot);
}

#[test]
fn stale_generation_staging_inputs_are_rejected_after_retry() {
    let mut record = staging(Direction::Send);
    let first = record.stamp();
    record
        .reduce(ProductInput::StageFailed { stamp: first })
        .unwrap();
    // The staging worker retires before the card can re-stage under a new gen.
    record
        .reduce(ProductInput::StagingRetired { stamp: first })
        .unwrap();
    // Re-pick is what advances the generation for a sourceless card, so the
    // acquisition key the stale worker answers to is discharged with it.
    record
        .reduce(ProductInput::Command(ProductCommand::RePickSource))
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert_ne!(record.stamp(), first);
    // A fresh document under the fresh key: the card is staging again.
    give_a_source(&mut record);

    let snapshot = record.clone();
    for input in [
        ProductInput::StageComplete {
            stamp: first,
            content: staged(STAGED_NAME, 100),
            possession: SourcePossession::Streamed,
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
            content: staged(STAGED_NAME, 100),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    assert_eq!(staged, vec![ProductEffect::RetireStaging { stamp }]);
    let launched = record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert_eq!(start_plan(&launched).resume, ResumeIntent::Fresh);
}

/// A card with no source does not RESUME — it re-picks.
///
/// Resume restarts an attempt, and advancing the generation for one would
/// discharge the acquisition key the card is waiting on, so the picker's answer
/// would arrive stale and the card would wait forever. The old code took that
/// branch and parked in `Preparing` with no effect at all, which is the same
/// hang wearing a different state.
#[test]
fn a_sourceless_card_re_picks_rather_than_resumes() {
    let mut record = staging(Direction::Send);
    let stamp = record.stamp();
    record.reduce(ProductInput::StageFailed { stamp }).unwrap();
    // The staging worker must retire before the failed card is at rest.
    record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    let before = record.clone();

    let effects = record
        .reduce(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(record, before, "resume moved a card that has no source");
    assert!(effects.is_empty());
    assert!(!record.allowed_commands().contains(&ProductCommand::Resume));

    // Re-pick is the command that does move it, and it asks again.
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::RePickSource))
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert_eq!(record.generation.get(), before.generation.get() + 1);
    assert!(
        effects.is_empty(),
        "asking a person to choose a file is an affordance, not platform work"
    );
    // And the fresh gate accepts the answer, which the old code never did: it
    // advanced the generation and left the card in `RePickRequired`, so the
    // picker opened and its offer was then refused.
    let offered = ProductInput::SourceOffered {
        offer: offer(&record, STAGED_NAME, None),
    };
    record.reduce(offered).unwrap();
    assert!(matches!(
        record.source,
        crate::SourceLifecycle::Acquiring(_)
    ));
}

#[test]
fn resume_after_completed_staging_goes_to_the_wire() {
    let mut record = staging(Direction::Send);
    let stamp = record.stamp();
    // Staging completes and its worker retires: the attempt goes to the wire.
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 100),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.state, ProductState::Connecting);
    assert!(record.source_is_ready());
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
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Allowed);
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
    assert_eq!(record.bytes_resumed, None);
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
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Allowed);
    record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    // UNSETTLED. Entering the phase used to copy this card's remembered
    // progress in as though the peer had agreed to it; only the peer can say.
    assert_eq!(record.bytes_resumed, None);
    assert_eq!(
        record.bytes,
        ByteCount::new(50),
        "the memory itself survives"
    );
}

/// The card's remembered progress is a guess in BOTH directions, and only the
/// peers can settle it.
///
/// Downward: the receiver's durable prefix failed its own digest, so nothing is
/// resumed and a card still showing 50 would be showing bytes that will be sent
/// again. Upward: the receiver checkpointed bytes whose progress event never
/// reached this card, so it resumes past what this card remembers.
///
/// This is why the attempt plan stopped carrying an offset: the executor used to
/// refuse a peer that disagreed with the guess, which made the second case a
/// protocol violation and the "I already hold this whole file" recovery
/// unreachable.
#[test]
fn a_settled_resume_corrects_the_card_in_either_direction() {
    for settled in [0, 20, 80] {
        let mut record = transfer(Direction::Send);
        record
            .reduce(event(
                &record,
                AttemptEventKind::Progress {
                    transferred: ByteCount::new(50),
                },
            ))
            .unwrap();
        assert_eq!(record.bytes, ByteCount::new(50));

        record
            .reduce(event(
                &record,
                AttemptEventKind::ResumeEstablished {
                    offset: ByteCount::new(settled),
                },
            ))
            .unwrap();
        assert_eq!(
            record.bytes_resumed,
            Some(ByteCount::new(settled)),
            "the settled offset is what was actually resumed"
        );
        assert_eq!(
            record.bytes,
            ByteCount::new(settled),
            "progress follows the settled offset, downward included"
        );
    }
}

/// Settled ONCE. Otherwise the one input allowed to move progress down could be
/// replayed, and an untrusted executor could drag the bar wherever it liked for
/// the life of the attempt — emit 80, let progress run, then emit 5, repeatedly.
///
/// The executor has a latch of its own, but a latch inside the thing being
/// distrusted is not an invariant. This is where it has to hold.
#[test]
fn a_settled_resume_cannot_be_replayed() {
    let mut record = transfer(Direction::Send);
    record
        .reduce(event(
            &record,
            AttemptEventKind::ResumeEstablished {
                offset: ByteCount::new(40),
            },
        ))
        .unwrap();
    assert_eq!(record.bytes_resumed, Some(ByteCount::new(40)));

    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(70),
            },
        ))
        .unwrap();

    record
        .reduce(event(
            &record,
            AttemptEventKind::ResumeEstablished {
                offset: ByteCount::new(5),
            },
        ))
        .unwrap();
    assert_eq!(
        record.bytes_resumed,
        Some(ByteCount::new(40)),
        "a second establishment must not rewrite the first"
    );
    assert_eq!(
        record.bytes,
        ByteCount::new(70),
        "and must not drag progress backwards"
    );
}

/// A new attempt has not settled anything yet, whatever the last one settled.
///
/// Both paths, and the PAUSE one is why. Cancel clears progress wholesale, so it
/// clears the settlement as a side effect and proves nothing about the rule. A
/// pause deliberately KEEPS its remembered bytes — that memory is the point — so
/// it is the path on which a settlement can be left standing into the next
/// generation, where the once-only guard would then reject the new attempt's
/// real answer and the card would never learn what it resumed from.
#[test]
fn a_new_generation_starts_unsettled() {
    for (intent, command) in [
        (RetirementIntent::Cancel, ProductCommand::Cancel),
        (RetirementIntent::Pause, ProductCommand::Pause),
    ] {
        let mut record = transfer(Direction::Send);
        record
            .reduce(event(
                &record,
                AttemptEventKind::ResumeEstablished {
                    offset: ByteCount::new(40),
                },
            ))
            .unwrap();
        assert_eq!(record.bytes_resumed, Some(ByteCount::new(40)));

        record.reduce(ProductInput::Command(command)).unwrap();
        quiesce(&mut record, intent);
        if intent == RetirementIntent::Cancel {
            assert_eq!(record.bytes_resumed, None);
            continue;
        }

        // The pause keeps what it remembers, and that is correct.
        assert_eq!(record.bytes, ByteCount::new(40));
        record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap();
        assert_eq!(
            record.bytes_resumed, None,
            "a new attempt has settled nothing, whatever the last one settled"
        );

        // And the new attempt's own answer is admitted rather than swallowed.
        record
            .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring)))
            .unwrap();
        record
            .reduce(event(
                &record,
                AttemptEventKind::ResumeEstablished {
                    offset: ByteCount::new(25),
                },
            ))
            .unwrap();
        assert_eq!(record.bytes_resumed, Some(ByteCount::new(25)));
        assert_eq!(record.bytes, ByteCount::new(25));
    }
}

fn declaration(record: &TransferRecord, name: &str, size: u64) -> PeerContentDeclaration {
    PeerContentDeclaration {
        transfer: record.identity.transfer,
        offered_name: OfferedName::from_untrusted(name).expect("a bounded name"),
        file_size: ByteCount::new(size),
    }
}

/// A receive learns what it is receiving, once, from the peer that knows.
///
/// Until this arrives a receiving card has no total at all, which is why every
/// bound in this reducer was inert for it.
#[test]
fn a_peer_declaration_establishes_a_receives_content() {
    let mut record = create(Direction::Receive).0;
    assert_eq!(record.known_total(), None, "nobody has said yet");

    let declared = declaration(&record, "report.pdf", 4096);
    assert_eq!(
        record.classify_peer_content(&declared),
        PeerContentDecision::Established
    );
    record
        .reduce(ProductInput::PeerContentDeclared(declared.clone()))
        .unwrap();
    assert_eq!(record.known_total(), Some(ByteCount::new(4096)));

    // The same header again — every resumed attempt re-sends one — settles
    // nothing new and must not restart anything.
    record.bytes = ByteCount::new(1000);
    assert_eq!(
        record.classify_peer_content(&declared),
        PeerContentDecision::AlreadyEstablished
    );
    record
        .reduce(ProductInput::PeerContentDeclared(declared))
        .unwrap();
    assert_eq!(
        record.bytes,
        ByteCount::new(1000),
        "an identical retry is inert"
    );
}

/// A re-picked document replaces what was announced, and takes the progress
/// measured against the old one with it.
///
/// Nobody chooses to swap a document — `RePickSource` is refused while a source
/// is ready. The only route is the person's file going away and the app asking
/// them to choose again, which does not constrain what they choose.
#[test]
fn a_different_declaration_replaces_an_unsealed_partial() {
    let mut record = create(Direction::Receive).0;
    record
        .reduce(ProductInput::PeerContentDeclared(declaration(
            &record, "a.pdf", 1000,
        )))
        .unwrap();
    record.state = ProductState::Transferring;
    record
        .reduce(event(
            &record,
            AttemptEventKind::ResumeEstablished {
                offset: ByteCount::new(0),
            },
        ))
        .unwrap();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(400),
            },
        ))
        .unwrap();
    assert_eq!(record.bytes, ByteCount::new(400));

    let replacement = declaration(&record, "b.pdf", 2000);
    assert_eq!(
        record.classify_peer_content(&replacement),
        PeerContentDecision::Replaced
    );
    record
        .reduce(ProductInput::PeerContentDeclared(replacement))
        .unwrap();
    assert_eq!(record.known_total(), Some(ByteCount::new(2000)));
    assert_eq!(
        record.bytes,
        ByteCount::new(0),
        "400 bytes of the OLD document is not progress towards the new one"
    );
    assert_eq!(record.bytes_resumed, None, "nor is anything resumed");
}

/// Delivered content is frozen, and not as a matter of taste.
///
/// The mailbox receipt seals under a transfer-derived key with a fixed zero
/// nonce, which is safe only because one transfer yields one canonical receipt
/// (`envoix-protocol/src/mailbox/receipt.rs`). A second content under the same
/// transfer would be nonce reuse.
#[test]
fn a_delivered_transfer_refuses_a_different_declaration() {
    for state in [
        ProductState::Completed,
        ProductState::Unconfirmed,
        ProductState::Verifying,
    ] {
        let mut record = create(Direction::Receive).0;
        record
            .reduce(ProductInput::PeerContentDeclared(declaration(
                &record, "a.pdf", 1000,
            )))
            .unwrap();
        record.state = state;

        let replacement = declaration(&record, "b.pdf", 2000);
        assert_eq!(
            record.classify_peer_content(&replacement),
            PeerContentDecision::FinalContentConflict,
            "{state:?} must not accept a different document"
        );
        record
            .reduce(ProductInput::PeerContentDeclared(replacement))
            .unwrap();
        assert_eq!(
            record.known_total(),
            Some(ByteCount::new(1000)),
            "and must not have changed anything by refusing"
        );
    }
}

/// A declaration for another transfer, or to a card that sends, changes nothing.
#[test]
fn a_declaration_that_is_not_this_cards_is_refused() {
    let mut record = create(Direction::Receive).0;
    let mut foreign = declaration(&record, "a.pdf", 1000);
    foreign.transfer = TransferId::from_bytes([0x9f; 16]);
    assert_eq!(
        record.classify_peer_content(&foreign),
        PeerContentDecision::NotThisTransfer
    );
    record
        .reduce(ProductInput::PeerContentDeclared(foreign))
        .unwrap();
    assert_eq!(record.known_total(), None);

    let sender = staging(Direction::Send);
    assert_eq!(
        sender.classify_peer_content(&declaration(&sender, "a.pdf", 1000)),
        PeerContentDecision::NotThisTransfer,
        "a card that sends is not told what it is receiving"
    );
}

/// An empty file is a real file, and its bounds are real bounds.
///
/// `total()` cannot tell "nobody has said" from "known to be nothing", because
/// both are zero — so every bound written against it switched OFF for an empty
/// transfer. That is reachable today for a staged zero-byte send, and it would
/// have applied to every receive the moment one admits its peer's declared
/// content.
#[test]
fn a_known_empty_transfer_still_bounds_everything() {
    let mut record = staging(Direction::Send);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 0),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    record
        .reduce(ProductInput::StagingRetired { stamp })
        .unwrap();
    assert_eq!(record.total(), ByteCount::new(0));
    assert_eq!(
        record.known_total(),
        Some(ByteCount::new(0)),
        "an established empty total is KNOWN, not absent"
    );

    record
        .reduce(event(&record, AttemptEventKind::Phase(Phase::Transferring)))
        .unwrap();
    record
        .reduce(event(
            &record,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(1),
            },
        ))
        .unwrap();
    assert_eq!(
        record.bytes,
        ByteCount::new(0),
        "no transfer of an empty file moves a byte"
    );
    record
        .reduce(event(
            &record,
            AttemptEventKind::ResumeEstablished {
                offset: ByteCount::new(1),
            },
        ))
        .unwrap();
    assert_eq!(record.bytes_resumed, None, "and none can be resumed");
}

/// More trusted than the other executor events, not trusted absolutely. An
/// offset past the total would let an untrusted attempt declare a card finished.
#[test]
fn a_settled_resume_past_the_total_is_ignored() {
    let mut record = transfer(Direction::Send);
    let before = record.bytes;
    record
        .reduce(event(
            &record,
            AttemptEventKind::ResumeEstablished {
                offset: ByteCount::new(record.total().get() + 1),
            },
        ))
        .unwrap();
    assert_eq!(record.bytes, before);
    assert_eq!(record.bytes_resumed, None);
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
    assert_eq!(record.bytes, record.total());
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
        let effects = record
            .reduce(ProductInput::Command(ProductCommand::Resume))
            .unwrap();
        assert_eq!(start_plan(&effects).resume, ResumeIntent::Allowed);
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
                    resume: ResumeIntent::Allowed,
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
    assert_eq!(record.bytes, record.total());
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

    let mut preparing = needs_repick();
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
    let mut record = staging(Direction::Send);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 90),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    assert_eq!(record.state, ProductState::Preparing);
    assert!(record.source_is_ready());
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

/// Progress above the final total is CONTRADICTORY, not something to clamp.
///
/// The old reducer silently clamped it into `Ready`, which authored a record
/// whose own history could not explain it: staging had reported more durable
/// bytes than the file it says it finished reading. Both observations came from
/// the same worker and they cannot both be true, so the honest answer is that
/// staging failed — and the card asks for a document again rather than sending
/// one whose length nobody can account for.
#[test]
fn stage_complete_refuses_a_total_below_the_progress_it_already_reported() {
    let mut record = staging(Direction::Send);
    let stamp = record.stamp();
    record
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(80),
        })
        .unwrap();
    let effects = record
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 50),
            possession: SourcePossession::Streamed,
        })
        .unwrap();

    assert_eq!(record.state, ProductState::Failed);
    assert!(!record.source_is_ready(), "a contradiction cannot be Ready");
    assert_eq!(effects, vec![ProductEffect::RetireStaging { stamp }]);
    assert_eq!(
        record.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );
    assert!(
        encode_record(&record).is_ok(),
        "the reduced staging record must be encodable by its own codec"
    );

    // An equal total is not contradictory: staging read exactly what it
    // reported, which is the ordinary end of a stream it had already counted.
    let mut exact = staging(Direction::Send);
    let stamp = exact.stamp();
    exact
        .reduce(ProductInput::StageProgress {
            stamp,
            transferred: ByteCount::new(80),
        })
        .unwrap();
    exact
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, 80),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    assert!(exact.source_is_ready());
    assert_eq!(exact.total(), ByteCount::new(80));
    assert_eq!(exact.bytes, ByteCount::new(80));
}

#[test]
fn admitted_progress_is_monotone_within_a_generation() {
    // An untrusted executor event must not move what the person is shown
    // backward. This no longer guards a resume decision — the plan carries no
    // offset — but a progress bar that jumps back on a stale event is its own
    // defect, and `ResumeEstablished` is the one event allowed to correct it.
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
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Allowed);
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

    let mut needs_repick = needs_repick();
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
    let mut needs_repick = needs_repick();
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
            staging(Direction::Send),
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
    let held = offer(&base, STAGED_NAME, Some(STAGED_TOTAL));
    let lifecycles = [
        crate::SourceLifecycle::initial(Direction::Receive),
        crate::SourceLifecycle::initial(Direction::Send),
        crate::SourceLifecycle::lost(held.clone(), SourceAcquisitionFailure::Unreadable),
        crate::SourceLifecycle::staging_failed(held.clone()),
        crate::SourceLifecycle::Acquiring(held.clone()),
        crate::SourceLifecycle::staging(
            held.clone(),
            AcquiredSelection::of_one(SourceRetention::Persisted, SourceSeekability::Seekable),
            crate::StagingPlan::ProviderStream {
                item: SourceItemId::new(0),
            },
        ),
        base.source.clone(),
    ];
    let mut swept = 0usize;
    let mut offered = [0usize; 5];
    let mut withheld = [0usize; 5];
    for state in states {
        for quiescence in quiescences {
            // Every fact, so one that starts gating a command tomorrow is
            // already inside the sweep — and every source lifecycle, which is
            // where the two booleans this replaced used to live.
            for bits in 0..16u8 {
                for source in &lifecycles {
                    for outcome in &outcomes {
                        let mut record = base.clone();
                        record.state = state;
                        record.quiescence = quiescence;
                        record.facts = Facts {
                            complete_sent: bits & 1 != 0,
                            proof_delivered: bits & 2 != 0,
                            receipt_mismatch: bits & 4 != 0,
                            remove_requested: bits & 8 != 0,
                        };
                        record.source = source.clone();
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
    // 11 states x 6 quiescences x 16 fact combinations x 7 lifecycles x 23
    // outcomes. It grew when the two source booleans became the lifecycle:
    // seven real source states replaced four boolean combinations, three of
    // which could never happen.
    assert_eq!(swept, 170_352, "the constructible space changed shape");
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
                record.total().get() == 0 || record.bytes.get() <= record.total().get(),
                "bytes exceed total: {record:?}"
            );
        }
    }
}

#[test]
fn product_model_scenario_trace() {
    // The trace starts where a sender actually reaches the wire: after a
    // document has been chosen, acquired and staged.
    let (mut send, launch_effects) = launched(Direction::Send);
    assert_eq!(start_plan(&launch_effects).resume, ResumeIntent::Fresh);
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
    assert_eq!(start_plan(&effects).resume, ResumeIntent::Allowed);
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

/// Creating a card that needs a document commissions NO platform work.
///
/// Asking a person to choose a file is an affordance the card publishes, not a
/// duty the authority hands the platform. It used to be one, issued the moment a
/// sender existed — so the adapter was told to bind a document nobody had picked
/// yet, claimed from an empty registry, and answered `source_unreadable` every
/// time. The handle duty belongs where there is something to bind: after an
/// offer is accepted.
#[test]
fn creating_a_card_that_needs_a_document_commissions_no_platform_work() {
    let (record, effects) = create(Direction::Send);
    assert_eq!(record.state, ProductState::Preparing);
    assert!(
        effects.is_empty(),
        "a card with no document asked the platform for work, got {effects:?}"
    );
    // What it publishes instead is the acquisition an offer must name, and the
    // authority will check an offer against the SAME derived value.
    assert_eq!(record.current_acquisition().card(), record.identity.card);
    assert_eq!(record.current_acquisition().generation(), record.generation);

    // Accepting an offer is what commissions the handle duty, and it is
    // post-commit: the acquisition is durable before the platform is asked to
    // hold what it names.
    let (mut session, outcome) = crate::CommittedSession::create_without_store(
        NewTransfer {
            direction: Direction::Send,
            participation: crate::RoomParticipation::Minted,
            pairing: None,
        },
        &mut DeterministicEntropy::default(),
    )
    .expect("deterministic identity source");
    assert_eq!(session.record().state, ProductState::Preparing);
    assert!(outcome.released_immediately.is_empty());
    assert!(outcome.released_after_commit.is_empty());

    let offered = ProductInput::SourceOffered {
        offer: offer(session.record(), STAGED_NAME, None),
    };
    let bound = session.apply(offered).expect("the offer is accepted");
    assert!(bound.released_immediately.is_empty());
    let [ProductEffect::CapabilityDuty { duty, action }] = bound.released_after_commit.as_slice()
    else {
        panic!(
            "an accepted offer commissions the handle duty, got {:?}",
            bound.released_after_commit
        );
    };
    assert_eq!(*action, CapabilityAction::AcquireSource);
    assert_eq!(duty.kind, DutyKind::SourceHandle);
    assert_eq!(duty.provenance.card, session.record().identity.card);
    assert_eq!(duty.provenance.generation, session.record().generation);

    // A receiver can never take a document, so nothing about one is ever
    // commissioned for it. That is a property of its DIRECTION now, not of a
    // source decision a caller supplied.
    let (_, effects) = create(Direction::Receive);
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            ProductEffect::CapabilityDuty {
                action: CapabilityAction::AcquireSource,
                ..
            }
        )),
        "a receiver was commissioned work for a document it can never have"
    );
}

/// The re-pick command is the recovery `RS04` says the old app stranded users
/// without. It asks again under a FRESH generation — which mints a fresh
/// acquisition key, so a late answer under the discharged one cannot bind.
#[test]
fn re_picking_a_source_asks_again_under_a_new_generation() {
    let mut record = needs_repick();
    let stamp = record.stamp();
    let _ = record.reduce(ProductInput::StageFailed { stamp });
    let _ = record.reduce(ProductInput::StagingRetired { stamp });
    assert_eq!(record.state, ProductState::Failed);
    assert_eq!(
        record.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );

    let before = record.current_acquisition();
    let effects = record
        .reduce(ProductInput::Command(ProductCommand::RePickSource))
        .expect("the re-pick reduces");
    assert!(effects.is_empty(), "the ask is published, not commissioned");
    let after = record.current_acquisition();
    assert_eq!(after.generation(), record.generation);
    assert_ne!(after.generation(), stamp.generation);
    assert!(!before.is(&after), "the re-pick reused the discharged key");
    assert_eq!(record.state, ProductState::Preparing);

    // And the discharged key is refused where the fresh one is accepted, which
    // is what makes a late answer to the old ask inert rather than binding.
    assert_eq!(
        record.answer_source_offer(&AcceptedSourceOffer::of_one_document(
            before,
            OfferedName::from_untrusted(STAGED_NAME).unwrap(),
            None,
        )),
        crate::SourceOfferAnswer::Stale
    );
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

/// The acquisition the fixture record's own identity mints, taken from the
/// record rather than restated.
fn fixture_acquisition() -> envoix_capabilities::SourceAcquisitionKey {
    fixture_skeleton().current_acquisition()
}

fn fixture_record() -> TransferRecord {
    let mut record = fixture_skeleton();
    record.source = crate::SourceLifecycle::Ready {
        offer: crate::AcceptedSourceOffer::of_one_document(
            fixture_acquisition(),
            OfferedName::from_untrusted("a.txt").unwrap(),
            Some(ByteCount::new(10)),
        ),
        acquired: AcquiredSelection::of_one(
            crate::SourceRetention::Persisted,
            SourceSeekability::Seekable,
        ),
        backing: crate::SourceBacking::PersistedProvider,
        content: crate::StagedContent::new(
            crate::TransferContent::new(
                OfferedName::from_untrusted("a.txt").unwrap(),
                ByteCount::new(10),
            ),
            ContentHash::from_bytes([5; 32]),
        ),
    };
    record
}

fn fixture_skeleton() -> TransferRecord {
    TransferRecord {
        identity: ProductIdentity {
            card: RecordId::new(1),
            transfer: TransferId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            artifact: ArtifactId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
        },
        direction: Direction::Send,
        // Replaced by `fixture_record`. The skeleton exists only so the
        // acquisition can be taken FROM the record it belongs to.
        source: crate::SourceLifecycle::initial(Direction::Send),
        participation: crate::RoomParticipation::Minted,
        state: ProductState::Paused(PauseOrigin::Local),
        quiescence: crate::Quiescence::Quiescent,
        generation: AttemptGen::new(7),
        phase: Phase::Transferring,
        bytes: ByteCount::new(4),
        bytes_resumed: Some(ByteCount::new(2)),
        outcome: None,
        facts: crate::Facts {
            complete_sent: false,
            proof_delivered: false,
            receipt_mismatch: false,
            remove_requested: false,
        },
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

/// A staging plan must read a document the selection actually holds.
///
/// The plan names WHICH item it streams or copies, because a lone document is
/// the only thing that can be streamed and a verbatim copy has exactly one
/// input. A record whose plan names an ordinal that is not in its own selection
/// would send a document the card never accepted — or nothing at all — so it is
/// refused where it is decoded rather than puzzled over when it is read.
#[test]
fn a_plan_naming_an_item_outside_the_selection_is_refused() {
    /// The envelope this build writes: schema length, schema, version, body
    /// length. The body is everything after it.
    const HEADER_BYTES: usize = 2 + 23 + 4 + 4;

    let (mut card, _) = create(Direction::Send);
    give_a_source(&mut card);
    let encoded = encode_record(&card).expect("a staging record encodes");
    let body = std::str::from_utf8(&encoded[HEADER_BYTES..]).expect("the body is text");
    assert!(
        body.contains(r#""provider_stream":{"item":0}"#),
        "the fixture is not a streaming staging record: {body}"
    );

    let reframed = |body: &str| {
        let body = body.as_bytes();
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&23_u16.to_be_bytes());
        encoded.extend_from_slice(b"envoix/product-record/1");
        encoded.extend_from_slice(&crate::PRODUCT_RECORD_VERSION.to_be_bytes());
        encoded.extend_from_slice(&(body.len() as u32).to_be_bytes());
        encoded.extend_from_slice(body);
        encoded
    };

    // The same record with the plan pointed at an item the selection has not
    // got. Nothing else about it changes.
    assert_eq!(
        decode_record(&reframed(&body.replace(
            r#""provider_stream":{"item":0}"#,
            r#""provider_stream":{"item":7}"#,
        ))),
        Err(RecordCodecError::MalformedBody),
        "a plan reading a document the card never accepted was made live"
    );

    // And the untampered bytes DO decode, so the refusal is about the ordinal
    // rather than about the reframing.
    assert!(
        matches!(decode_record(&reframed(body)), Ok(RecordDecode::Loaded(_))),
        "reframing broke the untampered record"
    );
}

#[test]
fn product_record_v11_has_a_byte_exact_fixture() {
    let body = br#"{"identity":{"card":1,"transfer":"00000000000000000000000000000002","artifact":"00000000000000000000000000000003"},"direction":"send","state":{"state":"paused","origin":"local"},"quiescence":{"status":"quiescent"},"generation":7,"phase":"transferring","bytes":4,"bytes_resumed":2,"outcome":null,"facts":{"complete_sent":false,"proof_delivered":false,"receipt_mismatch":false,"remove_requested":false},"source":{"ready":{"offer":{"key":{"card":1,"generation":7,"request":"656e766f69782f736f757263652f7635"},"selection":[{"id":0,"path":["a.txt"],"reported_size":10}],"output_name":"a.txt"},"acquired":[{"item":0,"retention":"persisted","seekability":"seekable"}],"backing":"persisted_provider","content":{"content":{"name":"a.txt","total":10},"content_hash":[5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5]}}},"participation":"minted","pairing":null,"create_request_id":null,"receipt_request":"00000000000000000000000000000004","command_ledger":[]}"#;
    let mut expected = Vec::new();
    expected.extend_from_slice(&23_u16.to_be_bytes());
    expected.extend_from_slice(b"envoix/product-record/1");
    expected.extend_from_slice(&11_u32.to_be_bytes());
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

/// An admitted source answer that names a DIFFERENT acquisition is inert.
///
/// The ledger proves a result was admitted once; it does not prove it belongs to
/// this card's current ask. Without the exact-key guard the reducer would act on
/// a neighbour's answer — and every other test in this file builds the answer
/// from the same record it offered, so none of them can catch its removal.
#[test]
fn a_source_answer_for_another_acquisition_moves_nothing() {
    let mut card = create(Direction::Send).0;
    let offered = ProductInput::SourceOffered {
        offer: offer(&card, STAGED_NAME, None),
    };
    card.reduce(offered).unwrap();
    let acquiring = card.clone();

    // Another card entirely. Built by moving THIS card's identity rather than
    // creating a second one, because the deterministic entropy would mint the
    // same identity twice and the case would pass for the wrong reason.
    let mut other = card.clone();
    other.identity.card = RecordId::new(card.identity.card.get() ^ 0x5a5a);
    let foreign = settled(&other, acquired());
    assert!(card.reduce(foreign).unwrap().is_empty());
    assert_eq!(card, acquiring, "another card's answer moved this one");

    // The same card under a superseded generation.
    let mut stale = card.clone();
    stale.generation = AttemptGen::new(card.generation.get() + 1);
    let superseded = settled(&stale, acquired());
    assert!(card.reduce(superseded).unwrap().is_empty());
    assert_eq!(card, acquiring, "a superseded generation's answer bound");

    // The same card and generation, a different request.
    let mut rerequested = card.clone();
    rerequested.receipt_request = RequestId::from_bytes([0x5a; 16]);
    let wrong_request = settled(&rerequested, acquired());
    assert!(card.reduce(wrong_request).unwrap().is_empty());
    assert_eq!(card, acquiring, "an answer to another request bound");

    // And the card's OWN answer does move it, so the three inertness cases
    // above are not passing because nothing works.
    let mine = settled(&card, acquired());
    card.reduce(mine).unwrap();
    assert!(matches!(
        card.source,
        crate::SourceLifecycle::Staging { .. }
    ));
}

/// A send that failed on its source can actually re-pick.
///
/// `source_failure` offers `Recovery::RePickSource`, and `RePickSource` is
/// refused while the lifecycle is still `Ready`. So a terminal that failed the
/// card without invalidating the source advertised a recovery the command guard
/// then denied — the user is told to choose again by the only affordance that
/// will not let them.
#[test]
fn a_source_failure_mid_send_leaves_the_card_able_to_pick_again() {
    let (mut card, _) = create(Direction::Send);
    give_a_source(&mut card);
    let stamp = card.stamp();
    card.reduce(ProductInput::StageComplete {
        stamp,
        content: staged(STAGED_NAME, STAGED_TOTAL),
        possession: SourcePossession::Streamed,
    })
    .unwrap();
    card.reduce(ProductInput::StagingRetired { stamp }).unwrap();
    assert!(card.source.is_ready());

    // Retiring the stager starts the attempt, which then cannot send from that
    // source — it changed under the sender, or stopped being readable.
    assert_ne!(
        card.state,
        ProductState::Preparing,
        "a ready send never left preparing"
    );
    card.reduce(event(
        &card.clone(),
        AttemptEventKind::Terminal(OutcomeCode::SourceUnreadable),
    ))
    .unwrap();

    assert_eq!(card.state, ProductState::Failed);
    assert!(
        !card.source.is_ready(),
        "the source the attempt could not send from is still ready: {:?}",
        card.source
    );
    assert_eq!(
        card.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );

    // Commands are offered only once the retirement the terminal requested has
    // been acknowledged, so the affordance is checked at rest.
    card.quiescence = Quiescence::Quiescent;
    assert!(
        card.allowed_commands()
            .contains(&ProductCommand::RePickSource),
        "the offered recovery is not an allowed command: {:?}",
        card.allowed_commands()
    );
}

/// The SAME admitted answer, delivered twice, moves the card once.
///
/// This is what makes delivery from the ledger to the card safe to repeat, and
/// therefore what lets a delivery round that was not acknowledged simply run
/// again. Without it the runtime had to hand out a move-only token and treat its
/// disappearance as proof of success — which an actor that died mid-apply also
/// produced.
#[test]
fn the_same_source_answer_delivered_twice_moves_the_card_once() {
    let (mut card, _) = create(Direction::Send);
    card.reduce(ProductInput::SourceOffered {
        offer: offer(&card, STAGED_NAME, None),
    })
    .unwrap();
    let acquiring = card.clone();

    // Built once and delivered twice, exactly as a repeated delivery round
    // carries the same admitted answer.
    let ProductInput::SourceSettled(admitted) = settled(&card.clone(), acquired()) else {
        panic!("the helper builds a settled source answer");
    };
    card.reduce(ProductInput::SourceSettled(admitted.clone()))
        .unwrap();
    assert_ne!(card, acquiring, "the first delivery did nothing");
    let staged_once = card.clone();

    assert!(
        card.reduce(ProductInput::SourceSettled(admitted))
            .unwrap()
            .is_empty(),
        "a repeated delivery produced effects"
    );
    assert_eq!(
        card, staged_once,
        "a repeated delivery moved the card again"
    );
}

/// A copy plan cannot be established by a worker that only read the source
/// through.
///
/// The backing used to be derived from the PLAN, so a worker with no copy sink
/// answered the same completion for both — and the card rested at `Ready` over
/// an `OwnedArtifact` that had never been written. A restart would then reopen
/// bytes nobody had. The completion now names the possession it achieved and
/// the two must agree.
#[test]
fn a_copy_plan_is_not_established_by_a_stream() {
    let (mut card, _) = create(Direction::Send);
    card.reduce(ProductInput::SourceOffered {
        offer: offer(&card, STAGED_NAME, None),
    })
    .unwrap();
    // A grant that survives, over a source that cannot seek: resume needs both,
    // so the authority commissions a copy.
    card.reduce(settled(
        &card.clone(),
        SourceReport::Acquired(AcquiredSelection::of_one(
            SourceRetention::Persisted,
            SourceSeekability::SequentialOnly,
        )),
    ))
    .unwrap();
    assert!(matches!(
        card.source,
        crate::SourceLifecycle::Staging {
            plan: StagingPlan::ProduceOwnedArtifact {
                derivation: DerivationSpec::VerbatimV1 { .. },
            },
            ..
        }
    ));

    let stamp = card.stamp();
    card.reduce(ProductInput::StageComplete {
        stamp,
        content: staged(STAGED_NAME, STAGED_TOTAL),
        possession: SourcePossession::Streamed,
    })
    .unwrap();

    assert!(
        !card.source.is_ready(),
        "a streamed completion established a copy plan: {:?}",
        card.source
    );
    assert_eq!(card.state, ProductState::Failed);

    // And the possession the plan DID commission establishes it, so the refusal
    // above is not passing because nothing works.
    let mut copied = create(Direction::Send).0;
    copied
        .reduce(ProductInput::SourceOffered {
            offer: offer(&copied, STAGED_NAME, None),
        })
        .unwrap();
    copied
        .reduce(settled(
            &copied.clone(),
            SourceReport::Acquired(AcquiredSelection::of_one(
                SourceRetention::Persisted,
                SourceSeekability::SequentialOnly,
            )),
        ))
        .unwrap();
    // The witness is EARNED: a real store seals real bytes, because
    // `SealedArtifact` has no other way in. Its facts have to describe this
    // card's commissioned work, so they are read off the card rather than
    // invented.
    let crate::SourceLifecycle::Staging {
        offer,
        plan: StagingPlan::ProduceOwnedArtifact { derivation },
        ..
    } = copied.source.clone()
    else {
        panic!("a sequential source did not commission a derivation");
    };
    let bytes = vec![0xab_u8; 32];
    let (_blobs, sealed) = sealed_artifact(
        copied.identity.card,
        copied.generation,
        copied.identity.artifact,
        &bytes,
        derivation.fingerprint(&offer),
    );
    let stamp = copied.stamp();
    copied
        .reduce(ProductInput::StageComplete {
            stamp,
            // From the SEAL, not a second account of the same bytes: one value,
            // so there is nothing for the two to disagree about.
            content: crate::StagedContent::new(
                crate::TransferContent::new(
                    OfferedName::from_untrusted(STAGED_NAME).expect("a bounded name"),
                    sealed.length(),
                ),
                sealed.digest(),
            ),
            possession: SourcePossession::Derived(sealed),
        })
        .unwrap();
    assert!(matches!(
        copied.source,
        crate::SourceLifecycle::Ready {
            backing: crate::SourceBacking::OwnedArtifact { .. },
            ..
        }
    ));
}

/// A GENUINE seal for the wrong work does not establish this card's source.
///
/// The witness cannot be forged — only a blob store mints one — but that only
/// says the bytes were sealed somewhere, not that they are the bytes this card
/// commissioned. Four facts have to agree, and each is a different way of
/// sending the wrong file:
///
/// - a different ARTIFACT is the sharp one. Staging vouches for what the attempt
///   will open, and the attempt opens the card's own minted artifact — so a seal
///   naming another would have staging vouch for X while the attempt reads Y,
///   and both are real artifacts, so nothing downstream could tell.
/// - a different CARD is another card's document.
/// - a different GENERATION is a superseded run's, from before a re-pick.
/// - a different FINGERPRINT is the same bytes produced under a different
///   commissioning — another selection, or another version of the derivation.
#[test]
fn a_seal_for_other_work_does_not_establish_this_card() {
    let bytes = vec![0xcd_u8; 16];
    let commission = |card: &TransferRecord| {
        let crate::SourceLifecycle::Staging {
            offer,
            plan: StagingPlan::ProduceOwnedArtifact { derivation },
            ..
        } = card.source.clone()
        else {
            panic!("a sequential source did not commission a derivation");
        };
        derivation.fingerprint(&offer)
    };

    for wrong in ["artifact", "card", "generation", "fingerprint"] {
        let mut card = create(Direction::Send).0;
        card.reduce(ProductInput::SourceOffered {
            offer: offer(&card, STAGED_NAME, None),
        })
        .unwrap();
        card.reduce(settled(
            &card.clone(),
            SourceReport::Acquired(AcquiredSelection::of_one(
                SourceRetention::Persisted,
                SourceSeekability::SequentialOnly,
            )),
        ))
        .unwrap();
        let honest = commission(&card);
        let (_blobs, sealed) = sealed_artifact(
            if wrong == "card" {
                RecordId::new(card.identity.card.get() ^ 0x5a5a)
            } else {
                card.identity.card
            },
            if wrong == "generation" {
                AttemptGen::new(card.generation.get() + 1)
            } else {
                card.generation
            },
            if wrong == "artifact" {
                ArtifactId::from_bytes([0x77; 16])
            } else {
                card.identity.artifact
            },
            &bytes,
            if wrong == "fingerprint" {
                ContentHash::from_bytes([0x11; 32])
            } else {
                honest
            },
        );
        let stamp = card.stamp();
        card.reduce(ProductInput::StageComplete {
            stamp,
            content: crate::StagedContent::new(
                crate::TransferContent::new(
                    OfferedName::from_untrusted(STAGED_NAME).expect("a bounded name"),
                    sealed.length(),
                ),
                sealed.digest(),
            ),
            possession: SourcePossession::Derived(sealed),
        })
        .unwrap();

        assert!(
            !card.source.is_ready(),
            "a seal with the wrong {wrong} established this card's source"
        );
        assert_eq!(card.state, ProductState::Failed);
    }
}

/// A persisted source that was mid-staging when the process died is REACQUIRED,
/// not failed.
///
/// The staging worker and its handle die with the process; the platform's
/// `Persisted` grant does not. Failing the card here would send the user back to
/// the picker to choose the file Android is still holding on their behalf — and
/// the record already says the grant survives, which is the whole reason
/// the platform's per-item answers are frozen onto it.
///
/// The re-issued duty must name the SAME acquisition, because the platform
/// resolves its ownership journal by that key: a fresh one would find nothing
/// and the recovery would fail for a reason it invented itself.
#[test]
fn a_persisted_staging_reacquires_across_a_restart() {
    let (mut card, _) = create(Direction::Send);
    give_a_source(&mut card);
    let acquisition = SourceAcquisitionKey::of(DutyProvenance {
        card: card.identity.card,
        generation: card.generation,
        request: card.source_request(),
    });
    assert!(matches!(
        card.source,
        crate::SourceLifecycle::Staging { ref acquired, .. }
            if acquired.retention() == SourceRetention::Persisted
    ));
    // Bytes were counted before the crash, and none of them were established.
    card.reduce(ProductInput::StageProgress {
        stamp: card.stamp(),
        transferred: ByteCount::new(4096),
    })
    .unwrap();
    assert_eq!(card.bytes.get(), 4096);

    let effects = card.reduce(ProductInput::Restore).unwrap();

    assert_eq!(card.state, ProductState::Preparing);
    let crate::SourceLifecycle::Acquiring(offer) = &card.source else {
        panic!(
            "a persisted staging did not go back to acquiring: {:?}",
            card.source
        );
    };
    assert!(
        offer.key().is(&acquisition),
        "the restart asked for a different acquisition than the platform holds"
    );
    assert_eq!(
        card.bytes.get(),
        0,
        "a read that has not restarted still showed progress"
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            ProductEffect::CapabilityDuty {
                duty,
                action: CapabilityAction::AcquireSource,
            } if SourceAcquisitionKey::of(duty.provenance).is(&acquisition)
        )),
        "the acquire duty was not re-issued: {effects:?}"
    );

    // And the re-issued duty can actually be ANSWERED. Emitting it is not the
    // recovery — a duty whose answer the reducer then refuses would leave the
    // card acquiring forever, which is what going back to `Acquiring` rather
    // than inventing a second accepted input is for.
    card.reduce(settled(&card.clone(), acquired())).unwrap();
    assert!(
        matches!(
            card.source,
            crate::SourceLifecycle::Staging {
                ref acquired,
                plan: StagingPlan::ProviderStream { .. },
                ..
            } if acquired.retention() == SourceRetention::Persisted
        ),
        "the platform's answer did not restart staging: {:?}",
        card.source
    );
}

/// An owned artifact that cannot be opened is a STORAGE failure, not a reason to
/// choose a different file.
///
/// The premise that every source failure ends in re-pick was true while every
/// source was somebody else's file. It stops being true the moment the bytes are
/// ours: asking the person to choose the same documents again does not repair a
/// disk fault, and it throws away an artifact that may still be there next time.
///
/// So the two codes do opposite things to the lifecycle. `SourceUnreadable`
/// invalidates `Ready` — the document is gone, choose again. `StorageFault`
/// leaves it exactly where it was, because the artifact is still what this card
/// is sending and `RetryLater` is the recovery that fits.
#[test]
fn an_owned_artifact_that_cannot_be_opened_is_retryable_not_re_pickable() {
    let ready_owned = || {
        let mut card = create(Direction::Send).0;
        card.reduce(ProductInput::SourceOffered {
            offer: offer(&card, STAGED_NAME, None),
        })
        .unwrap();
        card.reduce(settled(
            &card.clone(),
            SourceReport::Acquired(AcquiredSelection::of_one(
                SourceRetention::Persisted,
                SourceSeekability::SequentialOnly,
            )),
        ))
        .unwrap();
        let crate::SourceLifecycle::Staging {
            offer: commissioned,
            plan: StagingPlan::ProduceOwnedArtifact { derivation },
            ..
        } = card.source.clone()
        else {
            panic!("a sequential source did not commission a production");
        };
        let (blobs, sealed) = sealed_artifact(
            card.identity.card,
            card.generation,
            card.identity.artifact,
            &[3_u8; 16],
            derivation.fingerprint(&commissioned),
        );
        let stamp = card.stamp();
        card.reduce(ProductInput::StageComplete {
            stamp,
            content: crate::StagedContent::new(
                crate::TransferContent::new(
                    OfferedName::from_untrusted(STAGED_NAME).expect("a bounded name"),
                    sealed.length(),
                ),
                sealed.digest(),
            ),
            possession: SourcePossession::Derived(sealed),
        })
        .unwrap();
        card.reduce(ProductInput::StagingRetired { stamp }).unwrap();
        (blobs, card)
    };

    // Storage failed: the artifact is still this card's source, and the person
    // is offered a retry rather than the picker.
    let (_blobs, mut card) = ready_owned();
    card.reduce(event(
        &card.clone(),
        AttemptEventKind::Terminal(OutcomeCode::StorageFault),
    ))
    .unwrap();
    assert!(
        card.source.is_ready(),
        "a disk fault threw away an artifact this card owns: {:?}",
        card.source
    );
    card.quiescence = Quiescence::Quiescent;
    assert_eq!(
        card.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RetryLater)
    );
    assert!(
        card.allowed_commands().contains(&ProductCommand::Resume),
        "a retryable storage failure offered no retry: {:?}",
        card.allowed_commands()
    );

    // And the provider code still does the opposite on the very same card, so
    // the difference is the CODE rather than the backing happening to be safe.
    let (_blobs, mut card) = ready_owned();
    card.reduce(event(
        &card.clone(),
        AttemptEventKind::Terminal(OutcomeCode::SourceUnreadable),
    ))
    .unwrap();
    assert!(!card.source.is_ready());
    assert_eq!(
        card.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(Recovery::RePickSource)
    );
}

/// A ready source splits on WHO holds the bytes, and the two answers are
/// opposite for the same reason.
///
/// A PROVIDER source is somebody else's file reached through a descriptor that
/// died with the process, and it may have changed while we were gone — neither
/// knowable from the record. So the card re-acquires and re-reads, which is the
/// owner's ruling and costs a full pass per process death.
///
/// An OWNED artifact is ours: produced, sealed, immutable. Re-deriving it
/// because a process died would re-zip gigabytes to learn what the seal already
/// says, so restore does nothing at all and the attempt opens it — the store
/// refuses anything unsealed, and the send hashes what it transmits either way.
#[test]
fn restore_reacquires_a_provider_source_and_leaves_an_owned_one_alone() {
    // Provider: back to acquiring, under the same key the platform journals.
    let (mut provider, _) = create(Direction::Send);
    give_a_source(&mut provider);
    let stamp = provider.stamp();
    provider
        .reduce(ProductInput::StageComplete {
            stamp,
            content: staged(STAGED_NAME, STAGED_TOTAL),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    // Restored while still `Preparing`: the window between staging finishing
    // and the attempt starting. A card that had already STARTED an attempt
    // restores as `Paused(Lost)` and reaches its source through `Resume`, which
    // is a separate path with the transfer's own resume offset to preserve.
    let expected = *provider
        .source
        .key()
        .expect("a ready sender names its acquisition");
    assert!(matches!(
        provider.source,
        crate::SourceLifecycle::Ready {
            backing: crate::SourceBacking::PersistedProvider,
            ..
        }
    ));

    let effects = provider.reduce(ProductInput::Restore).unwrap();
    let crate::SourceLifecycle::Acquiring(reacquired) = &provider.source else {
        panic!(
            "a ready provider source was not reacquired: {:?}",
            provider.source
        );
    };
    assert!(
        reacquired.key().is(&expected),
        "the restart asked for a different acquisition"
    );
    assert!(effects.iter().any(|effect| matches!(
        effect,
        ProductEffect::CapabilityDuty {
            action: CapabilityAction::AcquireSource,
            ..
        }
    )));

    // Owned: untouched, and the attempt starts. Nothing asks the platform,
    // because the platform does not hold these bytes.
    let mut owned = create(Direction::Send).0;
    owned
        .reduce(ProductInput::SourceOffered {
            offer: offer(&owned, STAGED_NAME, None),
        })
        .unwrap();
    owned
        .reduce(settled(
            &owned.clone(),
            SourceReport::Acquired(AcquiredSelection::of_one(
                SourceRetention::Persisted,
                SourceSeekability::SequentialOnly,
            )),
        ))
        .unwrap();
    let crate::SourceLifecycle::Staging {
        offer: commissioned,
        plan: StagingPlan::ProduceOwnedArtifact { derivation },
        ..
    } = owned.source.clone()
    else {
        panic!("a sequential source did not commission a production");
    };
    let (_blobs, sealed) = sealed_artifact(
        owned.identity.card,
        owned.generation,
        owned.identity.artifact,
        &[7_u8; 24],
        derivation.fingerprint(&commissioned),
    );
    let stamp = owned.stamp();
    owned
        .reduce(ProductInput::StageComplete {
            stamp,
            content: crate::StagedContent::new(
                crate::TransferContent::new(
                    OfferedName::from_untrusted(STAGED_NAME).expect("a bounded name"),
                    sealed.length(),
                ),
                sealed.digest(),
            ),
            possession: SourcePossession::Derived(sealed),
        })
        .unwrap();
    let before = owned.source.clone();

    let effects = owned.reduce(ProductInput::Restore).unwrap();

    assert_eq!(
        owned.source, before,
        "an owned artifact was disturbed by a restart"
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, ProductEffect::StartAttempt { .. })),
        "a ready owned artifact did not start its attempt: {effects:?}"
    );
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            ProductEffect::CapabilityDuty {
                action: CapabilityAction::AcquireSource,
                ..
            }
        )),
        "an owned artifact asked the platform for a document it does not hold"
    );
}

/// The other half: a `Process` grant promised THIS process, and this is a
/// different one — so the INPUT is gone for good. But the OUTPUT may not be, and
/// that is a different question, asked of a different thing.
///
/// So restore re-commissions the same production rather than asking the platform
/// for a document it cannot have. The worker adopts a seal published before the
/// crash — durable, immutable, produced under this exact commissioning — and
/// answers `Failed` when there is nothing to adopt, which lands on the same
/// re-pick the platform round trip would have reached, minus throwing away an
/// artifact the card already owns.
#[test]
fn a_process_only_production_asks_the_bulk_store_before_giving_up() {
    let (mut card, _) = create(Direction::Send);
    card.reduce(ProductInput::SourceOffered {
        offer: offer(&card, STAGED_NAME, None),
    })
    .unwrap();
    card.reduce(settled(
        &card.clone(),
        SourceReport::Acquired(AcquiredSelection::of_one(
            SourceRetention::Process,
            SourceSeekability::Seekable,
        )),
    ))
    .unwrap();
    let crate::SourceLifecycle::Staging { offer, plan, .. } = card.source.clone() else {
        panic!("a process grant did not commission a production");
    };
    let StagingPlan::ProduceOwnedArtifact { derivation } = plan else {
        panic!("a process grant is not streamable: {plan:?}");
    };

    let effects = card.reduce(ProductInput::Restore).unwrap();

    // Still staging, and commissioned with the SAME fingerprint — a different
    // one would make the seal it is trying to adopt ineligible.
    assert!(matches!(
        card.source,
        crate::SourceLifecycle::Staging { .. }
    ));
    assert_eq!(
        card.quiescence,
        Quiescence::Running {
            worker: WorkerKind::Staging
        }
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            ProductEffect::StartSourceStaging { plan }
                if plan.work == crate::StagingWork::Produce {
                    artifact: card.identity.artifact,
                    derivation,
                    fingerprint: derivation.fingerprint(&offer),
                }
        )),
        "the production was not re-commissioned: {effects:?}"
    );
}

/// A card that has just been minted can be cancelled, and restarted afterwards.
///
/// It is `Quiescent + Preparing` — a shape that could not occur while every
/// `Preparing` card claimed a staging worker, so the quiescent cancel arm did
/// not cover it. The existing preparing-cancel test starts from `Staging` and
/// cannot catch that arm being narrowed back.
#[test]
fn a_minted_send_can_be_cancelled_and_then_asked_again() {
    let (mut card, _) = create(Direction::Send);
    assert_eq!(card.quiescence, Quiescence::Quiescent);
    assert!(
        card.allowed_commands().contains(&ProductCommand::Cancel),
        "a minted send cannot be cancelled: {:?}",
        card.allowed_commands()
    );

    card.reduce(ProductInput::Command(ProductCommand::Cancel))
        .unwrap();
    assert_eq!(card.state, ProductState::Cancelled);

    // Restarting it means asking for a document again — there is no offset to
    // resume from and nothing to send. Re-pick is offered, and it is the
    // LIFECYCLE that says so: this card never carried a `RePickSource` recovery
    // hint, because nothing failed.
    assert!(
        card.outcome.as_ref().and_then(|outcome| outcome.recovery) != Some(Recovery::RePickSource)
    );
    assert!(
        card.allowed_commands()
            .contains(&ProductCommand::RePickSource),
        "a cancelled send cannot ask for a document again: {:?}",
        card.allowed_commands()
    );
    let generation = card.generation;
    card.reduce(ProductInput::Command(ProductCommand::RePickSource))
        .unwrap();
    assert_eq!(card.state, ProductState::Preparing);
    assert_ne!(card.generation, generation, "the discharged key was reused");

    // And the fresh ask can actually be answered.
    let offered = ProductInput::SourceOffered {
        offer: offer(&card, STAGED_NAME, None),
    };
    card.reduce(offered).unwrap();
    assert!(matches!(card.source, crate::SourceLifecycle::Acquiring(_)));
}

/// Wraps a hand-written body in the v6 envelope, so a fixture is BYTES rather
/// than a constructed value. The constructors already refuse these; only a
/// hostile storage editor can still write them, and this is the boundary that
/// has to say no.
fn hostile_record(body: &str) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&23_u16.to_be_bytes());
    encoded.extend_from_slice(b"envoix/product-record/1");
    encoded.extend_from_slice(&crate::PRODUCT_RECORD_VERSION.to_be_bytes());
    encoded.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    encoded.extend_from_slice(body.as_bytes());
    encoded
}

/// The body of the byte-exact fixture, with one substring replaced. Every
/// hostile case below is the VALID record differing in exactly one place, so a
/// refusal cannot be passing for an unrelated reason.
fn fixture_body_with(from: &str, to: &str) -> String {
    let encoded = encode_record(&fixture_record()).unwrap();
    let body = String::from_utf8(encoded[33..].to_vec()).unwrap();
    assert!(body.contains(from), "the fixture body has no {from:?}");
    body.replace(from, to)
}

/// Bytes a hostile editor can write, and the decoder refuses.
///
/// These are the combinations the checked constructors make unbuildable in
/// Rust. Testing the constructors alone proves nothing about them: storage is
/// not a constructor, and until v6 every one of these DECODED — a receiver
/// holding a send source, an acquisition belonging to another card, and a
/// process-only grant claiming a provider it could reopen.
#[test]
fn hostile_v8_bytes_are_refused_one_invariant_at_a_time() {
    // Sanity: the unmodified body is accepted, so each refusal below is caused
    // by its own edit rather than by the harness.
    assert!(matches!(
        decode_record(&hostile_record(&fixture_body_with("\"send\"", "\"send\""))),
        Ok(RecordDecode::Loaded(_))
    ));

    let cases: [(&str, &str, &str, RecordInvariant); 5] = [
        (
            "a receiver cannot hold a send source",
            "\"direction\":\"send\"",
            "\"direction\":\"receive\"",
            RecordInvariant::DirectionDisagreesWithSource,
        ),
        (
            "an acquisition belonging to another card",
            "\"key\":{\"card\":1",
            "\"key\":{\"card\":2",
            RecordInvariant::ForeignAcquisition,
        ),
        (
            "an acquisition ahead of the record that would have issued it",
            "\"generation\":7,\"request\"",
            "\"generation\":8,\"request\"",
            RecordInvariant::ForeignAcquisition,
        ),
        (
            "an acquisition minted for a request this record never derives",
            "\"request\":\"656e766f69782f736f757263652f7635\"",
            "\"request\":\"ffffffffffffffffffffffffffffffff\"",
            RecordInvariant::ForeignAcquisition,
        ),
        (
            "progress past the counted total",
            "\"bytes\":4",
            "\"bytes\":11",
            RecordInvariant::ProgressExceedsTotal,
        ),
    ];
    for (what, from, to, invariant) in cases {
        assert_eq!(
            decode_record(&hostile_record(&fixture_body_with(from, to))),
            Err(RecordCodecError::InvalidRecord(invariant)),
            "{what}"
        );
    }

    // The impossible retention products are refused by the lifecycle DTO before
    // the record validator sees them, so they surface as a malformed body
    // rather than an invariant — the conversion is where they die.
    for (what, from, to) in [
        (
            "a process grant claiming a provider it can reopen",
            "\"retention\":\"persisted\"",
            "\"retention\":\"process\"",
        ),
        (
            "answers that describe a different selection than the offer beside them",
            "\"acquired\":[{\"item\":0,\"retention\":\"persisted\",\"seekability\":\"seekable\"}]",
            "\"acquired\":[{\"item\":0,\"retention\":\"persisted\",\"seekability\":\"seekable\"},{\"item\":1,\"retention\":\"persisted\",\"seekability\":\"seekable\"}]",
        ),
        (
            "a byte count past what this product can carry end to end",
            "\"total\":10",
            "\"total\":9223372036854775808",
        ),
    ] {
        assert_eq!(
            decode_record(&hostile_record(&fixture_body_with(from, to))),
            Err(RecordCodecError::MalformedBody),
            "{what}"
        );
    }
}
