//! Host boot: durable cards restore and the destructive outbox drains
//! AFTER restore, at-least-once, across process generations — through the
//! card's one live store, with the duty ledger's generation established from
//! durable truth.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use envoix_attempt_api::{AttemptEvent, AttemptEventKind, AttemptSupervisor, EventAdmission};
use envoix_bindings::command::{
    CommandBody, CommandFrame, CreateIntentView, CreateView, FrontendIntentView,
    LocalDirectionView, MintRoomView, SourceAcquisitionKeyView, SourceOfferAnswerView,
    SourceOfferOutcomeView, SourceOfferView, decode_command_frame, encode_command_frame,
};
use envoix_bindings::read::{
    CardActionView, CardUpdateKindView, ReadBody, SourceLifecycleView, SourceSelectionGateView,
    decode_read_frame,
};
use envoix_capabilities::{
    DutyProvenance, SourceAcquisitionKey, SourceReport, SourceRetention, SourceSeekability,
};
use envoix_host_android::{CardStores, FramePoll, Host, HostStore};
use envoix_operation_store::{ArtifactKey, OperationStore, PossessionState};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_platform_android::{Work, WorkOrder, WorkReport};
use envoix_product::{
    CommittedSession, NewTransfer, ProductCommand, ProductEffect, ProductInput, ProductState,
    SourceBacking, SourceLifecycle, SourcePromptReason, SystemIdentitySource, TransferRecord,
};
use envoix_storage_api::Durability;
use envoix_storage_local::LocalStorage;
use envoix_types::{AttemptGen, Direction, OfferedName, RecordId, RequestId};

fn open_store(root: &std::path::Path, card: RecordId) -> OperationStore<LocalStorage> {
    let storage = LocalStorage::open(root).expect("storage opens");
    OperationStore::open(storage, card).expect("operation store opens")
}

/// Boot restores every durable card, then replays the pending destructive
/// outbox exactly as a crashed drain would need: settlement is one durable
/// image, a second boot finds nothing left to replay, and re-staging the same
/// artifact key never re-arms the settled operation against fresh bytes.
#[test]
fn boot_restores_cards_and_drains_the_outbox() {
    let root = tempfile::tempdir().expect("tempdir");

    // Process generation 1: create one durable card, stage a partial
    // artifact, queue its discard — then die WITHOUT executing the outbox
    // (the crash window the drainer owns).
    let card = {
        let host = Host::boot(root.path()).expect("first boot");
        let card = host.create_for_e2e().expect("card creation commits");
        host.shutdown();
        card
    };
    let key = {
        let mut operation = open_store(root.path(), card);
        let record = operation.latest_record().expect("a committed record");
        let decoded = match envoix_product::decode_record(record).expect("decodes") {
            envoix_product::RecordDecode::Loaded(record) => record,
            envoix_product::RecordDecode::UnsupportedFuture { .. } => {
                panic!("fresh record cannot be future")
            }
        };
        let key = ArtifactKey {
            transfer: decoded.identity.transfer,
            artifact: decoded.identity.artifact,
        };
        operation
            .stage_artifact(
                key,
                OfferedName::from_untrusted("crash-victim.bin").unwrap(),
                b"partial",
            )
            .expect("stages");
        operation.queue_discard_partial(key).expect("queues");
        assert_eq!(operation.replayable_outbox().len(), 1, "the op is pending");
        key
    };

    // Process generation 2: boot drains the queue AFTER restore.
    {
        let host = Host::boot(root.path()).expect("second boot");
        host.shutdown();
    }
    {
        let operation = open_store(root.path(), card);
        assert!(
            operation.replayable_outbox().is_empty(),
            "the pending discard was executed and confirmed"
        );
        assert!(
            operation.possession(key).is_none()
                || !matches!(
                    operation.possession(key).expect("checked").state(),
                    PossessionState::Partial
                ),
            "the partial possession fact left the durable image"
        );
        assert!(
            operation.latest_record().is_some(),
            "the card's durable record survives the drain"
        );
    }

    // Process generation 3: a drained queue re-drains as a no-op.
    {
        let host = Host::boot(root.path()).expect("third boot");
        host.shutdown();
        let operation = open_store(root.path(), card);
        assert!(operation.replayable_outbox().is_empty());
    }

    // Process generation 4: the SAME artifact key is staged again. The settled
    // discard must not resurrect — the two-write execute/confirm split left it
    // pending and hidden, and this fresh partial would satisfy its safety
    // predicate on the next drain.
    {
        let mut operation = open_store(root.path(), card);
        operation
            .stage_artifact(
                key,
                OfferedName::from_untrusted("crash-victim.bin").unwrap(),
                b"fresh-partial",
            )
            .expect("stages again");
        assert!(operation.replayable_outbox().is_empty());
    }
    {
        let host = Host::boot(root.path()).expect("fourth boot");
        host.shutdown();
    }
    {
        let operation = open_store(root.path(), card);
        assert!(
            matches!(
                operation
                    .possession(key)
                    .expect("the fresh partial")
                    .state(),
                PossessionState::Partial
            ),
            "the freshly staged partial survives the boot drain"
        );
    }
}

