use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptSupervisor, CommitPointResult,
    EventAdmission, OpenResult, ResumeIntent, RetirementAckResult, RetirementIntent,
    RetirementRequestResult,
};
use envoix_capabilities::{
    Admission, Duty, DutyKind, DutyLedger, DutyReport, DutyResult, GenerationUpdate, Registration,
    SourceAcquisitionKey, SourceReport, SourceRetention, SourceSeekability,
};
use envoix_invite::RoomCode;
use envoix_operation_store::{
    ArtifactKey, DestructiveOperation, OperationStore, OutboxStatus, PossessionState, StoreError,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_pairing::{
    DescriptorPayload, EntropyError, EntropySource, MAX_MESSAGE_BODY, PairingCode, PairingError,
    WIRE_HEADER_LEN, initiator_start, responder_respond,
};
use envoix_product::{
    AcceptedSourceOffer, ApplyOutcome, CommitError, CommitStatus, CommittedSession, ContentHash,
    IdentityError, IdentitySource, NewTransfer, ProductCommand, ProductEffect, ProductInput,
    ProductState, Quiescence, RecordDecode, RecordStore, SourcePossession, StagedContent,
    StorageAction, TransferContent, TransferRecord, decode_record,
};
use envoix_rendezvous::{ClientConfig, ControlLimits, Role};
use envoix_rendezvous_iroh::{
    BrokerSession, EndpointConfig, IrohClientConfig, bind_endpoint, join_room,
};
use envoix_server::{ServerConfig, run};
use envoix_storage_api::{Durability, EnvelopeKey, LoadOutcome, Storage};
use envoix_storage_local::LocalStorage;
use envoix_types::{
    ArtifactId, ByteCount, Direction, LandedName, OfferedName, RecordId, TransferId,
};
use iroh::{EndpointAddr, SecretKey};
use tempfile::TempDir;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_BYTES: u64 = 8 * 1024;
const PARTIAL_BYTES: u64 = 3 * 1024;

type AuthenticatedOffer = (TransferId, ArtifactId, OfferedName, ByteCount);

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

impl IdentitySource for FixedBytes {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        self.write(destination);
        Ok(())
    }
}

impl EntropySource for FixedBytes {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        self.write(destination);
        Ok(())
    }
}

/// The L7 adapter binds an L3 record commit to the same card's durable P4 store.
///
/// Creation starts without a card, so the first encoded record supplies the
/// locally minted `identity.card`; every later write must carry that same card.
struct ProductOperationStore {
    root: PathBuf,
    operation: Option<OperationStore<LocalStorage>>,
    accepted_writes: usize,
    committed_writes: usize,
    reject_writes: bool,
}

impl ProductOperationStore {
    fn deferred(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            operation: None,
            accepted_writes: 0,
            committed_writes: 0,
            reject_writes: false,
        }
    }

    fn open(root: impl Into<PathBuf>, card: RecordId) -> Self {
        let root = root.into();
        let operation = OperationStore::open(LocalStorage::open(&root).unwrap(), card).unwrap();
        Self {
            root,
            operation: Some(operation),
            accepted_writes: 0,
            committed_writes: 0,
            reject_writes: false,
        }
    }

    fn operation(&self) -> &OperationStore<LocalStorage> {
        self.operation
            .as_ref()
            .expect("the first record commit opens the card store")
    }

    fn operation_mut(&mut self) -> &mut OperationStore<LocalStorage> {
        self.operation
            .as_mut()
            .expect("the first record commit opens the card store")
    }

    fn reject_all_writes(&mut self) {
        self.reject_writes = true;
    }
}

