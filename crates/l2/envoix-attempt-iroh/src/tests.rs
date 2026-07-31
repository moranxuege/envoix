use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, PeerContentVerdict, ResumeIntent, RetirementIntent,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_pairing::{
    DataPlaneToken, EntropyError, EntropySource, PairingCode, SystemEntropy, initiator_start,
    responder_respond,
};
use envoix_protocol::{ContentHash, FrameKind};
use envoix_session_iroh::{
    AuthFailureBudget, BindAddresses, CloseOrdering, CongestionControl, ExportedSecret, FlowWindow,
    IrohListener, PathObservation, SessionCancellation, SessionEndpointConfig, SessionError,
    SessionLink, SessionTimeouts, SessionTransportConfig, dial,
};
use envoix_transfer::{DurablePrefix, SourceReader, StagingSink, StorageFault, StorageOperation};
use envoix_types::{
    ArtifactId, AttemptGen, ByteCount, Direction, OfferedName, RecordId, TransferId,
};
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::time::timeout;

use crate::{
    AttemptTimeouts, AttemptTransferSpec, SharedAttemptSupervisor, spawn_iroh_receiver,
    spawn_receiver, spawn_sender,
};

struct TestEntropy {
    next: u8,
}

impl TestEntropy {
    const fn new(seed: u8) -> Self {
        Self { next: seed }
    }
}

impl EntropySource for TestEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        for byte in destination {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct MemorySource {
    bytes: Arc<Vec<u8>>,
}

impl SourceReader for MemorySource {
    fn read_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        let offset = offset.get() as usize;
        if offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = destination.len().min(self.bytes.len() - offset);
        destination[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        Ok(count)
    }
}

/// One artifact's staging bytes. Shared by `Arc` across generations on purpose:
/// a receive partial is keyed by the TRANSFER, so it must survive a generation
/// bump — which is the whole point of the resume this file exercises.
#[derive(Default)]
struct SinkState {
    staged: Vec<u8>,
    prefix: Option<DurablePrefix>,
    sealed: Option<Vec<u8>>,
}

#[derive(Clone, Default)]
struct MemorySink {
    state: Arc<Mutex<SinkState>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl MemorySink {
    fn resume_offset(&self) -> ByteCount {
        self.state
            .lock()
            .unwrap()
            .prefix
            .expect("cancelled receiver checkpoints a durable prefix")
            .length
    }

    fn sealed(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap()
            .sealed
            .clone()
            .expect("completed receiver seals bytes")
    }
}

impl StagingSink for MemorySink {
    type Seal = ();

    fn resume(&mut self) -> Result<DurablePrefix, StorageFault> {
        let mut state = self.state.lock().unwrap();
        let prefix = state.prefix.unwrap_or(DurablePrefix {
            length: ByteCount::new(0),
            digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
        });
        let length = prefix.length.get() as usize;
        state.staged.truncate(length);
        Ok(prefix)
    }

    fn read_partial_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        let state = self.state.lock().unwrap();
        let offset = offset.get() as usize;
        if offset >= state.staged.len() {
            return Ok(0);
        }
        let count = destination.len().min(state.staged.len() - offset);
        destination[..count].copy_from_slice(&state.staged[offset..offset + count]);
        Ok(count)
    }

    fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), StorageFault> {
        let mut state = self.state.lock().unwrap();
        if state.staged.len() != offset.get() as usize {
            return Err(StorageFault::new(StorageOperation::AppendStaging));
        }
        state.staged.extend_from_slice(bytes);
        Ok(())
    }

