use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    OpenResult, ResumeIntent, RetirementIntent,
};
use envoix_attempt_iroh::{
    AttemptHandle, AttemptTimeouts, AttemptTransferSpec, SharedAttemptSupervisor,
    spawn_iroh_receiver, spawn_sender,
};
use envoix_invite::RoomCode;
use envoix_outcomes::OutcomeCode;
use envoix_pairing::{
    DataPlaneToken, DescriptorPayload, EntropyError, EntropySource, MAX_MESSAGE_BODY, PairingCode,
    PairingError, WIRE_HEADER_LEN, initiator_start, responder_respond,
};
use envoix_protocol::ContentHash;
use envoix_rendezvous::{ClientConfig, ControlLimits, Role};
use envoix_rendezvous_iroh::{
    BrokerSession, EndpointConfig, IrohClientConfig, bind_endpoint, join_room,
};
use envoix_server::{ServerConfig, run};
use envoix_session_iroh::{
    AuthFailureBudget, BindAddresses, CongestionControl, FlowWindow, IrohListener,
    SessionCancellation, SessionEndpointConfig, SessionTimeouts, SessionTransportConfig, dial,
};
use envoix_transfer::{DurablePrefix, SourceReader, StagingSink, StorageFault, StorageOperation};
use envoix_types::{
    ArtifactId, AttemptGen, ByteCount, Direction, OfferedName, RecordId, TransferId,
};
use iroh::{EndpointAddr, SecretKey};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK_SIZE: u64 = 512 * 1024;
const SOURCE_SIZE: usize = 24 * 1024 * 1024 + 123;

static TRACING: Once = Once::new();

fn init_tracing() {
    TRACING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("info")
            .try_init();
    });
}

struct FixedEntropy {
    next: u8,
}

impl FixedEntropy {
    const fn new(seed: u8) -> Self {
        Self { next: seed }
    }
}

impl EntropySource for FixedEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        for byte in destination {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}

struct FileSource {
    file: File,
}

impl FileSource {
    fn open(path: &Path) -> Self {
        Self {
            file: File::open(path).expect("open source"),
        }
    }
}

impl SourceReader for FileSource {
    fn read_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        self.file
            .seek(SeekFrom::Start(offset.get()))
            .and_then(|_| self.file.read(destination))
            .map_err(|_| StorageFault::new(StorageOperation::ReadSource))
    }
}

/// A real on-disk sink for ONE artifact, with the crash orderings the production
/// store owes: sync bytes before publishing the prefix that names them, publish
/// the prefix by atomic rename, and sync the directory after.
#[derive(Clone)]
struct FileSink {
    root: PathBuf,
    checkpointed: Arc<Notify>,
    append_delay: Duration,
}

impl FileSink {
    fn new(root: PathBuf) -> Self {
        fs::create_dir_all(&root).expect("create sink root");
        Self {
            root,
            checkpointed: Arc::new(Notify::new()),
            append_delay: Duration::from_millis(5),
        }
    }

    fn staged_path(&self) -> PathBuf {
        self.root.join("artifact.part")
    }

    fn resume_path(&self) -> PathBuf {
        self.root.join("artifact.resume")
    }

    fn sealed_path(&self) -> PathBuf {
        self.root.join("artifact.sealed")
    }

    fn read_prefix(&self) -> DurablePrefix {
        let bytes = fs::read(self.resume_path()).expect("read durable prefix");
        decode_prefix(&bytes).expect("valid durable prefix")
    }

    fn corrupt_prefix(&self) {
        let mut staged = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.staged_path())
            .expect("open staged prefix");
        let mut byte = [0; 1];
        staged.read_exact(&mut byte).expect("read staged byte");
        byte[0] ^= 0xff;
        staged.seek(SeekFrom::Start(0)).expect("rewind staging");
        staged.write_all(&byte).expect("corrupt staged byte");
        staged.sync_all().expect("sync corrupted prefix");
    }

    fn sealed_bytes(&self) -> Vec<u8> {
        fs::read(self.sealed_path()).expect("read sealed file")
    }

    fn physical_length(&self) -> Result<u64, StorageFault> {
        match fs::metadata(self.staged_path()) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err(StorageFault::new(StorageOperation::ReadStaging)),
        }
    }

    fn sync_root(&self) -> Result<(), StorageFault> {
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| StorageFault::new(StorageOperation::Checkpoint))
    }
}