/// The composition root hands out ONE live store per card, so the session
/// provider behind a card actor and the boot outbox drainer write through the
/// same handle. Two `OperationStore`s over one root hold independent in-memory
/// writer leases: their cached images fork and their writes can lose updates.
#[test]
fn one_live_store_per_card_serializes_every_writer() {
    let root = tempfile::tempdir().expect("tempdir");
    let stores = CardStores::new(root.path().to_path_buf());
    let card = RecordId::new(0x5150);

    let provider_side = stores.open(card).expect("the store opens");
    let drain_side = stores.open(card).expect("the same live store");
    assert!(
        Arc::ptr_eq(&provider_side, &drain_side),
        "one live store per card"
    );

    // A commit through one holder is immediately the truth the other reads —
    // separate stores would each answer from their own cached image.
    provider_side
        .lock()
        .expect("uncontended")
        .commit_record(b"revision-1", Durability::Durable)
        .expect("commits");
    assert_eq!(
        drain_side.lock().expect("uncontended").latest_record(),
        Some(b"revision-1".as_slice())
    );

    // Concurrent writers serialize instead of racing to the same revision.
    let writers: Vec<_> = (0..2u8)
        .map(|writer| {
            let store = stores.open(card).expect("the same live store");
            std::thread::spawn(move || {
                for revision in 0..32u8 {
                    store
                        .lock()
                        .expect("a writer never panics")
                        .commit_record(&[writer, revision], Durability::Durable)
                        .expect("commits");
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("the writer finishes");
    }
    assert_eq!(
        provider_side
            .lock()
            .expect("uncontended")
            .record_revision_count(),
        65,
        "every commit landed on its own revision"
    );
}

/// A real duty round trip on the production path: the host establishes the
/// card's ledger generation from durable truth, so the receipt duty the
/// reducer re-emits on restore REGISTERS, dispatches a typed work order, and
/// the service's report is admitted exactly once.
#[test]
fn restored_receipt_duty_dispatches_and_admits_its_report() {
    let root = tempfile::tempdir().expect("tempdir");
    let card = commit_completed_receiver(root.path());

    let host = Host::boot(root.path()).expect("boot");
    let bytes = poll_work(&host).expect("the restored receipt duty dispatches");
    let order = WorkOrder::decode(&bytes).expect("a typed work order");
    assert_eq!(order.work, Work::Courier);
    assert_eq!(order.provenance.card.value(), card.get());

    let report = WorkReport::new(order.provenance.to_provenance(), OutcomeCode::Internal)
        .encode()
        .expect("encodes");
    assert!(
        host.report_duty(&report),
        "the ledger admits the report against the host-established generation"
    );
    assert!(
        !host.report_duty(&report),
        "a replayed report is never admitted twice"
    );

    // A fresh attachment re-delivers the still-outstanding duty; a discharged
    // duty must not be dispatched to the service again.
    host.attach(card);
    assert!(
        poll_work(&host).is_none(),
        "re-delivery does not re-dispatch"
    );
    host.shutdown();
}

/// Commits a durable card in the one state that re-emits a capability duty on
/// restore: a completed receiver whose receipt is not yet delivered.
fn commit_completed_receiver(root: &std::path::Path) -> RecordId {
    let stores = CardStores::new(root.to_path_buf());
    let transfer = NewTransfer {
        direction: Direction::Receive,
        participation: envoix_product::RoomParticipation::Minted,
        pairing: None,
    };
    let (mut session, outcome) = CommittedSession::create(
        transfer,
        &mut SystemIdentitySource,
        HostStore::deferred(stores),
        NonZeroUsize::new(3).expect("nonzero"),
    )
    .expect("creation commits");
    let plan = outcome
        .released_immediately
        .iter()
        .chain(outcome.released_after_commit.iter())
        .find_map(|effect| match effect {
            ProductEffect::StartAttempt { plan } => Some(*plan),
            _ => None,
        })
        .expect("creation starts an attempt");

    let mut supervisor = AttemptSupervisor::new();
    supervisor.open(plan);
    for kind in [
        AttemptEventKind::Phase(Phase::Transferring),
        AttemptEventKind::Terminal(OutcomeCode::Completed),
    ] {
        let EventAdmission::Accepted(admitted) = supervisor.observe(AttemptEvent {
            stamp: plan.stamp,
            kind,
        }) else {
            panic!("the live attempt's events are admitted");
        };
        session
            .apply(ProductInput::AttemptObserved(admitted))
            .expect("applies");
    }
    assert_eq!(session.record().state, ProductState::Completed);
    session.record().identity.card
}

/// One dispatched work order, or `None` once the lane has stayed idle.
fn poll_work(host: &Host) -> Option<Vec<u8>> {
    for _ in 0..100 {
        if let Some(order) = host.poll_work() {
            return Some(order);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// A freshly created card is live in the booting process and observable
/// through the host's own attachment (the read lane's source of truth).
#[test]
fn created_card_survives_reboot_and_reattaches() {
    let root = tempfile::tempdir().expect("tempdir");
    let card = {
        let host = Host::boot(root.path()).expect("first boot");
        let card = host.create_for_e2e().expect("creates");
        host.shutdown();
        card
    };
    let host = Host::boot(root.path()).expect("reboot");
    // The restored card produced a fresh attachment; frames flow from
    // durable truth (snapshot-first per epoch), so the frame lane yields at
    // least one encoded read frame for the card. Only an attachment can drain
    // it, so the token an observer would hold is what asks.
    let token = host.open_lane();
    let mut saw_frame = false;
    for _ in 0..100 {
        if let FramePoll::Frame(bytes) = host.poll_frame(token) {
            let text = String::from_utf8(bytes).expect("frames are UTF-8 JSON");
            assert!(text.contains(envoix_bindings::read::READ_SCHEMA_ID));
            saw_frame = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(saw_frame, "the restored card surfaces on the frame lane");
    host.shutdown();
    let _ = card;
}

/// URI ownership follows durable card ownership. A removal queues the release
/// once in this process, and a crash after delivery but before Android releases
/// the grant is closed by reseeding the same id from the removal record.
#[test]
fn durable_removal_replays_its_source_grant_release_after_restart() {
    let root = tempfile::tempdir().expect("tempdir");
    let stores = CardStores::new(root.path().to_path_buf());
    let transfer = NewTransfer {
        direction: Direction::Send,
        participation: envoix_product::RoomParticipation::Minted,
        // A freshly created sender is quiescent — it is waiting for a person to
        // choose a document — so Remove can commit its tombstone without a
        // worker acknowledgement obscuring what this test owns.
        pairing: None,
    };
    let (mut session, _) = CommittedSession::create(
        transfer,
        &mut SystemIdentitySource,
        HostStore::deferred(stores),
        NonZeroUsize::new(3).expect("nonzero"),
    )
    .expect("creation commits");
    let card = session.record().identity.card;
    session
        .apply(ProductInput::Command(ProductCommand::Remove))
        .expect("removal commits");
    drop(session);

    for generation in 1..=2 {
        let host = Host::boot(root.path()).expect("host restores removal truth");
        assert_eq!(
            host.poll_source_release(),
            Some(card),
            "process generation {generation} replays the release"
        );
        assert_eq!(
            host.poll_source_release(),
            None,
            "one process delivers one id once"
        );
        host.shutdown();
    }
}

/// The acquisition a card PUBLISHED, read off its `pick_source` action.
fn published_acquisition(
    host: &Host,
    token: envoix_host_android::AttachmentToken,
) -> SourceAcquisitionKeyView {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match host.poll_frame(token) {
            FramePoll::Frame(bytes) => {
                let ReadBody::CardUpdate(update) =
                    decode_read_frame(&bytes).expect("a read frame").body
                else {
                    continue;
                };
                let CardUpdateKindView::Snapshot(view) = update.kind else {
                    continue;
                };
                for action in view.allowed_actions {
                    if let CardActionView::PickSource(pick) = action {
                        return SourceAcquisitionKeyView {
                            card: pick.acquisition.card,
                            generation: pick.acquisition.generation,
                            request: pick.acquisition.request,
                        };
                    }
                }
            }
            FramePoll::Drained => std::thread::sleep(Duration::from_millis(25)),
            FramePoll::Superseded => panic!("the attachment was superseded"),
        }
    }
    panic!("the card never published a pick_source action");
}

fn offer_bytes(key: &SourceAcquisitionKeyView, name: &str, size: Option<u64>) -> Vec<u8> {
    encode_command_frame(&CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::SourceOffer(SourceOfferView {
            key: key.clone(),
            display_name: name.to_owned(),
            reported_size: size,
        })),
    })
    .expect("the offer encodes")
}

fn offer_outcome(answer: &[u8]) -> SourceOfferOutcomeView {
    let CommandBody::SourceOfferResult(result) =
        decode_command_frame(answer).expect("a command frame").body
    else {
        panic!("the authority answered an offer with something else");
    };
    result.outcome
}

/// A picked document reaches the AUTHORITY and moves the card.
///
/// This is the seam that was refused: the host decoded a well-formed source
/// offer and answered `contract breach`, so a frontend that published a
/// `pick_source` action could never answer it. The whole round trip is here —
/// the acquisition the card publishes, an offer built from it, the answer, and
/// the durable record showing the card acquiring.
#[test]
fn a_source_offer_reaches_the_authority_and_moves_the_card() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    host.intent(
        &encode_command_frame(&CommandFrame {
            body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
                intent: CreateIntentView::MintRoom(MintRoomView {
                    local_direction: LocalDirectionView::Send,
                }),
                request_id: "11".repeat(16),
            })),
        })
        .expect("the create encodes"),
    )
    .expect("the authority answers the create");

    // The acquisition the card published, as a frontend reads it off the
    // `pick_source` action rather than deriving anything.
    let token = host.open_lane();
    let acquisition = published_acquisition(&host, token);

    let answer = host
        .intent(&offer_bytes(&acquisition, "chosen.bin", Some(4096)))
        .expect("the authority answers the offer");
    assert_eq!(
        offer_outcome(&answer),
        SourceOfferOutcomeView::Answered(SourceOfferAnswerView::Accepted),
        "a document offered to the acquisition the card published was refused"
    );

    // The same offer again is an idempotent retry, not a second binding.
    let repeat = host
        .intent(&offer_bytes(&acquisition, "chosen.bin", Some(4096)))
        .expect("the authority answers the repeat");
    assert_eq!(
        offer_outcome(&repeat),
        SourceOfferOutcomeView::Answered(SourceOfferAnswerView::AlreadyAccepted)
    );

    // The same key with DIFFERENT metadata was never committed, and saying
    // "already accepted" would tell a frontend its payload had been applied.
    let conflicting = host
        .intent(&offer_bytes(&acquisition, "other.bin", Some(4096)))
        .expect("the authority answers the conflict");
    assert_eq!(
        offer_outcome(&conflicting),
        SourceOfferOutcomeView::Answered(SourceOfferAnswerView::Conflict)
    );

    // And an acquisition the card never published binds nothing.
    let mut elsewhere = acquisition.clone();
    elsewhere.generation = elsewhere.generation.wrapping_add(1);
    let stale = host
        .intent(&offer_bytes(&elsewhere, "chosen.bin", Some(4096)))
        .expect("the authority answers the stale offer");
    assert_eq!(
        offer_outcome(&stale),
        SourceOfferOutcomeView::Answered(SourceOfferAnswerView::Stale)
    );

    host.shutdown();
}

/// The platform's answer moves the card out of `Acquiring`.
///
/// The edge that had no producer at all: the duty lane could carry an outcome
/// code and nothing else, so the ledger refused every source report as
/// `Incompatible` and a card stopped at `Acquiring` forever. duty/2 gives the
/// lane the vocabulary; this drives the whole round trip through the SHIPPED
/// codec — the order the host dispatched, the report the executor would encode,
/// the ledger, and the reducer.
#[test]
fn an_admitted_acquisition_moves_the_card_out_of_acquiring() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    host.intent(
        &encode_command_frame(&CommandFrame {
            body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
                intent: CreateIntentView::MintRoom(MintRoomView {
                    local_direction: LocalDirectionView::Send,
                }),
                request_id: "22".repeat(16),
            })),
        })
        .expect("the create encodes"),
    )
    .expect("the authority answers the create");

    let token = host.open_lane();
    let acquisition = published_acquisition(&host, token);
    host.intent(&offer_bytes(&acquisition, "chosen.bin", Some(4096)))
        .expect("the authority answers the offer");

    // Accepting the offer commissions the handle duty. The order is what the
    // service executor would receive.
    let order = poll_work(&host).expect("an accepted offer dispatches the handle duty");
    let order = WorkOrder::decode(&order).expect("the order decodes");
    assert_eq!(order.work, Work::SourceHandle);

    // The answer an executor gives once it has taken hold: both facts, because
    // both are what the stream-versus-copy decision reads.
    let report = WorkReport::source(
        order.provenance.to_provenance(),
        SourceReport::Acquired {
            retention: SourceRetention::Persisted,
            seekability: SourceSeekability::Seekable,
        },
    );
    assert!(
        host.report_duty(&report.encode().expect("the report encodes")),
        "a typed acquisition was not admitted"
    );

    // And the card moved. Streaming, because the platform promised a persisted
    // grant on a seekable source — the one product of the four that needs no
    // copy.
    // Nothing bound a source for this acquisition, so the worker opens nothing
    // and answers `Failed`. The card RESTS at re-pick, and that resting state is
    // the whole proof: the reason is `StagingFailed`, which is reachable from
    // `Staging` and from nowhere else — so a card carrying it left `Acquiring`,
    // which no card could do before duty/2.
    //
    // The transit itself is deliberately NOT asserted. Replaceable projection
    // updates coalesce, so a state a card passes through in microseconds may
    // legitimately never be published; waiting for one is waiting on something
    // the lane does not promise.
    drain_until_source(&host, token, |source| {
        matches!(
            source,
            SourceLifecycleView::AwaitingSelection(view)
                if matches!(view.selection, SourceSelectionGateView::RePickRequired(_))
        )
    });
    let card = order.provenance.to_provenance().card;
    host.shutdown();
    let record = durable_record(root.path(), card);
    let SourceLifecycle::AwaitingSelection(gate) = &record.source else {
        panic!(
            "a card no host can stage did not return to asking: {:?}",
            record.source
        );
    };
    assert_eq!(gate.reason(), SourcePromptReason::StagingFailed);
}