impl RecordStore for ProductOperationStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        self.accepted_writes += 1;
        if self.reject_writes {
            return Err(CommitError);
        }

        let record = decode_loaded_record(encoded).map_err(|_| CommitError)?;
        let card = record.identity.card;
        if self.operation.is_none() {
            self.operation = Some(
                OperationStore::open(
                    LocalStorage::open(&self.root).map_err(|_| CommitError)?,
                    card,
                )
                .map_err(|_| CommitError)?,
            );
        }
        let operation = self.operation.as_mut().ok_or(CommitError)?;
        if operation.record_id() != card {
            return Err(CommitError);
        }
        operation
            .commit_record(encoded, Durability::Durable)
            .map_err(|_| CommitError)?;
        self.committed_writes += 1;
        Ok(())
    }
}

fn decode_loaded_record(encoded: &[u8]) -> Result<TransferRecord, ()> {
    match decode_record(encoded).map_err(|_| ())? {
        RecordDecode::Loaded(record) => Ok(*record),
        RecordDecode::UnsupportedFuture { .. } => Err(()),
    }
}

fn reopen_session(root: &Path, card: RecordId) -> CommittedSession<ProductOperationStore> {
    let store = ProductOperationStore::open(root, card);
    let encoded = store
        .operation()
        .latest_record()
        .expect("a committed product record")
        .to_vec();
    let record = decode_loaded_record(&encoded).expect("the latest product record decodes");
    CommittedSession::from_record(record, store, NonZeroUsize::MIN)
}

fn rendezvous_endpoint_config() -> EndpointConfig {
    EndpointConfig::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        None,
        SecretKey::generate(),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn rendezvous_client_config() -> IrohClientConfig {
    IrohClientConfig::new(
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
        ClientConfig::new(Duration::from_secs(5), ControlLimits::new(64).unwrap()).unwrap(),
    )
    .unwrap()
}

async fn send_pairing_frame(session: &mut BrokerSession, frame: &[u8]) {
    session
        .streams_mut()
        .0
        .write_all(frame)
        .await
        .expect("send pairing frame");
}

async fn receive_pairing_frame(session: &mut BrokerSession) -> Vec<u8> {
    let mut header = [0; WIRE_HEADER_LEN];
    session
        .streams_mut()
        .1
        .read_exact(&mut header)
        .await
        .expect("read pairing header");
    let body_len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
    assert!(body_len <= MAX_MESSAGE_BODY);
    let mut frame = Vec::with_capacity(WIRE_HEADER_LEN + body_len);
    frame.extend_from_slice(&header);
    frame.resize(WIRE_HEADER_LEN + body_len, 0);
    session
        .streams_mut()
        .1
        .read_exact(&mut frame[WIRE_HEADER_LEN..])
        .await
        .expect("read pairing body");
    frame
}

async fn joined_sessions(
    broker: &EndpointAddr,
    room_code: &str,
) -> (iroh::Endpoint, iroh::Endpoint, BrokerSession, BrokerSession) {
    let sender_endpoint = bind_endpoint(rendezvous_endpoint_config())
        .await
        .expect("bind sender endpoint");
    let receiver_endpoint = bind_endpoint(rendezvous_endpoint_config())
        .await
        .expect("bind receiver endpoint");
    let room_key = RoomCode::parse(room_code)
        .expect("valid room code")
        .namespaced_key();
    let sender_join = join_room(
        &sender_endpoint,
        broker.clone(),
        room_key.clone(),
        rendezvous_client_config(),
    );
    let receiver_join = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        join_room(
            &receiver_endpoint,
            broker.clone(),
            room_key,
            rendezvous_client_config(),
        )
        .await
    };
    let (sender_session, receiver_session) = timeout(TEST_TIMEOUT, async {
        tokio::join!(sender_join, receiver_join)
    })
    .await
    .expect("rendezvous join deadline");
    let sender_session = sender_session.expect("sender joins");
    let receiver_session = receiver_session.expect("receiver joins");
    assert_eq!(sender_session.role(), Role::Initiator);
    assert_eq!(receiver_session.role(), Role::Responder);
    (
        sender_endpoint,
        receiver_endpoint,
        sender_session,
        receiver_session,
    )
}