impl StagingSink for FileSink {
    type Seal = ();

    fn resume(&mut self) -> Result<DurablePrefix, StorageFault> {
        let prefix = match fs::read(self.resume_path()) {
            Ok(bytes) => decode_prefix(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => DurablePrefix {
                length: ByteCount::new(0),
                digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
            },
            Err(_) => return Err(StorageFault::new(StorageOperation::LoadResume)),
        };
        // A promise longer than the file is a store that lost bytes it said were
        // durable. `set_len` would EXTEND to meet it — zero-filling a hole and
        // calling it a resumable prefix — so it is refused rather than met.
        if prefix.length.get() > self.physical_length()? {
            return Err(StorageFault::new(StorageOperation::LoadResume));
        }
        // Opening discards any tail past the promised prefix. A torn append can
        // leave bytes on disk the engine never accepted, and no reader may ever
        // see them.
        match OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.staged_path())
            .and_then(|staged| staged.set_len(prefix.length.get()).map(|()| staged))
            .and_then(|staged| staged.sync_all())
        {
            Ok(()) => {}
            Err(_) => return Err(StorageFault::new(StorageOperation::TruncateStaging)),
        }
        Ok(prefix)
    }

    fn read_partial_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        let mut staged = match File::open(self.staged_path()) {
            Ok(staged) => staged,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(StorageFault::new(StorageOperation::ReadStaging)),
        };
        staged
            .seek(SeekFrom::Start(offset.get()))
            .and_then(|_| staged.read(destination))
            .map_err(|_| StorageFault::new(StorageOperation::ReadStaging))
    }

    fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), StorageFault> {
        let mut staged = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.staged_path())
            .map_err(|_| StorageFault::new(StorageOperation::AppendStaging))?;
        let length = staged
            .metadata()
            .map_err(|_| StorageFault::new(StorageOperation::AppendStaging))?
            .len();
        if length != offset.get() {
            return Err(StorageFault::new(StorageOperation::AppendStaging));
        }
        staged
            .seek(SeekFrom::End(0))
            .and_then(|_| staged.write_all(bytes))
            .map_err(|_| StorageFault::new(StorageOperation::AppendStaging))?;
        std::thread::sleep(self.append_delay);
        Ok(())
    }

    fn checkpoint(&mut self, prefix: DurablePrefix) -> Result<(), StorageFault> {
        // The engine supplies the length because only it knows what it ACCEPTED,
        // but a sink still corroborates: it cannot make durable what it was
        // never given. The two counters are maintained independently, so this
        // catches them diverging rather than publishing the disagreement.
        //
        // Physical length rather than exact equality, because this double writes
        // a real file and a torn append leaves a longer one. The production
        // lease tracks its own accepted offset and requires equality.
        if prefix.length.get() > self.physical_length()? {
            return Err(StorageFault::new(StorageOperation::Checkpoint));
        }
        // `create` because `reset` publishes the zero prefix BEFORE any byte is
        // written — a promise of nothing is still a promise, and it has to be
        // durable before the bytes it supersedes are gone.
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.staged_path())
            .and_then(|staged| staged.sync_all())
            .map_err(|_| StorageFault::new(StorageOperation::Checkpoint))?;

        let destination = self.resume_path();
        let temporary = destination.with_extension("resume.tmp");
        let mut resume = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| StorageFault::new(StorageOperation::Checkpoint))?;
        resume
            .write_all(&encode_prefix(prefix))
            .and_then(|_| resume.sync_all())
            .map_err(|_| StorageFault::new(StorageOperation::Checkpoint))?;
        fs::rename(temporary, destination)
            .map_err(|_| StorageFault::new(StorageOperation::Checkpoint))?;
        self.sync_root()?;
        if prefix.length.get() > 0 {
            self.checkpointed.notify_one();
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StorageFault> {
        // Publish the zero prefix FIRST, then truncate. A crash after the
        // publication leaves an ignorable tail; the other order leaves a promise
        // about bytes that are gone.
        self.checkpoint(DurablePrefix {
            length: ByteCount::new(0),
            digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
        })?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.staged_path())
            .and_then(|staged| staged.set_len(0).map(|()| staged))
            .and_then(|staged| staged.sync_all())
            .map_err(|_| StorageFault::new(StorageOperation::TruncateStaging))
    }

    fn seal(
        &mut self,
        expected_size: ByteCount,
        digest: ContentHash,
    ) -> Result<Self::Seal, StorageFault> {
        let staged_path = self.staged_path();
        let mut staged = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staged_path)
            .map_err(|_| StorageFault::new(StorageOperation::Seal))?;
        if staged
            .metadata()
            .map_err(|_| StorageFault::new(StorageOperation::Seal))?
            .len()
            != expected_size.get()
        {
            return Err(StorageFault::new(StorageOperation::Seal));
        }
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = staged
                .read(&mut buffer)
                .map_err(|_| StorageFault::new(StorageOperation::Seal))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if ContentHash::from_bytes(*hasher.finalize().as_bytes()) != digest {
            return Err(StorageFault::new(StorageOperation::Seal));
        }
        staged
            .sync_all()
            .map_err(|_| StorageFault::new(StorageOperation::Seal))?;
        drop(staged);
        fs::rename(staged_path, self.sealed_path())
            .map_err(|_| StorageFault::new(StorageOperation::Seal))?;
        let _ = fs::remove_file(self.resume_path());
        self.sync_root()
            .map_err(|_| StorageFault::new(StorageOperation::Seal))
    }
}