/// Watches the LANE until the card's published source reaches `want`.
///
/// Through the frame lane, not the store. A test that polls the operation store
/// while the host is live opens a SECOND store on a card that already has one,
/// which is exactly what `one_live_store_per_card_serializes_every_writer`
/// forbids — and it reads torn revisions for its trouble. What a frontend sees
/// is observable; what is on disk is checked once, after shutdown.
fn drain_until_source(
    host: &Host,
    token: envoix_host_android::AttachmentToken,
    want: fn(&SourceLifecycleView) -> bool,
) -> SourceLifecycleView {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        match host.poll_frame(token) {
            FramePoll::Frame(bytes) => {
                let ReadBody::CardUpdate(update) =
                    decode_read_frame(&bytes).expect("a read frame").body
                else {
                    continue;
                };
                let source = match update.kind {
                    CardUpdateKindView::Snapshot(view)
                    | CardUpdateKindView::Progress(view)
                    | CardUpdateKindView::State(view)
                    | CardUpdateKindView::Terminal(view) => view.source,
                    CardUpdateKindView::CapabilityDuty(_) => continue,
                };
                if want(&source) {
                    return source;
                }
                last = Some(source);
            }
            FramePoll::Drained => std::thread::sleep(Duration::from_millis(25)),
            FramePoll::Superseded => panic!("the attachment was superseded"),
        }
    }
    panic!("the card never reached the expected source state; it rested at {last:?}");
}