async fn handoff_offer_through_real_pairing(
    broker: &EndpointAddr,
    room_code: &str,
    sender_descriptor: &DescriptorPayload,
) -> AuthenticatedOffer {
    let (sender_endpoint, receiver_endpoint, mut sender_session, mut receiver_session) =
        joined_sessions(broker, room_code).await;
    let pairing_code = PairingCode::new(room_code.as_bytes().to_vec()).unwrap();
    let (sender_waiting, start) =
        initiator_start(&pairing_code, &mut FixedBytes::new(0x20)).unwrap();
    send_pairing_frame(&mut sender_session, &start).await;
    let start = receive_pairing_frame(&mut receiver_session).await;
    let (receiver_waiting, response) =
        responder_respond(&pairing_code, &start, &mut FixedBytes::new(0x60)).unwrap();
    send_pairing_frame(&mut receiver_session, &response).await;
    let response = receive_pairing_frame(&mut sender_session).await;
    let (sender_confirming, sender_confirmation) =
        sender_waiting.receive_response(&response).unwrap();
    send_pairing_frame(&mut sender_session, &sender_confirmation).await;
    let sender_confirmation = receive_pairing_frame(&mut receiver_session).await;
    let (mut receiver_paired, receiver_confirmation) = receiver_waiting
        .verify_initiator(&sender_confirmation)
        .unwrap();
    send_pairing_frame(&mut receiver_session, &receiver_confirmation).await;
    let receiver_confirmation = receive_pairing_frame(&mut sender_session).await;
    let mut sender_paired = sender_confirming
        .verify_responder(&receiver_confirmation)
        .unwrap();
    assert_eq!(sender_paired.data_token(), receiver_paired.data_token());

    let sealed_sender = sender_paired.seal_descriptor(sender_descriptor).unwrap();
    let mut tampered = sealed_sender.clone();
    *tampered.last_mut().expect("sealed descriptor has a tag") ^= 0x01;
    assert!(matches!(
        receiver_paired.open_peer_descriptor(&tampered),
        Err(PairingError::AuthenticationFailed)
    ));

    let receiver_descriptor = DescriptorPayload::new(b"receiver-ready".to_vec()).unwrap();
    let sealed_receiver = receiver_paired
        .seal_descriptor(&receiver_descriptor)
        .unwrap();
    send_pairing_frame(&mut sender_session, &sealed_sender).await;
    send_pairing_frame(&mut receiver_session, &sealed_receiver).await;
    let at_sender = receive_pairing_frame(&mut sender_session).await;
    let at_receiver = receive_pairing_frame(&mut receiver_session).await;
    let opened_by_sender = sender_paired.open_peer_descriptor(&at_sender).unwrap();
    let opened_by_receiver = receiver_paired.open_peer_descriptor(&at_receiver).unwrap();
    assert_eq!(
        opened_by_sender.payload().as_bytes(),
        receiver_descriptor.as_bytes()
    );
    assert_eq!(opened_by_receiver.data_token(), sender_paired.data_token());

    let offer = serde_json::from_slice(opened_by_receiver.payload().as_bytes()).unwrap();
    let (sender_closed, receiver_closed) =
        tokio::join!(sender_session.close(), receiver_session.close());
    sender_closed.expect("close sender pairing session");
    receiver_closed.expect("close receiver pairing session");
    sender_endpoint.close().await;
    receiver_endpoint.close().await;
    offer
}