fn encode_prefix(prefix: DurablePrefix) -> [u8; 40] {
    let mut encoded = [0; 40];
    encoded[..8].copy_from_slice(&prefix.length.get().to_be_bytes());
    encoded[8..].copy_from_slice(prefix.digest.as_bytes());
    encoded
}

fn decode_prefix(encoded: &[u8]) -> Result<DurablePrefix, StorageFault> {
    if encoded.len() != 40 {
        return Err(StorageFault::new(StorageOperation::LoadResume));
    }
    let length = u64::from_be_bytes(encoded[..8].try_into().unwrap());
    let digest = ContentHash::from_bytes(encoded[8..].try_into().unwrap());
    Ok(DurablePrefix {
        length: ByteCount::new(length),
        digest,
    })
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

fn data_endpoint_config() -> SessionEndpointConfig {
    SessionEndpointConfig {
        bind: BindAddresses::ipv4_only(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        relay: None,
        transport: SessionTransportConfig {
            flow_window: FlowWindow::default(),
            congestion: CongestionControl::Bbr3,
        },
    }
}

fn session_timeouts() -> SessionTimeouts {
    SessionTimeouts::new(
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn attempt_timeouts() -> AttemptTimeouts {
    AttemptTimeouts::new(
        Duration::from_secs(5),
        Duration::from_secs(10),
        Duration::from_secs(5),
        session_timeouts(),
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
    assert!(
        body_len <= MAX_MESSAGE_BODY,
        "pairing body must be bounded before allocation"
    );
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
        .expect("bind sender rendezvous endpoint");
    let receiver_endpoint = bind_endpoint(rendezvous_endpoint_config())
        .await
        .expect("bind receiver rendezvous endpoint");
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

async fn assert_bad_code_rejected(broker: &EndpointAddr) {
    let sender_code = "410000-amber-anchor";
    let receiver_code = "410000-azure-basil";
    let (sender_endpoint, receiver_endpoint, mut sender_session, mut receiver_session) =
        joined_sessions(broker, sender_code).await;
    let sender_code = PairingCode::new(sender_code.as_bytes().to_vec()).unwrap();
    let receiver_code = PairingCode::new(receiver_code.as_bytes().to_vec()).unwrap();
    let (sender_waiting, start) =
        initiator_start(&sender_code, &mut FixedEntropy::new(0x10)).unwrap();
    send_pairing_frame(&mut sender_session, &start).await;
    let start = receive_pairing_frame(&mut receiver_session).await;
    let (receiver_waiting, response) =
        responder_respond(&receiver_code, &start, &mut FixedEntropy::new(0x50)).unwrap();
    send_pairing_frame(&mut receiver_session, &response).await;
    let response = receive_pairing_frame(&mut sender_session).await;
    let (_sender_confirming, confirmation) = sender_waiting.receive_response(&response).unwrap();
    send_pairing_frame(&mut sender_session, &confirmation).await;
    let confirmation = receive_pairing_frame(&mut receiver_session).await;
    assert!(matches!(
        receiver_waiting.verify_initiator(&confirmation),
        Err(PairingError::ConfirmationFailed)
    ));

    let (sender_closed, receiver_closed) =
        tokio::join!(sender_session.close(), receiver_session.close());
    sender_closed.expect("close rejected sender session");
    receiver_closed.expect("close rejected receiver session");
    sender_endpoint.close().await;
    receiver_endpoint.close().await;
}

struct PairMaterial {
    sender_token: DataPlaneToken,
    receiver_token: DataPlaneToken,
    receiver_addr: EndpointAddr,
}

async fn pair_through_broker(
    broker: &EndpointAddr,
    room_code: &str,
    advertised_receiver: &EndpointAddr,
    entropy_seed: u8,
) -> PairMaterial {
    let (sender_endpoint, receiver_endpoint, mut sender_session, mut receiver_session) =
        joined_sessions(broker, room_code).await;
    let sender_code = PairingCode::new(room_code.as_bytes().to_vec()).unwrap();
    let receiver_code = PairingCode::new(room_code.as_bytes().to_vec()).unwrap();
    let (sender_waiting, start) =
        initiator_start(&sender_code, &mut FixedEntropy::new(entropy_seed)).unwrap();
    send_pairing_frame(&mut sender_session, &start).await;
    let start = receive_pairing_frame(&mut receiver_session).await;
    let (receiver_waiting, response) = responder_respond(
        &receiver_code,
        &start,
        &mut FixedEntropy::new(entropy_seed.wrapping_add(0x40)),
    )
    .unwrap();
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
    let sender_descriptor = DescriptorPayload::new(b"headless-sender".to_vec()).unwrap();
    let receiver_descriptor =
        DescriptorPayload::new(serde_json::to_vec(advertised_receiver).unwrap()).unwrap();
    let sealed_sender = sender_paired.seal_descriptor(&sender_descriptor).unwrap();
    let sealed_receiver = receiver_paired
        .seal_descriptor(&receiver_descriptor)
        .unwrap();
    send_pairing_frame(&mut sender_session, &sealed_sender).await;
    send_pairing_frame(&mut receiver_session, &sealed_receiver).await;

    // Each peer consumes exactly one sealed descriptor, then the pairing stream closes.
    let at_sender = receive_pairing_frame(&mut sender_session).await;
    let at_receiver = receive_pairing_frame(&mut receiver_session).await;
    let opened_by_sender = sender_paired.open_peer_descriptor(&at_sender).unwrap();
    let opened_by_receiver = receiver_paired.open_peer_descriptor(&at_receiver).unwrap();
    assert_eq!(
        opened_by_receiver.payload().as_bytes(),
        sender_descriptor.as_bytes()
    );
    assert_eq!(opened_by_sender.data_token(), receiver_paired.data_token());
    assert_eq!(opened_by_receiver.data_token(), sender_paired.data_token());
    let receiver_addr: EndpointAddr =
        serde_json::from_slice(opened_by_sender.payload().as_bytes()).unwrap();
    assert_eq!(&receiver_addr, advertised_receiver);

    let (sender_closed, receiver_closed) =
        tokio::join!(sender_session.close(), receiver_session.close());
    sender_closed.expect("close sender pairing session");
    receiver_closed.expect("close receiver pairing session");
    sender_endpoint.close().await;
    receiver_endpoint.close().await;

    PairMaterial {
        sender_token: sender_paired.into_data_token(),
        receiver_token: receiver_paired.into_data_token(),
        receiver_addr,
    }
}

struct AttemptCase {
    room_code: &'static str,
    sender_plan: AttemptPlan,
    receiver_plan: AttemptPlan,
    spec: AttemptTransferSpec,
    source_path: PathBuf,
    sink: FileSink,
    sender_supervisor: SharedAttemptSupervisor,
    receiver_supervisor: SharedAttemptSupervisor,
    entropy_seed: u8,
}

struct RunningAttempts {
    sender: AttemptHandle,
    receiver: AttemptHandle,
}

async fn start_real_attempt(broker: &EndpointAddr, case: AttemptCase) -> RunningAttempts {
    let cancellation = SessionCancellation::new();
    let listener = IrohListener::bind(data_endpoint_config(), &cancellation, session_timeouts())
        .await
        .expect("bind data listener");
    let advertised_receiver = listener.addr();
    let pair = pair_through_broker(
        broker,
        case.room_code,
        &advertised_receiver,
        case.entropy_seed,
    )
    .await;
    let receiver = spawn_iroh_receiver(
        case.receiver_plan,
        case.spec.clone(),
        pair.receiver_token,
        case.sink,
        listener,
        AuthFailureBudget::new(1).unwrap(),
        case.receiver_supervisor,
        FixedEntropy::new(case.entropy_seed.wrapping_add(0x80)),
    )
    .expect("spawn receiver");
    let sender_link = dial(
        data_endpoint_config(),
        pair.receiver_addr,
        &cancellation,
        session_timeouts(),
    )
    .await
    .expect("dial advertised receiver");
    let sender = spawn_sender(
        case.sender_plan,
        case.spec,
        pair.sender_token,
        FileSource::open(&case.source_path),
        sender_link,
        case.sender_supervisor,
        FixedEntropy::new(case.entropy_seed.wrapping_add(0xc0)),
    )
    .expect("spawn sender");
    RunningAttempts { sender, receiver }
}

/// What one side reported over its whole life.
struct AttemptOutcome {
    outcome: OutcomeCode,
    first_progress: Option<ByteCount>,
    /// The offset the peers SETTLED on, which is the only honest answer to
    /// "how much did this resume skip" — the plan carries no offset, and this
    /// is what the product records.
    established: Option<ByteCount>,
}

async fn terminal_and_first_progress(handle: &mut AttemptHandle) -> AttemptOutcome {
    let mut first_progress = None;
    let mut established = None;
    loop {
        let event = handle.next_event().await.expect("attempt emits terminal");
        match event.kind {
            AttemptEventKind::Progress { transferred } => {
                first_progress.get_or_insert(transferred);
            }
            AttemptEventKind::ResumeEstablished { offset } => {
                assert!(
                    established.replace(offset).is_none(),
                    "the settled resume offset is reported once per attempt"
                );
                assert!(
                    first_progress.is_none(),
                    "a card must learn what was resumed before it is shown progress"
                );
            }
            AttemptEventKind::Terminal(outcome) => {
                return AttemptOutcome {
                    outcome,
                    first_progress,
                    established,
                };
            }
            AttemptEventKind::Phase(_) => {}
        }
    }
}

/// The settled resume offset both sides agreed on, and the sender's first
/// progress.
struct Completion {
    first_progress: ByteCount,
    established: ByteCount,
}

async fn complete_attempts(mut running: RunningAttempts) -> Completion {
    let sender_control = running.sender.control();
    let receiver_control = running.receiver.control();
    let (sender, receiver) = timeout(TEST_TIMEOUT, async {
        tokio::join!(
            terminal_and_first_progress(&mut running.sender),
            terminal_and_first_progress(&mut running.receiver)
        )
    })
    .await
    .expect("transfer completion deadline");
    assert_eq!(sender.outcome, OutcomeCode::Completed);
    assert_eq!(receiver.outcome, OutcomeCode::Completed);
    // Both sides settle on the SAME number. They compute it independently — the
    // sender from its own prefix comparison, the receiver from what the sender
    // then sent it — so agreement is a real check rather than an echo.
    assert_eq!(
        sender.established, receiver.established,
        "the two sides disagree about what was resumed"
    );
    sender_control.request(RetirementIntent::Finalize).unwrap();
    receiver_control
        .request(RetirementIntent::Finalize)
        .unwrap();
    let (sender_ack, receiver_ack) = timeout(TEST_TIMEOUT, async {
        tokio::join!(running.sender.wait_ack(), running.receiver.wait_ack())
    })
    .await
    .expect("retirement acknowledgement deadline");
    assert_eq!(sender_ack.unwrap().outcome(), OutcomeCode::Completed);
    assert_eq!(receiver_ack.unwrap().outcome(), OutcomeCode::Completed);
    Completion {
        first_progress: sender
            .first_progress
            .expect("non-empty transfer emits progress"),
        established: sender.established.expect("every attempt settles a resume"),
    }
}

async fn cancel_after_checkpoint(
    mut running: RunningAttempts,
    sink: &FileSink,
) -> (DurablePrefix, AttemptEvent) {
    timeout(TEST_TIMEOUT, sink.checkpointed.notified())
        .await
        .expect("receiver checkpoints before completion");
    let stale_event = loop {
        let event = running
            .receiver
            .next_event()
            .await
            .expect("receiver emits progress");
        if matches!(event.kind, AttemptEventKind::Progress { .. }) {
            break event;
        }
    };
    running
        .sender
        .control()
        .request(RetirementIntent::Cancel)
        .unwrap();
    running
        .receiver
        .control()
        .request(RetirementIntent::Cancel)
        .unwrap();
    let (sender, receiver) = timeout(TEST_TIMEOUT, async {
        tokio::join!(
            terminal_and_first_progress(&mut running.sender),
            terminal_and_first_progress(&mut running.receiver)
        )
    })
    .await
    .expect("cancelled transfer terminal deadline");
    assert_eq!(sender.outcome, OutcomeCode::Cancelled);
    assert_eq!(receiver.outcome, OutcomeCode::Cancelled);
    let (sender_ack, receiver_ack) = timeout(TEST_TIMEOUT, async {
        tokio::join!(running.sender.wait_ack(), running.receiver.wait_ack())
    })
    .await
    .expect("cancel retirement deadline");
    assert_eq!(sender_ack.unwrap().outcome(), OutcomeCode::Cancelled);
    assert_eq!(receiver_ack.unwrap().outcome(), OutcomeCode::Cancelled);
    let prefix = sink.read_prefix();
    assert!(prefix.length.get() > 0);
    assert!(prefix.length.get() < SOURCE_SIZE as u64);
    (prefix, stale_event)
}

fn plan_pair(
    card: u64,
    generation: u32,
    transfer: TransferId,
    artifact: ArtifactId,
    resume: ResumeIntent,
) -> (AttemptPlan, AttemptPlan) {
    let stamp = AttemptStamp {
        card: RecordId::new(card),
        generation: AttemptGen::new(generation),
    };
    (
        AttemptPlan {
            stamp,
            direction: Direction::Send,
            transfer,
            artifact,
            resume,
        },
        AttemptPlan {
            stamp,
            direction: Direction::Receive,
            transfer,
            artifact,
            resume,
        },
    )
}

fn transfer_spec() -> AttemptTransferSpec {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&source_bytes());
    AttemptTransferSpec {
        offered_name: OfferedName::from_untrusted("walking-slice.bin").unwrap(),
        file_size: ByteCount::new(SOURCE_SIZE as u64),
        chunk_size: ByteCount::new(CHUNK_SIZE),
        claimed_complete: None,
        // What staging would have established. A send states what it intends to
        // send, and this slice's source is deterministic, so it can say so
        // exactly — including across the resume, which re-reads the same bytes.
        content_hash: Some(ContentHash::from_bytes(*hasher.finalize().as_bytes())),
        timeouts: attempt_timeouts(),
    }
}

/// The deterministic payload this slice sends, in one place so the spec's
/// digest and the file on disk cannot drift apart.
fn source_bytes() -> Vec<u8> {
    (0..SOURCE_SIZE)
        .map(|index| ((index * 31 + index / 251) % 256) as u8)
        .collect()
}

fn supervisors() -> (SharedAttemptSupervisor, SharedAttemptSupervisor) {
    (
        Arc::new(Mutex::new(AttemptSupervisor::new())),
        Arc::new(Mutex::new(AttemptSupervisor::new())),
    )
}

fn assert_same_file(expected: &[u8], actual: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    assert_eq!(blake3::hash(actual), blake3::hash(expected));
    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_end_to_end_transfer_and_resume() {
    init_tracing();
    let directory = TempDir::new().unwrap();
    let source_bytes = source_bytes();
    let source_path = directory.path().join("source.bin");
    fs::write(&source_path, &source_bytes).unwrap();

    let mut server_config = ServerConfig::operational_defaults();
    server_config.bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    server_config.mailbox_bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    server_config.node_key_path = directory.path().join("rendezvous-node.key");
    server_config.room_ttl = Duration::from_secs(10);
    server_config.relay_ttl = Duration::from_secs(10);
    server_config.join_deadline = Duration::from_secs(5);
    server_config.close_grace = Duration::from_secs(5);
    server_config.handshake_deadline = Duration::from_secs(5);
    server_config.bind_deadline = Duration::from_secs(5);
    server_config.max_waiting_rooms = 16;
    server_config.max_connections = 16;
    let server = run(server_config).await.unwrap();
    let broker = server.endpoint_addr().clone();

    assert_bad_code_rejected(&broker).await;

    let clean_transfer = TransferId::from_bytes([0x11; 16]);
    let clean_artifact = ArtifactId::from_bytes([0x21; 16]);
    let clean_sink = FileSink::new(directory.path().join("clean"));
    let (clean_sender_supervisor, clean_receiver_supervisor) = supervisors();
    let (clean_sender_plan, clean_receiver_plan) =
        plan_pair(100, 1, clean_transfer, clean_artifact, ResumeIntent::Fresh);
    let clean = start_real_attempt(
        &broker,
        AttemptCase {
            room_code: "410001-amber-anchor",
            sender_plan: clean_sender_plan,
            receiver_plan: clean_receiver_plan,
            spec: transfer_spec(),
            source_path: source_path.clone(),
            sink: clean_sink.clone(),
            sender_supervisor: clean_sender_supervisor,
            receiver_supervisor: clean_receiver_supervisor,
            entropy_seed: 0x11,
        },
    )
    .await;
    assert_eq!(clean.sender.open_result(), OpenResult::Opened);
    assert_eq!(clean.receiver.open_result(), OpenResult::Opened);
    let clean_completion = complete_attempts(clean).await;
    assert_eq!(
        clean_completion.established,
        ByteCount::new(0),
        "a fresh transfer settles on resuming nothing"
    );
    assert_same_file(&source_bytes, &clean_sink.sealed_bytes());

    let resume_transfer = TransferId::from_bytes([0x12; 16]);
    let resume_artifact = ArtifactId::from_bytes([0x22; 16]);
    let resume_sink = FileSink::new(directory.path().join("resume"));
    let (resume_sender_supervisor, resume_receiver_supervisor) = supervisors();
    let (sender_gen1, receiver_gen1) = plan_pair(
        101,
        1,
        resume_transfer,
        resume_artifact,
        ResumeIntent::Fresh,
    );
    let gen1 = start_real_attempt(
        &broker,
        AttemptCase {
            room_code: "410002-amber-anchor",
            sender_plan: sender_gen1,
            receiver_plan: receiver_gen1,
            spec: transfer_spec(),
            source_path: source_path.clone(),
            sink: resume_sink.clone(),
            sender_supervisor: resume_sender_supervisor.clone(),
            receiver_supervisor: resume_receiver_supervisor.clone(),
            entropy_seed: 0x22,
        },
    )
    .await;
    let (resume_prefix, stale_event) = cancel_after_checkpoint(gen1, &resume_sink).await;
    let resume = ResumeIntent::Allowed;
    let (sender_gen2, receiver_gen2) = plan_pair(101, 2, resume_transfer, resume_artifact, resume);
    let gen2 = start_real_attempt(
        &broker,
        AttemptCase {
            room_code: "410003-amber-anchor",
            sender_plan: sender_gen2,
            receiver_plan: receiver_gen2,
            spec: transfer_spec(),
            source_path: source_path.clone(),
            sink: resume_sink.clone(),
            sender_supervisor: resume_sender_supervisor,
            receiver_supervisor: resume_receiver_supervisor.clone(),
            entropy_seed: 0x33,
        },
    )
    .await;
    assert_eq!(gen2.sender.open_result(), OpenResult::Superseded);
    assert_eq!(gen2.receiver.open_result(), OpenResult::Superseded);
    assert_eq!(
        resume_receiver_supervisor
            .lock()
            .unwrap()
            .observe(stale_event),
        EventAdmission::Stale
    );
    let resumed = complete_attempts(gen2).await;
    // An INTACT prefix is adopted, so the settled offset is exactly what the
    // previous run made durable — no longer inferred from where progress
    // happened to start.
    assert_eq!(
        resumed.established, resume_prefix.length,
        "an intact durable prefix must be resumed in full"
    );
    assert!(
        resumed.first_progress.get() > resume_prefix.length.get(),
        "gen-2 must start after the verified prefix"
    );
    assert!(
        resumed.first_progress.get() <= resume_prefix.length.get() + CHUNK_SIZE,
        "the first gen-2 progress must account for only one tail chunk"
    );
    assert_same_file(&source_bytes, &resume_sink.sealed_bytes());

    let corrupt_transfer = TransferId::from_bytes([0x13; 16]);
    let corrupt_artifact = ArtifactId::from_bytes([0x23; 16]);
    let corrupt_sink = FileSink::new(directory.path().join("corrupt"));
    let (corrupt_sender_supervisor, corrupt_receiver_supervisor) = supervisors();
    let (corrupt_sender_gen1, corrupt_receiver_gen1) = plan_pair(
        102,
        1,
        corrupt_transfer,
        corrupt_artifact,
        ResumeIntent::Fresh,
    );
    let corrupt_gen1 = start_real_attempt(
        &broker,
        AttemptCase {
            room_code: "410004-amber-anchor",
            sender_plan: corrupt_sender_gen1,
            receiver_plan: corrupt_receiver_gen1,
            spec: transfer_spec(),
            source_path: source_path.clone(),
            sink: corrupt_sink.clone(),
            sender_supervisor: corrupt_sender_supervisor.clone(),
            receiver_supervisor: corrupt_receiver_supervisor.clone(),
            entropy_seed: 0x44,
        },
    )
    .await;
    let (corrupt_prefix, _) = cancel_after_checkpoint(corrupt_gen1, &corrupt_sink).await;
    corrupt_sink.corrupt_prefix();
    let corrupt_resume = ResumeIntent::Allowed;
    let (corrupt_sender_gen2, corrupt_receiver_gen2) =
        plan_pair(102, 2, corrupt_transfer, corrupt_artifact, corrupt_resume);
    let corrupt_gen2 = start_real_attempt(
        &broker,
        AttemptCase {
            room_code: "410005-amber-anchor",
            sender_plan: corrupt_sender_gen2,
            receiver_plan: corrupt_receiver_gen2,
            spec: transfer_spec(),
            source_path,
            sink: corrupt_sink.clone(),
            sender_supervisor: corrupt_sender_supervisor,
            receiver_supervisor: corrupt_receiver_supervisor,
            entropy_seed: 0x55,
        },
    )
    .await;
    let restarted = complete_attempts(corrupt_gen2).await;
    // The card asked to resume and a nonzero prefix WAS durable, but it failed
    // its own digest — so the settled answer is zero and the card is told so.
    // Under the old plan-offset comparison this run was a protocol violation.
    assert_eq!(
        restarted.established,
        ByteCount::new(0),
        "a divergent prefix must settle on resuming nothing"
    );
    assert!(
        corrupt_prefix.length.get() > 0,
        "the case needs a prefix to diverge from"
    );
    assert!(
        restarted.first_progress.get() <= CHUNK_SIZE,
        "a divergent prefix must restart sender progress at zero"
    );
    assert_same_file(&source_bytes, &corrupt_sink.sealed_bytes());

    server.shutdown().await.unwrap();
}
