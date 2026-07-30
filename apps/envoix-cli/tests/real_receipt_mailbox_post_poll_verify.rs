use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, RetirementAckResult, RetirementIntent, RetirementRequestResult,
};
use envoix_capabilities::{
    Admission, Duty, DutyKind, DutyLedger, DutyProvenance, DutyReport, DutyResult,
    GenerationUpdate, Registration, SourceAcquisitionKey, SourceReport, SourceRetention,
    SourceSeekability,
};
use envoix_mailbox::{HttpReceiptMailbox, MailboxClientError};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_pairing::{
    EntropyError, EntropySource, Paired, PairingCode, initiator_start, responder_respond,
};
use envoix_product::{
    AcceptedSourceOffer, ApplyOutcome, CapabilityAction, CommitError, CommittedSession,
    IdentityError, IdentitySource, NewTransfer, ProductEffect, ProductInput, ProductState,
    RecordStore, StagedContent, TransferContent,
};
use envoix_protocol::mailbox::{
    MailboxProtocolError, ReceiptPayload, SealedReceipt, open_receipt, receipt_slot, seal_receipt,
};
use envoix_protocol::{Complete, ContentHash};
use envoix_server::{ServerConfig, run};
use envoix_types::{ByteCount, Direction, OfferedName, TransferId};
use tempfile::TempDir;

#[derive(Default)]
struct MemoryRecordStore;

impl RecordStore for MemoryRecordStore {
    fn commit(&mut self, _encoded: &[u8]) -> Result<(), CommitError> {
        Ok(())
    }
}

struct FixedBytes {
    next: u8,
}

impl FixedBytes {
    const fn new(seed: u8) -> Self {
        Self { next: seed }
    }

    fn write(&mut self, destination: &mut [u8]) {
        for byte in destination {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
    }
}

impl EntropySource for FixedBytes {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        self.write(destination);
        Ok(())
    }
}

impl IdentitySource for FixedBytes {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        self.write(destination);
        Ok(())
    }
}

fn complete_pair(code: &str, initiator_seed: u8, responder_seed: u8) -> (Paired, Paired) {
    let initiator_code = PairingCode::new(code.as_bytes().to_vec()).unwrap();
    let responder_code = PairingCode::new(code.as_bytes().to_vec()).unwrap();
    let (initiator, start) =
        initiator_start(&initiator_code, &mut FixedBytes::new(initiator_seed)).unwrap();
    let (responder, response) = responder_respond(
        &responder_code,
        &start,
        &mut FixedBytes::new(responder_seed),
    )
    .unwrap();
    let (initiator, initiator_confirmation) = initiator.receive_response(&response).unwrap();
    let (responder, responder_confirmation) =
        responder.verify_initiator(&initiator_confirmation).unwrap();
    let initiator = initiator.verify_responder(&responder_confirmation).unwrap();
    (initiator, responder)
}

fn server_config(root: &Path) -> ServerConfig {
    let mut config = ServerConfig::operational_defaults();
    config.bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.mailbox_bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.node_key_path = root.join("rendezvous-node.key");
    config.close_grace = Duration::from_secs(2);
    config.bind_deadline = Duration::from_secs(2);
    config
}

/// A card as a frontend states it. No name, total or source decision crosses
/// here any more: the authority derives the opening source state from the
/// direction, and the name and total arrive with the document.
fn new_transfer(direction: Direction) -> NewTransfer {
    NewTransfer {
        direction,
        participation: envoix_product::RoomParticipation::Minted,
        pairing: None,
    }
}

/// The acquisition key a frontend actually has: it arrives on the published
/// source duty, which is how a real frontend learns which one it is answering.
fn published_acquisition(outcome: &ApplyOutcome) -> DutyProvenance {
    outcome
        .released_after_commit
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::CapabilityDuty { duty, action } => {
                (*action == CapabilityAction::SelectSource).then_some(duty.provenance)
            }
            _ => None,
        })
        .expect("a card that needs a source publishes the duty that asks for one")
}