fn server_config(root: &Path) -> ServerConfig {
    let mut config = ServerConfig::operational_defaults();
    config.bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.mailbox_bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    config.node_key_path = root.join("rendezvous-node.key");
    config.room_ttl = Duration::from_secs(10);
    config.relay_ttl = Duration::from_secs(10);
    config.join_deadline = Duration::from_secs(5);
    config.close_grace = Duration::from_secs(5);
    config.handshake_deadline = Duration::from_secs(5);
    config.bind_deadline = Duration::from_secs(5);
    config.max_waiting_rooms = 8;
    config.max_connections = 8;
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

/// Walks a sending session from "needs a document" to a live attempt, and
/// returns the outcome that LAUNCHED it. A sender has no shorter route: the
/// lifecycle is the only source authority, so nothing can declare it ready.
fn stage_the_source<S: RecordStore>(
    session: &mut CommittedSession<S>,
    _created: &ApplyOutcome,
    offered_name: &OfferedName,
    total: ByteCount,
) -> ApplyOutcome {
    // The acquisition a frontend answers is the one the card PUBLISHES, which
    // read/9 carries as the `pick_source` action. This is the same derivation
    // the projection uses and the authority checks an offer against.
    let provenance = session.record().current_acquisition().provenance();
    session
        .apply(ProductInput::SourceOffered {
            offer: AcceptedSourceOffer::of_one_document(
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
            possession: SourcePossession::Streamed,
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
        .expect("a committed attempt start")
}

fn admit_attempt(
    supervisor: &AttemptSupervisor,
    plan: AttemptPlan,
    kind: AttemptEventKind,
) -> ProductInput {
    match supervisor.observe(AttemptEvent {
        stamp: plan.stamp,
        kind,
    }) {
        EventAdmission::Accepted(event) => ProductInput::AttemptObserved(event),
        other => panic!("current attempt event was not admitted: {other:?}"),
    }
}

fn receipt_duty(outcome: &ApplyOutcome) -> Duty {
    outcome
        .released_after_commit
        .iter()
        .find_map(|effect| match effect {
            ProductEffect::CapabilityDuty { duty, .. } => Some(*duty),
            _ => None,
        })
        .expect("a committed receipt duty")
}

fn has_publication_duty(outcome: &ApplyOutcome) -> bool {
    outcome
        .released_after_commit
        .iter()
        .chain(&outcome.released_immediately)
        .any(|effect| {
            matches!(
                effect,
                ProductEffect::CapabilityDuty { duty, .. }
                    if duty.kind == DutyKind::Publication
            )
        })
}

#[derive(Default)]
struct FakeCourier {
    posts: usize,
}

impl FakeCourier {
    fn post(&mut self, duty: Duty) -> DutyResult {
        assert_eq!(duty.kind, DutyKind::Courier);
        self.posts += 1;
        DutyResult {
            provenance: duty.provenance,
            report: DutyReport::Outcome(OutcomeCode::Completed),
        }
    }
}

// Scope: this slice is RECEIVER-centric — it drives the receive card's full
// product+op-store lifecycle (adopt → transfer → crash/resume → complete →
// receive-side Courier receipt → publish → tombstone/remove). The sender-side
// confirm→ReceiptVerified path is NOT exercised here (it depends on the
// deferred M4 `Phase(Confirming)` emission — a named acceptance property owned
// by M4/executor, tested once that lands).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn product_crash_receipt_publish_tombstone_slice() {
    let directory = TempDir::new().unwrap();
    let durable_root = directory.path().join("durable");
    let server = run(server_config(directory.path())).await.unwrap();
    let broker = server.endpoint_addr().clone();
    let offered_name = OfferedName::from_untrusted("p5-walking-slice.bin").unwrap();
    let total = ByteCount::new(TOTAL_BYTES);

    // Create the sender first: it alone mints the shared transfer/artifact pair.
    let (mut sender, sender_created) = CommittedSession::create(
        new_transfer(Direction::Send),
        &mut FixedBytes::new(0x10),
        ProductOperationStore::deferred(&durable_root),
        NonZeroUsize::MIN,
    )
    .unwrap();
    assert_eq!(
        sender_created.commit,
        CommitStatus::Committed { attempts: 1 }
    );
    // Then give it a document: the lifecycle is the only source authority, so
    // this is the only way a sender reaches the wire.
    let sender_created = stage_the_source(&mut sender, &sender_created, &offered_name, total);
    let sender_plan = start_plan(&sender_created);
    assert_eq!(
        sender.store().operation().record_id(),
        sender.record().identity.card
    );

    let sender_offer = (
        sender.record().identity.transfer,
        sender.record().identity.artifact,
        offered_name.clone(),
        sender.record().total(),
    );
    let descriptor = DescriptorPayload::new(serde_json::to_vec(&sender_offer).unwrap()).unwrap();
    let authenticated_offer =
        handoff_offer_through_real_pairing(&broker, "520001-amber-anchor", &descriptor).await;
    server.shutdown().await.unwrap();
    assert_eq!(authenticated_offer, sender_offer);

    // Adoption occurs only after the real C3 descriptor opened successfully.
    // The name and total the peer authenticated belong in the receiver's
    // `NotRequired { peer_content }`, and nothing admits a peer header into it
    // yet — that producer arrives with the receive-side header work. They are
    // asserted equal to the sender's above, which is what this slice can prove
    // today; binding them to the record is not silently skipped, it is unbuilt.
    let (transfer_id, artifact_id, _received_name, _received_total) = authenticated_offer;
    let (mut receiver, receiver_created) = CommittedSession::create_with_identity(
        new_transfer(Direction::Receive),
        transfer_id,
        artifact_id,
        &mut FixedBytes::new(0x90),
        ProductOperationStore::deferred(&durable_root),
        NonZeroUsize::MIN,
    )
    .unwrap();
    assert_eq!(
        receiver_created.commit,
        CommitStatus::Committed { attempts: 1 }
    );
    let initial_plan = start_plan(&receiver_created);
    assert_eq!(initial_plan.resume, ResumeIntent::Fresh);
    assert_eq!(initial_plan.transfer, sender_plan.transfer);
    assert_eq!(initial_plan.artifact, sender_plan.artifact);
    assert_ne!(initial_plan.stamp.card, sender_plan.stamp.card);
    assert_eq!(receiver.record().identity.transfer, transfer_id);
    assert_eq!(receiver.record().identity.artifact, artifact_id);
    assert_eq!(receiver.store().accepted_writes, 1);
    assert_eq!(receiver.store().committed_writes, 1);
    assert_eq!(
        receiver.store().operation().record_id(),
        receiver.record().identity.card
    );

    let receiver_card = receiver.record().identity.card;
    let artifact_key = ArtifactKey {
        transfer: transfer_id,
        artifact: artifact_id,
    };
    let mut first_attempt = AttemptSupervisor::new();
    assert_eq!(first_attempt.open(initial_plan), OpenResult::Opened);
    let phase = receiver
        .apply(admit_attempt(
            &first_attempt,
            initial_plan,
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    assert!(phase.commit.authorizing_commit_succeeded());
    let progress = receiver
        .apply(admit_attempt(
            &first_attempt,
            initial_plan,
            AttemptEventKind::Progress {
                transferred: ByteCount::new(PARTIAL_BYTES),
            },
        ))
        .unwrap();
    assert!(progress.commit.authorizing_commit_succeeded());
    assert_eq!(receiver.record().state, ProductState::Transferring);
    assert_eq!(receiver.record().bytes, ByteCount::new(PARTIAL_BYTES));
    let staged_prefix = vec![0x5a; PARTIAL_BYTES as usize];
    receiver
        .store_mut()
        .operation_mut()
        .stage_artifact(artifact_key, offered_name.clone(), &staged_prefix)
        .unwrap();
    let partial_record = receiver.record().clone();

    // Crash #1 drops both peer sessions, the live supervisor, and their stores.
    // The durable receiver record and staged bytes are recovered byte-for-byte.
    drop(first_attempt);
    drop(sender);
    drop(receiver);
    let mut receiver = reopen_session(&durable_root, receiver_card);
    assert_eq!(receiver.record(), &partial_record);
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .storage_mut()
            .get(EnvelopeKey::Artifact {
                record_id: receiver_card,
                artifact_id,
            })
            .unwrap(),
        LoadOutcome::Loaded(
            envoix_storage_api::OperationEnvelope::new(staged_prefix.clone()).unwrap()
        )
    );

    let restored = receiver.apply(ProductInput::Restore).unwrap();
    assert!(restored.released_after_commit.is_empty());
    assert_eq!(
        receiver.record().state,
        ProductState::Paused(envoix_product::PauseOrigin::Lost)
    );
    assert_eq!(receiver.record().quiescence, Quiescence::Quiescent);
    assert_eq!(receiver.record().bytes, ByteCount::new(PARTIAL_BYTES));

    // A reduction accepted by L3 is not a committed decision. Reject both the
    // authorizing write and best-effort escalation: no StartAttempt may escape.
    receiver.store_mut().reject_all_writes();
    let rejected_resume = receiver
        .apply(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert!(matches!(
        rejected_resume.commit,
        CommitStatus::Escalated {
            failed_state_persisted: false,
            ..
        }
    ));
    assert!(rejected_resume.released_after_commit.is_empty());
    assert_eq!(receiver.store().accepted_writes, 3);
    assert_eq!(receiver.store().committed_writes, 1);

    // Crash #2 discards the accepted-but-uncommitted reduction. Reopening yields
    // the last committed paused card, then a fresh resume is durably authorized.
    drop(receiver);
    let mut receiver = reopen_session(&durable_root, receiver_card);
    assert_eq!(
        receiver.record().state,
        ProductState::Paused(envoix_product::PauseOrigin::Lost)
    );
    assert_eq!(receiver.record().quiescence, Quiescence::Quiescent);
    let resumed = receiver
        .apply(ProductInput::Command(ProductCommand::Resume))
        .unwrap();
    assert_eq!(resumed.commit, CommitStatus::Committed { attempts: 1 });
    let resume_plan = start_plan(&resumed);
    assert_eq!(
        resume_plan.resume,
        ResumeIntent::ResumeFrom {
            offset: ByteCount::new(PARTIAL_BYTES)
        }
    );
    assert!(resume_plan.stamp.generation > initial_plan.stamp.generation);
    assert_eq!(resume_plan.transfer, transfer_id);
    assert_eq!(resume_plan.artifact, artifact_id);

    let mut resumed_attempt = AttemptSupervisor::new();
    assert_eq!(resumed_attempt.open(resume_plan), OpenResult::Opened);
    receiver
        .apply(admit_attempt(
            &resumed_attempt,
            resume_plan,
            AttemptEventKind::Phase(Phase::Transferring),
        ))
        .unwrap();
    receiver
        .apply(admit_attempt(
            &resumed_attempt,
            resume_plan,
            AttemptEventKind::Progress { transferred: total },
        ))
        .unwrap();
    assert_eq!(
        resumed_attempt.cross_commit_point(resume_plan.stamp),
        CommitPointResult::Crossed
    );
    let completed = receiver
        .apply(admit_attempt(
            &resumed_attempt,
            resume_plan,
            AttemptEventKind::Terminal(OutcomeCode::Completed),
        ))
        .unwrap();
    assert_eq!(receiver.record().state, ProductState::Completed);
    assert!(receiver.record().quiescence.is_retiring());
    assert!(completed.released_after_commit.is_empty());
    assert!(!has_publication_duty(&completed));
    receiver
        .store_mut()
        .operation_mut()
        .record_completion(artifact_key, None)
        .unwrap();
    assert!(matches!(
        receiver
            .store()
            .operation()
            .possession(artifact_key)
            .unwrap()
            .state(),
        PossessionState::Complete { landed_name: None }
    ));

    assert_eq!(
        resumed_attempt.request_retirement(resume_plan.stamp, RetirementIntent::Finalize),
        RetirementRequestResult::Requested
    );
    let RetirementAckResult::Acknowledged(ack) =
        resumed_attempt.acknowledge_retirement(resume_plan.stamp)
    else {
        panic!("the completed attempt must retire");
    };
    let retired = receiver.apply(ProductInput::AttemptRetired(ack)).unwrap();
    assert_eq!(receiver.record().quiescence, Quiescence::Quiescent);
    assert_eq!(retired.commit, CommitStatus::Committed { attempts: 1 });
    let duty = receipt_duty(&retired);
    assert_eq!(duty.kind, DutyKind::Courier);
    assert!(!has_publication_duty(&retired));
    let operation = receiver.store_mut().operation_mut();
    assert_eq!(
        operation
            .advance_generation(duty.provenance.generation)
            .unwrap(),
        GenerationUpdate::Initialized
    );
    assert_eq!(
        operation.register_duty(duty).unwrap(),
        Registration::Registered
    );

    // Crash #3 lands after durable duty registration but before capability
    // admission. The store-owned DutyLedger reconstructs one outstanding duty.
    drop(resumed_attempt);
    drop(receiver);
    let mut receiver = reopen_session(&durable_root, receiver_card);
    assert_eq!(
        receiver.store().operation().outstanding_duties(),
        vec![duty]
    );
    let replayed = receiver.apply(ProductInput::Restore).unwrap();
    assert_eq!(receipt_duty(&replayed), duty);
    assert!(!has_publication_duty(&replayed));

    let mut courier = FakeCourier::default();
    let posted = courier.post(duty);
    let admitted = match receiver
        .store_mut()
        .operation_mut()
        .admit_duty(posted)
        .unwrap()
    {
        Admission::Fresh(admitted) => admitted,
        other => panic!("the exact receipt result was not admitted: {other:?}"),
    };
    assert_eq!(admitted.duty(), duty);
    let proof = receiver
        .apply(ProductInput::ReceiptPosted(admitted))
        .unwrap();
    assert!(proof.commit.authorizing_commit_succeeded());
    assert!(receiver.record().facts.proof_delivered);
    receiver
        .store_mut()
        .operation_mut()
        .record_receipt(artifact_key)
        .unwrap();
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .admit_duty(posted)
            .unwrap(),
        Admission::Duplicate
    );
    assert_eq!(courier.posts, 1);

    // Crash #4 proves both the product proof and P4 receipt survive. Restore
    // emits no second receipt, and publication remains an L7 possession action.
    drop(receiver);
    let mut receiver = reopen_session(&durable_root, receiver_card);
    assert!(receiver.record().facts.proof_delivered);
    assert!(receiver.store().operation().outstanding_duties().is_empty());
    let fact = receiver
        .store()
        .operation()
        .possession(artifact_key)
        .unwrap();
    assert!(fact.completion_proven());
    assert!(fact.receipt_proven());
    let settled_restore = receiver.apply(ProductInput::Restore).unwrap();
    assert!(settled_restore.released_after_commit.is_empty());
    assert!(!has_publication_duty(&settled_restore));
    assert!(matches!(
        receiver
            .store_mut()
            .operation_mut()
            .queue_artifact_gc(artifact_key),
        Err(StoreError::TombstoneRequired)
    ));

    // The reducer deliberately emits no Publication duty. L7 records the landed
    // publication directly from the completion/receipt possession facts.
    let landed_name = LandedName::new("published-p5-walking-slice.bin");
    receiver
        .store_mut()
        .operation_mut()
        .record_completion(artifact_key, Some(landed_name.clone()))
        .unwrap();
    let published = receiver
        .store()
        .operation()
        .possession(artifact_key)
        .unwrap();
    assert!(published.receipt_proven());
    assert!(matches!(
        published.state(),
        PossessionState::Complete {
            landed_name: Some(name)
        } if name == &landed_name
    ));
    let manifest = receiver
        .store()
        .operation()
        .storage()
        .manifest(receiver_card)
        .unwrap()
        .unwrap();
    assert_eq!(manifest.artifacts().len(), 1);

    let removed = receiver
        .apply(ProductInput::Command(ProductCommand::Remove))
        .unwrap();
    assert!(receiver.record().facts.remove_requested);
    assert!(!receiver.store().operation().is_tombstoned());
    assert!(matches!(
        removed.released_after_commit.as_slice(),
        [ProductEffect::StorageIntent {
            identity,
            action: StorageAction::TombstoneCard,
        }] if *identity == receiver.record().identity
    ));
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .commit_tombstone()
            .unwrap(),
        OutboxStatus::Recorded
    );
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .commit_tombstone()
            .unwrap(),
        OutboxStatus::AlreadyPending
    );
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .queue_artifact_gc(artifact_key)
            .unwrap(),
        OutboxStatus::Recorded
    );

    // Crash #5: restore replays the tombstone intent at least once, while P4
    // retains exactly one tombstone and one post-tombstone collection entry.
    drop(receiver);
    let mut receiver = reopen_session(&durable_root, receiver_card);
    assert!(receiver.store().operation().is_tombstoned());
    // The published landed name (and receipt proof) is durable across the crash.
    let republished = receiver
        .store()
        .operation()
        .possession(artifact_key)
        .unwrap();
    assert!(republished.receipt_proven());
    assert!(matches!(
        republished.state(),
        PossessionState::Complete {
            landed_name: Some(name)
        } if name == &landed_name
    ));
    let tombstone = DestructiveOperation::TombstoneCard {
        card: receiver_card,
    };
    let collect = DestructiveOperation::CollectArtifact {
        card: receiver_card,
        key: artifact_key,
    };
    let replayable = receiver.store().operation().replayable_outbox();
    assert_eq!(
        replayable
            .iter()
            .filter(|entry| **entry == tombstone)
            .count(),
        1
    );
    assert_eq!(
        replayable.iter().filter(|entry| **entry == collect).count(),
        1
    );
    let replayed_remove = receiver.apply(ProductInput::Restore).unwrap();
    assert!(matches!(
        replayed_remove.released_after_commit.as_slice(),
        [ProductEffect::StorageIntent {
            action: StorageAction::TombstoneCard,
            ..
        }]
    ));
    assert_eq!(
        receiver
            .store_mut()
            .operation_mut()
            .commit_tombstone()
            .unwrap(),
        OutboxStatus::AlreadyPending
    );
    assert_eq!(
        receiver
            .store()
            .operation()
            .replayable_outbox()
            .iter()
            .filter(|entry| **entry == tombstone)
            .count(),
        1
    );

    // Last-good-copy boundary (composed op-store): the main flow proves the tombstone
    // path with an already-safe (receipt + landed) artifact, so it never presents the
    // rejection. Prove it directly over the durable store: even under a committed
    // tombstone, GC of an artifact whose only durable copy is a completed-but-unproven
    // local one is REFUSED (it would lose the last good copy) until a receipt exists.
    {
        let root = tempfile::tempdir().unwrap();
        let card = RecordId::new(0xA5A5);
        let key = ArtifactKey {
            transfer: TransferId::from_bytes([0x51; 16]),
            artifact: ArtifactId::from_bytes([0x52; 16]),
        };
        let mut store =
            OperationStore::open(LocalStorage::open(root.path()).unwrap(), card).unwrap();
        store
            .stage_artifact(
                key,
                OfferedName::from_untrusted("last-copy.bin").unwrap(),
                b"complete",
            )
            .unwrap();
        store.record_completion(key, None).unwrap();
        store.commit_tombstone().unwrap();
        assert!(matches!(
            store.queue_artifact_gc(key),
            Err(StoreError::WouldLoseLastGoodCopy)
        ));
        store.record_receipt(key).unwrap();
        assert_eq!(
            store.queue_artifact_gc(key).unwrap(),
            OutboxStatus::Recorded
        );
    }
}