/// The durable record, read ONCE with no live host competing for the store.
fn durable_record(root: &std::path::Path, card: RecordId) -> TransferRecord {
    let operation = open_store(root, card);
    let bytes = operation.latest_record().expect("a committed record");
    let envoix_product::RecordDecode::Loaded(record) =
        envoix_product::decode_record(bytes).expect("the durable record decodes")
    else {
        panic!("this build wrote a record it cannot read");
    };
    *record
}

/// A card reaches `Ready`, and `Ready` says which bytes.
///
/// The edge that had no producer AND no port. `Staging → Ready` needs a counted
/// total and a digest of the bytes that were counted — without the digest,
/// "staged" means only "once observed a length", and a provider could swap the
/// document across a restart. Nothing read anything, so no card had ever left
/// `Staging`.
///
/// Driven through the real worker over a real file, so what is asserted is what
/// the reader actually produced.
#[test]
fn staging_reads_the_source_through_and_ready_says_which_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    // Larger than the worker's read chunk, so the loop runs more than once and
    // progress is a sequence rather than a single report.
    let contents: Vec<u8> = (0..(600 * 1024_u32)).map(|byte| byte as u8).collect();
    let source = root.path().join("chosen.bin");
    std::fs::write(&source, &contents).expect("the source is written");

    let host = Host::boot(root.path()).expect("the host boots");
    host.intent(
        &encode_command_frame(&CommandFrame {
            body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
                intent: CreateIntentView::MintRoom(MintRoomView {
                    local_direction: LocalDirectionView::Send,
                }),
                request_id: "33".repeat(16),
            })),
        })
        .expect("the create encodes"),
    )
    .expect("the authority answers the create");

    let token = host.open_lane();
    let acquisition = published_acquisition(&host, token);
    // The platform binds the document under the acquisition that asked for it.
    // A filesystem host binds a PATH; Android binds a detached descriptor
    // through the JNI lane. Same registry, same key, two platform answers.
    host.sources().bind(
        SourceAcquisitionKey::of(DutyProvenance {
            card: RecordId::new(u64::from_str_radix(&acquisition.card, 16).expect("hex card")),
            generation: AttemptGen::new(acquisition.generation),
            request: RequestId::from_bytes(
                u128::from_str_radix(&acquisition.request, 16)
                    .expect("hex request")
                    .to_be_bytes(),
            ),
        }),
        source,
    );
    host.intent(&offer_bytes(&acquisition, "chosen.bin", Some(1)))
        .expect("the authority answers the offer");

    let order = poll_work(&host).expect("an accepted offer dispatches the handle duty");
    let order = WorkOrder::decode(&order).expect("the order decodes");
    let report = WorkReport::source(
        order.provenance.to_provenance(),
        SourceReport::Acquired {
            retention: SourceRetention::Persisted,
            seekability: SourceSeekability::Seekable,
        },
    );
    assert!(host.report_duty(&report.encode().expect("the report encodes")));

    drain_until_source(&host, token, |source| {
        matches!(source, SourceLifecycleView::Ready(_))
    });

    let card = order.provenance.to_provenance().card;
    host.shutdown();
    let record = durable_record(root.path(), card);
    let SourceLifecycle::Ready { content, .. } = &record.source else {
        panic!("the card did not reach ready: {:?}", record.source);
    };
    // Streaming: the platform promised a persisted grant on a seekable source,
    // the one product of the four that needs no copy — so nothing was written.
    let SourceLifecycle::Ready { backing, .. } = &record.source else {
        unreachable!("matched above")
    };
    assert_eq!(*backing, SourceBacking::PersistedProvider);

    // COUNTED, not claimed. The offer reported one byte; the authority never
    // promoted that, and what `Ready` carries is what the worker read.
    assert_eq!(content.total().get(), contents.len() as u64);
    assert_eq!(
        content.content_hash(),
        envoix_product::ContentHash::from_bytes(*blake3::hash(&contents).as_bytes()),
        "Ready identifies bytes the worker did not read"
    );
    // And the name is the accepted offer's, normalized by the authority — a
    // worker does not get to rename the document it read.
    assert_eq!(content.name().as_str(), "chosen.bin");
}