/// Walks a sending session from "needs a document" to a live attempt, and
/// returns the outcome that LAUNCHED it. A sender has no shorter route: the
/// lifecycle is the only source authority, so nothing can declare it ready.
fn stage_the_source<S: RecordStore>(
    session: &mut CommittedSession<S>,
    created: &ApplyOutcome,
    offered_name: &OfferedName,
    total: ByteCount,
) -> ApplyOutcome {
    let provenance = published_acquisition(created);
    session
        .apply(ProductInput::SourceOffered {
            offer: AcceptedSourceOffer::new(
                SourceAcquisitionKey::of(provenance),
                offered_name.clone(),
                Some(total),
            ),
        })
        .unwrap();
    let mut ledger = DutyLedger::new();
    ledger.advance_generation(provenance.card, provenance.generation);
    assert_eq!(
        ledger.register(Duty {
            provenance,
            kind: DutyKind::SourceHandle,
        }),
        Registration::Registered
    );
    let Admission::Fresh(admitted) = ledger.admit(DutyResult {
        provenance,
        report: DutyReport::Source(SourceReport::Acquired {
            retention: SourceRetention::Persisted,
            seekability: SourceSeekability::Seekable,
        }),
    }) else {
        panic!("an outstanding source duty admits its first result");
    };
    session
        .apply(ProductInput::SourceSettled(
            admitted.into_source().expect("a source answer"),
        ))
        .unwrap();
    let stamp = session.record().stamp();
    session
        .apply(ProductInput::StageComplete {
            stamp,
            content: StagedContent::new(
                TransferContent::new(offered_name.clone(), total),
                ContentHash::from_bytes([7; 32]),
            ),
        })
        .unwrap();
    session
        .apply(ProductInput::StagingRetired { stamp })
        .unwrap()
}

fn start_plan(outcome: &ApplyOutcome) -> AttemptPlan {
    outcome
        .released_after_commit
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::StartAttempt { plan } => Some(*plan),
            _ => None,
        })
        .expect("create releases a committed attempt plan")
}

fn admitted_event(
    supervisor: &AttemptSupervisor,
    plan: AttemptPlan,
    kind: AttemptEventKind,
) -> ProductInput {
    match supervisor.observe(AttemptEvent {
        stamp: plan.stamp,
        kind,
    }) {
        EventAdmission::Accepted(event) => ProductInput::AttemptObserved(event),
        admission => panic!("current attempt event was not admitted: {admission:?}"),
    }
}

fn receipt_duty(outcome: &ApplyOutcome) -> Duty {
    outcome
        .released_after_commit
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::CapabilityDuty {
                duty,
                action: CapabilityAction::PostReceipt,
            } => Some(*duty),
            _ => None,
        })
        .expect("completed receive releases a receipt duty")
}

fn has_mailbox_poll(outcome: &ApplyOutcome) -> bool {
    outcome
        .released_immediately
        .iter()
        .chain(&outcome.released_after_commit)
        .any(|effect| matches!(effect, ProductEffect::StartMailboxPoll { .. }))
}