    fn checkpoint(&mut self, prefix: DurablePrefix) -> Result<(), StorageFault> {
        let mut state = self.state.lock().unwrap();
        assert!(
            prefix.length.get() as usize <= state.staged.len(),
            "a sink cannot promise bytes it was never given"
        );
        state.prefix = Some(prefix);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StorageFault> {
        let mut state = self.state.lock().unwrap();
        state.prefix = Some(DurablePrefix {
            length: ByteCount::new(0),
            digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
        });
        state.staged.clear();
        Ok(())
    }

    fn seal(
        &mut self,
        expected_size: ByteCount,
        digest: ContentHash,
    ) -> Result<Self::Seal, StorageFault> {
        let mut state = self.state.lock().unwrap();
        if state.staged.len() as u64 != expected_size.get()
            || ContentHash::from_bytes(*blake3::hash(&state.staged).as_bytes()) != digest
        {
            return Err(StorageFault::new(StorageOperation::Seal));
        }
        self.order.lock().unwrap().push("receiver_seal");
        state.sealed = Some(state.staged.clone());
        state.prefix = None;
        Ok(())
    }
}

struct MemoryLink {
    sender: mpsc::UnboundedSender<Vec<u8>>,
    receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    paths: Option<mpsc::UnboundedReceiver<PathObservation>>,
    released: Arc<AtomicBool>,
    my_closed: Arc<Notify>,
    peer_closed: Arc<Notify>,
    chunk_gate: Option<Arc<Semaphore>>,
    chunk_count: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl SessionLink for MemoryLink {
    async fn send_packet(&mut self, packet: &[u8]) -> Result<(), SessionError> {
        if packet.get(6) == Some(&FrameKind::Chunk.wire_id()) {
            self.chunk_count.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.chunk_gate {
                let permit = gate.acquire().await.map_err(|_| SessionError::PeerClosed)?;
                permit.forget();
            }
        }
        if packet.get(6) == Some(&FrameKind::CompleteAck.wire_id()) {
            self.order.lock().unwrap().push("ack_sent");
        }
        tokio::task::yield_now().await;
        self.sender
            .send(packet.to_vec())
            .map_err(|_| SessionError::PeerClosed)
    }

    async fn receive_packet(&mut self, _maximum_payload: usize) -> Result<Vec<u8>, SessionError> {
        self.receiver.recv().await.ok_or(SessionError::PeerClosed)
    }

    fn export_keying_material(
        &self,
        _label: &[u8],
        _context: &[u8],
    ) -> Result<ExportedSecret, SessionError> {
        Ok(ExportedSecret::new([0x5a; 32]))
    }

    fn take_path_observations(&mut self) -> mpsc::UnboundedReceiver<PathObservation> {
        self.paths.take().unwrap()
    }

    async fn close(
        &mut self,
        ordering: CloseOrdering,
        timeouts: SessionTimeouts,
    ) -> Result<(), SessionError> {
        if matches!(ordering, CloseOrdering::AwaitPeer) {
            let _ = timeout(timeouts.peer_close(), self.peer_closed.notified()).await;
        }
        self.released.store(true, Ordering::SeqCst);
        self.my_closed.notify_waiters();
        Ok(())
    }
}

struct LinkPair {
    sender: MemoryLink,
    receiver: MemoryLink,
    sender_released: Arc<AtomicBool>,
    receiver_released: Arc<AtomicBool>,
    sender_chunks: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

fn link_pair(sender_gate: Option<Arc<Semaphore>>) -> LinkPair {
    let (a_tx, a_rx) = mpsc::unbounded_channel();
    let (b_tx, b_rx) = mpsc::unbounded_channel();
    let (a_path_tx, a_path_rx) = mpsc::unbounded_channel();
    let (b_path_tx, b_path_rx) = mpsc::unbounded_channel();
    drop(a_path_tx);
    drop(b_path_tx);
    let a_released = Arc::new(AtomicBool::new(false));
    let b_released = Arc::new(AtomicBool::new(false));
    let a_closed = Arc::new(Notify::new());
    let b_closed = Arc::new(Notify::new());
    let a_chunks = Arc::new(AtomicUsize::new(0));
    let order = Arc::new(Mutex::new(Vec::new()));
    LinkPair {
        sender: MemoryLink {
            sender: a_tx,
            receiver: b_rx,
            paths: Some(a_path_rx),
            released: a_released.clone(),
            my_closed: a_closed.clone(),
            peer_closed: b_closed.clone(),
            chunk_gate: sender_gate,
            chunk_count: a_chunks.clone(),
            order: order.clone(),
        },
        receiver: MemoryLink {
            sender: b_tx,
            receiver: a_rx,
            paths: Some(b_path_rx),
            released: b_released.clone(),
            my_closed: b_closed,
            peer_closed: a_closed,
            chunk_gate: None,
            chunk_count: Arc::new(AtomicUsize::new(0)),
            order: order.clone(),
        },
        sender_released: a_released,
        receiver_released: b_released,
        sender_chunks: a_chunks,
        order,
    }
}

fn session_timeouts() -> SessionTimeouts {
    SessionTimeouts::new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .unwrap()
}

fn attempt_timeouts() -> AttemptTimeouts {
    AttemptTimeouts::new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        session_timeouts(),
    )
    .unwrap()
}

fn plan(direction: Direction, generation: u32, resume: ResumeIntent) -> AttemptPlan {
    AttemptPlan {
        stamp: AttemptStamp {
            card: RecordId::new(77),
            generation: AttemptGen::new(generation),
        },
        direction,
        transfer: TransferId::from_bytes([0x33; 16]),
        artifact: ArtifactId::from_bytes([0x44; 16]),
        resume,
    }
}

fn spec(bytes: &[u8]) -> AttemptTransferSpec {
    let mut hasher = blake3::Hasher::new();
    hasher.update(bytes);
    AttemptTransferSpec {
        offered_name: OfferedName::from_untrusted("payload.bin").unwrap(),
        file_size: ByteCount::new(bytes.len() as u64),
        chunk_size: ByteCount::new(1024),
        claimed_complete: None,
        // What staging would have established over exactly these bytes. A send
        // states what it intends to send, so a spec built for a source must say
        // what that source hashes to.
        content_hash: Some(ContentHash::from_bytes(*hasher.finalize().as_bytes())),
        timeouts: attempt_timeouts(),
    }
}

fn token_pair() -> (DataPlaneToken, DataPlaneToken) {
    token_pair_for(b"m4-in-memory-pair")
}

fn token_pair_for(code: &[u8]) -> (DataPlaneToken, DataPlaneToken) {
    let code_a = PairingCode::new(code.to_vec()).unwrap();
    let code_b = PairingCode::new(code.to_vec()).unwrap();
    let mut entropy_a = SystemEntropy;
    let mut entropy_b = SystemEntropy;
    let (initiator, start) = initiator_start(&code_a, &mut entropy_a).unwrap();
    let (responder, response) = responder_respond(&code_b, &start, &mut entropy_b).unwrap();
    let (initiator, confirmation) = initiator.receive_response(&response).unwrap();
    let (responder, response) = responder.verify_initiator(&confirmation).unwrap();
    let initiator = initiator.verify_responder(&response).unwrap();
    (initiator.into_data_token(), responder.into_data_token())
}

fn loopback_config() -> SessionEndpointConfig {
    SessionEndpointConfig {
        bind: BindAddresses::ipv4_only(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        relay: None,
        transport: SessionTransportConfig {
            flow_window: FlowWindow::default(),
            congestion: CongestionControl::Bbr3,
        },
    }
}

async fn terminal_outcome(handle: &mut crate::AttemptHandle) -> OutcomeCode {
    loop {
        let event = handle.next_event().await.expect("attempt emits a terminal");
        if let AttemptEventKind::Terminal(outcome) = event.kind {
            return outcome;
        }
    }
}

async fn next_progress(handle: &mut crate::AttemptHandle) -> AttemptEvent {
    loop {
        let event = handle.next_event().await.expect("attempt emits terminal");
        if matches!(event.kind, AttemptEventKind::Progress { .. }) {
            return event;
        }
    }
}

async fn drain_events(handle: &mut crate::AttemptHandle) {
    while handle.next_event().await.is_some() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attempt_iroh_generation_and_retirement() {
    let bytes = (0..64 * 1024)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let source = MemorySource {
        bytes: Arc::new(bytes.clone()),
    };
    let sink = MemorySink::default();
    let sender_supervisor: SharedAttemptSupervisor = Arc::new(Mutex::new(AttemptSupervisor::new()));
    let receiver_supervisor: SharedAttemptSupervisor =
        Arc::new(Mutex::new(AttemptSupervisor::new()));

    let gate = Arc::new(Semaphore::new(1));
    let first_links = link_pair(Some(gate));
    let first_sender_released = first_links.sender_released.clone();
    let first_receiver_released = first_links.receiver_released.clone();
    let (sender_token, receiver_token) = token_pair();
    let mut first_sender = spawn_sender(
        plan(Direction::Send, 1, ResumeIntent::Fresh),
        spec(&bytes),
        sender_token,
        source.clone(),
        first_links.sender,
        sender_supervisor.clone(),
        TestEntropy::new(0x10),
    )
    .unwrap();
    let mut first_receiver = spawn_receiver(
        plan(Direction::Receive, 1, ResumeIntent::Fresh),
        spec(&bytes),
        receiver_token,
        sink.clone(),
        first_links.receiver,
        receiver_supervisor.clone(),
        TestEntropy::new(0x80),
    )
    .unwrap();
    admit_peer_content(&mut first_receiver);
    assert_eq!(first_sender.open_result(), OpenResult::Opened);
    assert_eq!(first_receiver.open_result(), OpenResult::Opened);

    let stale_event = next_progress(&mut first_receiver).await;
    first_sender
        .control()
        .request(RetirementIntent::Cancel)
        .unwrap();
    first_receiver
        .control()
        .request(RetirementIntent::Cancel)
        .unwrap();
    let sender_ack = first_sender.wait_ack().await.unwrap();
    let receiver_ack = first_receiver.wait_ack().await.unwrap();
    assert_eq!(sender_ack.outcome(), OutcomeCode::Cancelled);
    assert_eq!(receiver_ack.outcome(), OutcomeCode::Cancelled);
    assert!(first_sender_released.load(Ordering::SeqCst));
    assert!(first_receiver_released.load(Ordering::SeqCst));
    drain_events(&mut first_sender).await;
    drain_events(&mut first_receiver).await;
    assert!(first_sender.next_event().await.is_none());
    assert!(first_receiver.next_event().await.is_none());

    let offset = sink.resume_offset();
    assert!(offset.get() > 0);
    assert!(offset.get() < bytes.len() as u64);

    let second_links = link_pair(None);
    let second_sender_chunks = second_links.sender_chunks.clone();
    let order = second_links.order.clone();
    let sink = MemorySink {
        state: sink.state.clone(),
        order: order.clone(),
    };
    let (sender_token, receiver_token) = token_pair();
    let resume = ResumeIntent::Allowed;
    let mut second_sender = spawn_sender(
        plan(Direction::Send, 2, resume),
        spec(&bytes),
        sender_token,
        source,
        second_links.sender,
        sender_supervisor.clone(),
        TestEntropy::new(0x20),
    )
    .unwrap();
    let mut second_receiver = spawn_receiver(
        plan(Direction::Receive, 2, resume),
        spec(&bytes),
        receiver_token,
        sink.clone(),
        second_links.receiver,
        receiver_supervisor.clone(),
        TestEntropy::new(0x90),
    )
    .unwrap();
    admit_peer_content(&mut second_receiver);
    assert_eq!(second_sender.open_result(), OpenResult::Superseded);
    assert_eq!(second_receiver.open_result(), OpenResult::Superseded);
    assert_eq!(
        receiver_supervisor.lock().unwrap().observe(stale_event),
        EventAdmission::Stale
    );

    let sender_control = second_sender.control();
    let receiver_control = second_receiver.control();
    let sender_terminal = async {
        loop {
            let event = second_sender.next_event().await.unwrap();
            if matches!(
                event.kind,
                AttemptEventKind::Terminal(OutcomeCode::Completed)
            ) {
                return event;
            }
        }
    };
    let receiver_terminal = async {
        loop {
            let event = second_receiver.next_event().await.unwrap();
            if matches!(
                event.kind,
                AttemptEventKind::Terminal(OutcomeCode::Completed)
            ) {
                return event;
            }
        }
    };
    let (sender_terminal, receiver_terminal) = tokio::join!(sender_terminal, receiver_terminal);
    assert_eq!(sender_terminal.stamp.generation, AttemptGen::new(2));
    assert_eq!(receiver_terminal.stamp.generation, AttemptGen::new(2));
    sender_control.request(RetirementIntent::Finalize).unwrap();
    receiver_control
        .request(RetirementIntent::Finalize)
        .unwrap();
    let sender_ack = second_sender.wait_ack().await.unwrap();
    let receiver_ack = second_receiver.wait_ack().await.unwrap();
    assert_eq!(sender_ack.outcome(), OutcomeCode::Completed);
    assert_eq!(receiver_ack.outcome(), OutcomeCode::Completed);
    assert_eq!(sink.sealed(), bytes);

    let expected_tail_chunks = (bytes.len() as u64 - offset.get()).div_ceil(1024) as usize;
    assert_eq!(
        second_sender_chunks.load(Ordering::SeqCst),
        expected_tail_chunks,
        "gen-2 must send only the verified tail"
    );
    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["receiver_seal", "ack_sent"],
        "durable receiver seal must precede CompleteAck"
    );
    assert_eq!(
        sender_supervisor
            .lock()
            .unwrap()
            .cross_commit_point(plan(Direction::Send, 2, resume).stamp),
        envoix_attempt_api::CommitPointResult::Retired
    );
    assert_eq!(
        receiver_supervisor
            .lock()
            .unwrap()
            .cross_commit_point(plan(Direction::Receive, 2, resume).stamp),
        envoix_attempt_api::CommitPointResult::Retired
    );
    drain_events(&mut second_sender).await;
    drain_events(&mut second_receiver).await;
    assert!(second_sender.next_event().await.is_none());
    assert!(second_receiver.next_event().await.is_none());
}

#[tokio::test]
async fn failed_attempt_stays_terminal_until_finalize() {
    let bytes = b"small".to_vec();
    let links = link_pair(None);
    drop(links.receiver);
    let supervisor: SharedAttemptSupervisor = Arc::new(Mutex::new(AttemptSupervisor::new()));
    let (token, _) = token_pair();
    let mut handle = spawn_sender(
        plan(Direction::Send, 1, ResumeIntent::Fresh),
        spec(&bytes),
        token,
        MemorySource {
            bytes: Arc::new(bytes),
        },
        links.sender,
        supervisor,
        TestEntropy::new(0x30),
    )
    .unwrap();
    loop {
        let event = handle.next_event().await.unwrap();
        if let AttemptEventKind::Terminal(outcome) = event.kind {
            assert_eq!(outcome, OutcomeCode::PeerLost);
            break;
        }
    }
    handle
        .control()
        .request(RetirementIntent::Finalize)
        .unwrap();
    assert_eq!(
        handle.wait_ack().await.unwrap().outcome(),
        OutcomeCode::PeerLost
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_receiver_retries_a_failed_pairing() {
    let bytes = b"authenticated after one rejected candidate".to_vec();
    let cancellation = SessionCancellation::new();
    let listener = IrohListener::bind(loopback_config(), &cancellation, session_timeouts())
        .await
        .unwrap();
    let target = listener.addr();
    let receiver_supervisor: SharedAttemptSupervisor =
        Arc::new(Mutex::new(AttemptSupervisor::new()));
    let sender_supervisor: SharedAttemptSupervisor = Arc::new(Mutex::new(AttemptSupervisor::new()));
    let sink = MemorySink::default();
    let (good_sender_token, receiver_token) = token_pair_for(b"m4-good-pair");
    let (bad_sender_token, _) = token_pair_for(b"m4-wrong-pair");

    let mut receiver = spawn_iroh_receiver(
        plan(Direction::Receive, 1, ResumeIntent::Fresh),
        spec(&bytes),
        receiver_token,
        sink.clone(),
        listener,
        AuthFailureBudget::new(2).unwrap(),
        receiver_supervisor,
        TestEntropy::new(0xa0),
    )
    .unwrap();
    admit_peer_content(&mut receiver);

    let bad_link = dial(
        loopback_config(),
        target.clone(),
        &cancellation,
        session_timeouts(),
    )
    .await
    .unwrap();
    let mut bad_sender = spawn_sender(
        plan(Direction::Send, 1, ResumeIntent::Fresh),
        spec(&bytes),
        bad_sender_token,
        MemorySource {
            bytes: Arc::new(bytes.clone()),
        },
        bad_link,
        sender_supervisor.clone(),
        TestEntropy::new(0x40),
    )
    .unwrap();
    let rejected_outcome = terminal_outcome(&mut bad_sender).await;
    assert!(matches!(
        rejected_outcome,
        OutcomeCode::Unauthenticated | OutcomeCode::PeerLost
    ));
    bad_sender
        .control()
        .request(RetirementIntent::Finalize)
        .unwrap();
    assert_eq!(
        bad_sender.wait_ack().await.unwrap().outcome(),
        rejected_outcome
    );

    let good_link = dial(loopback_config(), target, &cancellation, session_timeouts())
        .await
        .unwrap();
    let mut good_sender = spawn_sender(
        plan(Direction::Send, 2, ResumeIntent::Fresh),
        spec(&bytes),
        good_sender_token,
        MemorySource {
            bytes: Arc::new(bytes.clone()),
        },
        good_link,
        sender_supervisor,
        TestEntropy::new(0x50),
    )
    .unwrap();
    let (sender_outcome, receiver_outcome) = tokio::join!(
        terminal_outcome(&mut good_sender),
        terminal_outcome(&mut receiver)
    );
    assert_eq!(sender_outcome, OutcomeCode::Completed);
    assert_eq!(receiver_outcome, OutcomeCode::Completed);
    good_sender
        .control()
        .request(RetirementIntent::Finalize)
        .unwrap();
    receiver
        .control()
        .request(RetirementIntent::Finalize)
        .unwrap();
    assert_eq!(
        good_sender.wait_ack().await.unwrap().outcome(),
        OutcomeCode::Completed
    );
    assert_eq!(
        receiver.wait_ack().await.unwrap().outcome(),
        OutcomeCode::Completed
    );
    assert_eq!(sink.sealed(), bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_emits_confirming_between_complete_and_ack() {
    let bytes = (0..8 * 1024)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let source = MemorySource {
        bytes: Arc::new(bytes.clone()),
    };
    let sink = MemorySink::default();
    let sender_supervisor: SharedAttemptSupervisor = Arc::new(Mutex::new(AttemptSupervisor::new()));
    let receiver_supervisor: SharedAttemptSupervisor =
        Arc::new(Mutex::new(AttemptSupervisor::new()));
    let links = link_pair(None);
    let (sender_token, receiver_token) = token_pair();
    let mut sender = spawn_sender(
        plan(Direction::Send, 1, ResumeIntent::Fresh),
        spec(&bytes),
        sender_token,
        source,
        links.sender,
        sender_supervisor,
        TestEntropy::new(0x30),
    )
    .unwrap();
    let mut receiver = spawn_receiver(
        plan(Direction::Receive, 1, ResumeIntent::Fresh),
        spec(&bytes),
        receiver_token,
        sink,
        links.receiver,
        receiver_supervisor,
        TestEntropy::new(0xa0),
    )
    .unwrap();
    admit_peer_content(&mut receiver);
    let sender_control = sender.control();
    let receiver_control = receiver.control();

    let receiver_run = terminal_outcome(&mut receiver);
    let sender_run = async {
        let mut kinds = Vec::new();
        loop {
            let event = sender.next_event().await.unwrap();
            let terminal = matches!(event.kind, AttemptEventKind::Terminal(_));
            kinds.push(event.kind);
            if terminal {
                return kinds;
            }
        }
    };
    let (kinds, receiver_outcome) = tokio::join!(sender_run, receiver_run);
    assert_eq!(receiver_outcome, OutcomeCode::Completed);

    let confirming = kinds
        .iter()
        .position(|kind| matches!(kind, AttemptEventKind::Phase(Phase::Confirming)))
        .expect("sender emits Phase(Confirming) once Complete is sent");
    let last_progress = kinds
        .iter()
        .rposition(|kind| matches!(kind, AttemptEventKind::Progress { .. }))
        .expect("sender emits progress");
    let terminal = kinds.len() - 1;
    assert!(
        last_progress < confirming,
        "Confirming follows the final chunk"
    );
    assert!(confirming < terminal, "Confirming precedes the terminal");
    assert!(matches!(
        kinds[terminal],
        AttemptEventKind::Terminal(OutcomeCode::Completed)
    ));
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| matches!(kind, AttemptEventKind::Phase(Phase::Confirming)))
            .count(),
        1,
        "the confirm window opens exactly once"
    );

    sender_control.request(RetirementIntent::Finalize).unwrap();
    receiver_control
        .request(RetirementIntent::Finalize)
        .unwrap();
    sender.wait_ack().await.unwrap();
    receiver.wait_ack().await.unwrap();
}

/// Silence is not consent.
///
/// A declaration nobody answers must end the attempt, not hold an authenticated
/// session and a writer lease open waiting for one. Before the wait was bounded
/// this hung forever, and every other test in this file hung with it — which is
/// how the flaw was found.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_unanswered_declaration_refuses_rather_than_waits() {
    let bytes = (0..2048_u32).map(|index| index as u8).collect::<Vec<_>>();
    let source = MemorySource {
        bytes: Arc::new(bytes.clone()),
    };
    let sink = MemorySink::default();
    let (sender_supervisor, receiver_supervisor) = (
        Arc::new(Mutex::new(AttemptSupervisor::new())) as SharedAttemptSupervisor,
        Arc::new(Mutex::new(AttemptSupervisor::new())) as SharedAttemptSupervisor,
    );
    let links = link_pair(None);
    let (sender_token, receiver_token) = token_pair();

    // NO authority is wired: the request goes nowhere.
    let mut receiver = spawn_receiver(
        plan(Direction::Receive, 1, ResumeIntent::Fresh),
        spec(&bytes),
        receiver_token,
        sink.clone(),
        links.receiver,
        receiver_supervisor,
        TestEntropy::new(0x90),
    )
    .expect("spawn receiver");
    let sender = spawn_sender(
        plan(Direction::Send, 1, ResumeIntent::Fresh),
        spec(&bytes),
        sender_token,
        source,
        links.sender,
        sender_supervisor,
        TestEntropy::new(0x91),
    )
    .expect("spawn sender");

    let terminal = loop {
        let event = receiver.next_event().await.expect("a terminal");
        if let AttemptEventKind::Terminal(outcome) = event.kind {
            break outcome;
        }
    };
    assert_eq!(
        terminal,
        OutcomeCode::Internal,
        "it ends, and does not hang"
    );
    assert!(
        sink.state.lock().unwrap().staged.is_empty(),
        "an unauthorized declaration never reached the destination"
    );
    drop(sender);
}

/// Answers every declaration with `Admitted`.
///
/// These suites are about the transport, not about what a card decides — but a
/// receive now WAITS for an authority before it touches a destination, so one
/// has to exist. A test that supplied none would be testing the refusal path.
fn admit_peer_content(handle: &mut crate::AttemptHandle) {
    let mut requests = handle.take_peer_content();
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let _ = request.verdict.send(PeerContentVerdict::Admitted);
        }
    });
}