#[derive(Clone, Copy)]
struct CommittedSendFact {
    file_hash: ContentHash,
    file_size: ByteCount,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollResult {
    Absent,
    Unopenable,
    Mismatch,
    Verified,
}

async fn poll_sender(
    mailbox: &HttpReceiptMailbox,
    sender: &mut CommittedSession<MemoryRecordStore>,
    stamp: AttemptStamp,
    transfer_id: TransferId,
    mailbox_secret: &[u8; 32],
    committed: CommittedSendFact,
) -> PollResult {
    let sealed = match mailbox.poll(receipt_slot(transfer_id)).await {
        Ok(Some(sealed)) => sealed,
        Ok(None) => return PollResult::Absent,
        Err(MailboxClientError::InvalidBlob) => return PollResult::Unopenable,
        Err(error) => panic!("unexpected mailbox error: {error}"),
    };
    let receipt = match open_receipt(transfer_id, mailbox_secret, &sealed) {
        Ok(receipt) => receipt,
        Err(
            MailboxProtocolError::AuthenticationFailed
            | MailboxProtocolError::InvalidPayload
            | MailboxProtocolError::InvalidSealedReceipt,
        ) => return PollResult::Unopenable,
        Err(MailboxProtocolError::SealFailed) => {
            panic!("opening a receipt cannot produce a seal failure")
        }
    };
    if receipt.file_hash() != committed.file_hash || receipt.file_size() != committed.file_size {
        sender
            .apply(ProductInput::ReceiptMismatch { stamp })
            .unwrap();
        PollResult::Mismatch
    } else {
        sender
            .apply(ProductInput::ReceiptVerified { stamp })
            .unwrap();
        PollResult::Verified
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_receipt_mailbox_post_poll_verify() {
    let directory = TempDir::new().unwrap();
    let server = run(server_config(directory.path())).await.unwrap();
    let mailbox =
        HttpReceiptMailbox::new(&format!("http://{}", server.mailbox_bound_addr())).unwrap();
    let offered_name = OfferedName::from_untrusted("offered-name-is-not-identity.bin").unwrap();
    let total = ByteCount::new(12_345);

    let (mut sender, sender_created) = CommittedSession::create(
        new_transfer(Direction::Send),
        &mut FixedBytes::new(0x10),
        MemoryRecordStore,
        NonZeroUsize::MIN,
    )
    .unwrap();
    // The sender reaches the wire only through a real acquisition.
    let sender_created = stage_the_source(&mut sender, &sender_created, &offered_name, total);
    let sender_plan = start_plan(&sender_created);
    let transfer_id = sender.record().identity.transfer;
    let artifact_id = sender.record().identity.artifact;
    let (mut receiver, receiver_created) = CommittedSession::create_with_identity(
        new_transfer(Direction::Receive),
        transfer_id,
        artifact_id,
        &mut FixedBytes::new(0x70),
        MemoryRecordStore,
        NonZeroUsize::MIN,
    )
    .unwrap();
    let receiver_plan = start_plan(&receiver_created);

    let mut sender_attempt = AttemptSupervisor::new();
    let mut receiver_attempt = AttemptSupervisor::new();
    assert_eq!(sender_attempt.open(sender_plan), OpenResult::Opened);
    assert_eq!(receiver_attempt.open(receiver_plan), OpenResult::Opened);
    sender
        .apply(admitted_event(
            &sender_attempt,
            sender_plan,
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    receiver
        .apply(admitted_event(
            &receiver_attempt,
            receiver_plan,
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    sender
        .apply(admitted_event(
            &sender_attempt,
            sender_plan,
            AttemptEventKind::Progress { transferred: total },
        ))
        .unwrap();
    receiver
        .apply(admitted_event(
            &receiver_attempt,
            receiver_plan,
            AttemptEventKind::Progress { transferred: total },
        ))
        .unwrap();

    let polling = sender
        .apply(admitted_event(
            &sender_attempt,
            sender_plan,
            AttemptEventKind::Phase(Phase::Confirming),
        ))
        .unwrap();
    assert!(has_mailbox_poll(&polling));
    let unconfirmed = sender
        .apply(admitted_event(
            &sender_attempt,
            sender_plan,
            AttemptEventKind::Terminal(OutcomeCode::PeerLost),
        ))
        .unwrap();
    assert_eq!(sender.record().state, ProductState::Unconfirmed);
    assert!(has_mailbox_poll(&unconfirmed));

    assert_eq!(
        receiver_attempt.cross_commit_point(receiver_plan.stamp),
        envoix_attempt_api::CommitPointResult::Crossed
    );
    receiver
        .apply(admitted_event(
            &receiver_attempt,
            receiver_plan,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(
        receiver_attempt.request_retirement(receiver_plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) =
        receiver_attempt.acknowledge_retirement(receiver_plan.stamp)
    else {
        panic!("completed receive retirement must acknowledge");
    };
    let retired = receiver.apply(ProductInput::AttemptRetired(ack)).unwrap();
    let duty = receipt_duty(&retired);
    assert_eq!(duty.kind, DutyKind::Courier);

    // The sender verifies the receipt against ITS OWN committed send fact. In the
    // real product the receiver's `Complete.file_hash` must be computed over the
    // bytes it actually landed (never echoed from the sender), or the identity
    // check would be circular; that hashing lives in the transfer mechanism.
    let complete = Complete {
        transfer_id,
        file_hash: ContentHash::from_bytes([0xA5; 32]),
    };
    let committed_send = CommittedSendFact {
        file_hash: complete.file_hash,
        file_size: total,
    };
    let receipt = ReceiptPayload::new(complete.file_hash, total);
    let (sender_paired, receiver_paired) = complete_pair("812345-amber-anchor", 0x20, 0x80);
    assert_eq!(
        sender_paired.mailbox_secret(),
        receiver_paired.mailbox_secret()
    );
    let slot = receipt_slot(complete.transfer_id);
    assert_eq!(
        poll_sender(
            &mailbox,
            &mut sender,
            sender_plan.stamp,
            transfer_id,
            sender_paired.mailbox_secret().expose(),
            committed_send,
        )
        .await,
        PollResult::Absent
    );

    let correct = seal_receipt(
        complete.transfer_id,
        receiver_paired.mailbox_secret().expose(),
        &receipt,
    )
    .unwrap();

    let mut tampered = correct.as_bytes().to_vec();
    *tampered.last_mut().unwrap() ^= 1;
    let tampered = SealedReceipt::from_bytes(tampered).unwrap();
    mailbox.post(slot, &tampered).await.unwrap();
    assert_eq!(
        poll_sender(
            &mailbox,
            &mut sender,
            sender_plan.stamp,
            transfer_id,
            sender_paired.mailbox_secret().expose(),
            committed_send,
        )
        .await,
        PollResult::Unopenable
    );
    assert_eq!(sender.record().state, ProductState::Unconfirmed);

    let wrong_hash = ReceiptPayload::new(ContentHash::from_bytes([0x5A; 32]), total);
    let wrong_hash = seal_receipt(
        transfer_id,
        receiver_paired.mailbox_secret().expose(),
        &wrong_hash,
    )
    .unwrap();
    mailbox.post(slot, &wrong_hash).await.unwrap();
    assert_eq!(
        poll_sender(
            &mailbox,
            &mut sender,
            sender_plan.stamp,
            transfer_id,
            sender_paired.mailbox_secret().expose(),
            committed_send,
        )
        .await,
        PollResult::Mismatch
    );
    assert_eq!(sender.record().state, ProductState::Unconfirmed);
    assert!(sender.record().facts.receipt_mismatch);

    let (_, different_peer) = complete_pair("912345-cobalt-comet", 0x30, 0x90);
    let wrong_secret = seal_receipt(
        transfer_id,
        different_peer.mailbox_secret().expose(),
        &receipt,
    )
    .unwrap();
    mailbox.post(slot, &wrong_secret).await.unwrap();
    assert_eq!(
        poll_sender(
            &mailbox,
            &mut sender,
            sender_plan.stamp,
            transfer_id,
            sender_paired.mailbox_secret().expose(),
            committed_send,
        )
        .await,
        PollResult::Unopenable
    );
    assert_eq!(sender.record().state, ProductState::Unconfirmed);

    mailbox.post(slot, &correct).await.unwrap();
    let mut ledger = DutyLedger::new();
    assert_eq!(
        ledger.advance_generation(duty.provenance.card, duty.provenance.generation),
        GenerationUpdate::Initialized
    );
    assert_eq!(ledger.register(duty), Registration::Registered);
    let admitted = match ledger.admit(DutyResult {
        provenance: duty.provenance,
        report: DutyReport::Outcome(OutcomeCode::Completed),
    }) {
        Admission::Fresh(admitted) => admitted,
        admission => panic!("posted receipt result was not admitted: {admission:?}"),
    };
    receiver
        .apply(ProductInput::ReceiptPosted(admitted))
        .unwrap();
    assert!(receiver.record().facts.proof_delivered);

    let stored = mailbox.poll(slot).await.unwrap().unwrap();
    assert_eq!(stored, correct);
    assert!(
        !stored
            .as_bytes()
            .windows(complete.file_hash.as_bytes().len())
            .any(|window| window == complete.file_hash.as_bytes())
    );
    drop(receiver_paired);
    drop(receiver);

    assert_eq!(
        poll_sender(
            &mailbox,
            &mut sender,
            sender_plan.stamp,
            transfer_id,
            sender_paired.mailbox_secret().expose(),
            committed_send,
        )
        .await,
        PollResult::Verified
    );
    assert_eq!(sender.record().state, ProductState::Completed);
    server.shutdown().await.unwrap();
}
