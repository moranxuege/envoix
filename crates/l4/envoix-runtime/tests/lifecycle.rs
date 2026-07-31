//! RT1 lifecycle proof over the real operation store + supervisor.
//!
//! The executor is scripted (a controllable signal producer), but the runtime's
//! `AttemptSupervisor` is real, so every `RetirementAck` fed to the reducer is a
//! genuine, non-forgeable token.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use envoix_attempt_api::{AttemptEventKind, RetirementIntent};
use envoix_capabilities::{
    AcquiredSelection, Admission, Duty, DutyLedger, DutyProvenance, DutyReport, DutyResult,
    Registration, SourceAcquisitionKey, SourceReport, SourceRetention, SourceSeekability,
};
use envoix_evidence::EvidenceRecord;
use envoix_operation_store::OperationStore;
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_product::{
    AcceptedSourceOffer, ApplyOutcome, CommitError, CommittedSession, ContentHash, IdentityError,
    IdentitySource, NewTransfer, ProductCommand, ProductInput, ProductState, Quiescence,
    RecordDecode, RecordStore, SourceLifecycle, SourcePossession, StagedContent, TransferContent,
    TransferRecord, decode_record,
};
use envoix_runtime::{
    AcquireError, AttemptExecution, AttemptExecutor, AttemptLaunch, CardUpdateKind,
    CommandCompletion, CommandVerdict, DutyKind, EvidenceSink, EvidenceSinkError, ExecutorSignal,
    LosslessUpdateKind, NoSourceStaging, PlatformPorts, PreparedReceiveSink, PreparedSinkResolver,
    PreparedSource, PreparedSourceResolver, Runtime, RuntimeConfig, SessionProvider,
    ShutdownReport, SinkOpenError, SourceLocator, SourceResolveError, StagedIdentity, StopSignal,
    SubscribeError, TryRecvError, stop_channel,
};
use envoix_storage_api::Durability;
use envoix_storage_local::LocalStorage;
use envoix_types::{ByteCount, CommandId, Direction, OfferedName, RecordId, TransferId};
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---- durable session provider (the composition-root L2 binding, dev only) ----

struct OpStoreRecords {
    root: PathBuf,
    operation: Option<OperationStore<LocalStorage>>,
}

impl OpStoreRecords {
    fn deferred(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            operation: None,
        }
    }

    fn opened(root: &Path, card: RecordId) -> Option<Self> {
        let storage = LocalStorage::open(root).ok()?;
        let operation = OperationStore::open(storage, card).ok()?;
        Some(Self {
            root: root.to_path_buf(),
            operation: Some(operation),
        })
    }

    fn latest(&self) -> Option<Vec<u8>> {
        self.operation.as_ref()?.latest_record().map(<[u8]>::to_vec)
    }
}

impl RecordStore for OpStoreRecords {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        let card = match decode_record(encoded).map_err(|_| CommitError)? {
            RecordDecode::Loaded(record) => record.identity.card,
            RecordDecode::UnsupportedFuture { .. } => return Err(CommitError),
        };
        if self.operation.is_none() {
            let storage = LocalStorage::open(&self.root).map_err(|_| CommitError)?;
            self.operation = Some(OperationStore::open(storage, card).map_err(|_| CommitError)?);
        }
        let operation = self.operation.as_mut().ok_or(CommitError)?;
        if operation.record_id() != card {
            return Err(CommitError);
        }
        operation
            .commit_record(encoded, Durability::Durable)
            .map_err(|_| CommitError)?;
        Ok(())
    }
}

struct OpStoreProvider {
    root: PathBuf,
}

impl SessionProvider for OpStoreProvider {
    type Store = OpStoreRecords;

    fn restore(&self, card: RecordId) -> Option<CommittedSession<OpStoreRecords>> {
        let store = OpStoreRecords::opened(&self.root, card)?;
        let encoded = store.latest()?;
        let record = match decode_record(&encoded).ok()? {
            RecordDecode::Loaded(record) => record,
            RecordDecode::UnsupportedFuture { .. } => return None,
        };
        Some(CommittedSession::from_record(
            *record,
            store,
            NonZeroUsize::MIN,
        ))
    }
}

// ---- scripted executor: the supervisor (owned by the runtime) mints real acks ----

#[derive(Clone, Copy)]
enum Script {
    /// Runs to a completed commit, then stops when asked.
    Complete,
    /// Emits one phase then runs until stopped.
    RunUntilStop,
    /// Panics inside `start` to exercise supervision.
    PanicOnStart,
}

#[derive(Clone)]
struct ScriptedExecutor {
    scripts: Arc<Mutex<HashMap<TransferId, Script>>>,
    signals: Arc<Mutex<HashMap<TransferId, mpsc::Sender<ExecutorSignal>>>>,
    default: Script,
}

impl ScriptedExecutor {
    fn new(default: Script) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(HashMap::new())),
            signals: Arc::new(Mutex::new(HashMap::new())),
            default,
        }
    }

    fn set(&self, transfer: TransferId, script: Script) {
        self.scripts.lock().unwrap().insert(transfer, script);
    }

    async fn signal(&self, transfer: TransferId, signal: ExecutorSignal) {
        let sender = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(sender) = self.signals.lock().unwrap().get(&transfer).cloned() {
                    break sender;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the executor should start");
        sender.send(signal).await.expect("the actor is still live");
    }
}

impl AttemptExecutor for ScriptedExecutor {
    fn start(&self, launch: AttemptLaunch) -> AttemptExecution {
        let plan = launch.plan();
        let script = self
            .scripts
            .lock()
            .unwrap()
            .get(&plan.transfer)
            .copied()
            .unwrap_or(self.default);
        if let Script::PanicOnStart = script {
            panic!("scripted executor panic for supervision test");
        }
        let (signal_tx, signals) = mpsc::channel(16);
        self.signals
            .lock()
            .unwrap()
            .insert(plan.transfer, signal_tx.clone());
        let (stop, token) = stop_channel();
        tokio::spawn(async move {
            let _ = signal_tx
                .send(ExecutorSignal::Event(AttemptEventKind::Phase(
                    Phase::Transferring,
                )))
                .await;
            if let Script::Complete = script {
                let _ = signal_tx.send(ExecutorSignal::CommitCrossed).await;
                let _ = signal_tx
                    .send(ExecutorSignal::Event(AttemptEventKind::Terminal(
                        OutcomeCode::Completed,
                    )))
                    .await;
            }
            token.stopped().await;
            let _ = signal_tx.send(ExecutorSignal::Stopped).await;
        });
        AttemptExecution { signals, stop }
    }
}

// ---- hostile evidence sink: slow, full/erroring, panicking, then closed ----

struct HostileEvidenceState {
    calls: AtomicUsize,
    released: Mutex<bool>,
    release: Condvar,
}

#[derive(Clone)]
struct HostileEvidence {
    state: Arc<HostileEvidenceState>,
}

impl HostileEvidence {
    fn new() -> Self {
        Self {
            state: Arc::new(HostileEvidenceState {
                calls: AtomicUsize::new(0),
                released: Mutex::new(false),
                release: Condvar::new(),
            }),
        }
    }

    fn calls(&self) -> usize {
        self.state.calls.load(Ordering::SeqCst)
    }

    fn release(&self) {
        *self.state.released.lock().unwrap() = true;
        self.state.release.notify_all();
    }
}

impl EvidenceSink for HostileEvidence {
    fn record(&self, _record: EvidenceRecord) -> Result<(), EvidenceSinkError> {
        match self.state.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                let released = self.state.released.lock().unwrap();
                let _guard = self
                    .state
                    .release
                    .wait_while(released, |released| !*released)
                    .unwrap();
                Err(EvidenceSinkError::Full)
            }
            1 => panic!("hostile evidence sink panic"),
            _ => Err(EvidenceSinkError::Closed),
        }
    }
}

// ---- helpers ----

struct FixedIdentity {
    next: u8,
}

impl FixedIdentity {
    const fn new(seed: u8) -> Self {
        Self { next: seed }
    }
}

impl IdentitySource for FixedIdentity {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        for byte in destination {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}

fn config(max_live: usize) -> RuntimeConfig {
    RuntimeConfig::new(
        NonZeroUsize::new(max_live).unwrap(),
        Duration::from_secs(2),
        NonZeroUsize::new(3).unwrap(),
    )
}

type Session = CommittedSession<OpStoreRecords>;

/// A card as a frontend states it. No name, total or source decision crosses
/// here: the authority derives the opening source state from the direction.
fn new_transfer(direction: Direction) -> NewTransfer {
    NewTransfer {
        direction,
        participation: envoix_product::RoomParticipation::Minted,
        pairing: None,
    }
}

/// The platform's answer, admitted through a real ledger — the only way to
/// obtain an `AdmittedSourceResult`.
fn source_settled(provenance: DutyProvenance, report: SourceReport) -> ProductInput {
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
        report: DutyReport::Source(report),
    }) else {
        panic!("an outstanding source duty admits its first result");
    };
    ProductInput::SourceSettled(admitted.into_source().expect("a source answer"))
}

/// Walks a sending session from "needs a document" to a live attempt, and
/// returns the outcome that LAUNCHED it — which is what these tests assert on.
fn stage_the_source(
    session: &mut Session,
    _created: &envoix_product::ApplyOutcome,
) -> ApplyOutcome {
    // The acquisition a frontend answers is the one the card PUBLISHES, and
    // read/9 carries it as the `pick_source` action. `current_acquisition()` is
    // the same derivation the projection uses and the authority checks against.
    let provenance = session.record().current_acquisition().provenance();
    session
        .apply(ProductInput::SourceOffered {
            offer: AcceptedSourceOffer::of_one_document(
                SourceAcquisitionKey::of(provenance),
                OfferedName::from_untrusted("payload.bin").unwrap(),
                Some(ByteCount::new(1024)),
            ),
        })
        .unwrap();
    session
        .apply(source_settled(
            provenance,
            SourceReport::Acquired(AcquiredSelection::of_one(
                SourceRetention::Persisted,
                SourceSeekability::Seekable,
            )),
        ))
        .unwrap();
    let stamp = session.record().stamp();
    session
        .apply(ProductInput::StageComplete {
            stamp,
            content: StagedContent::new(
                TransferContent::new(
                    OfferedName::from_untrusted("payload.bin").unwrap(),
                    ByteCount::new(1024),
                ),
                ContentHash::from_bytes([7; 32]),
            ),
            possession: SourcePossession::Streamed,
        })
        .unwrap();
    session
        .apply(ProductInput::StagingRetired { stamp })
        .unwrap()
}

/// A session whose source is established. A receiver needs none; a sender is
/// walked through the whole acquisition, because there is no longer any other
/// way for one to reach the wire.
fn create_session(
    root: &Path,
    direction: Direction,
    seed: u8,
) -> (Session, envoix_product::ApplyOutcome) {
    let (mut session, created) = CommittedSession::create(
        new_transfer(direction),
        &mut FixedIdentity::new(seed),
        OpStoreRecords::deferred(root),
        NonZeroUsize::new(3).unwrap(),
    )
    .unwrap();
    if direction == Direction::Receive {
        return (session, created);
    }
    let launched = stage_the_source(&mut session, &created);
    (session, launched)
}

/// A sender that has been created and is still waiting to be given a document.
fn create_awaiting_session(root: &Path, seed: u8) -> (Session, envoix_product::ApplyOutcome) {
    CommittedSession::create(
        new_transfer(Direction::Send),
        &mut FixedIdentity::new(seed),
        OpStoreRecords::deferred(root),
        NonZeroUsize::new(3).unwrap(),
    )
    .unwrap()
}

/// Reads a card's authoritative record straight from the durable store — the
/// runtime owns no truth, so a hibernated card is read here, not via a snapshot.
fn durable_state(root: &Path, card: RecordId) -> TransferRecord {
    let store = OpStoreRecords::opened(root, card).expect("the card store opens");
    let encoded = store.latest().expect("a committed product record");
    match decode_record(&encoded).unwrap() {
        RecordDecode::Loaded(record) => *record,
        RecordDecode::UnsupportedFuture { .. } => panic!("the record decodes"),
    }
}

/// Waits for a card to hibernate (leave the live registry).
async fn settle<P: SessionProvider, E: AttemptExecutor>(runtime: &Runtime<P, E>, card: RecordId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.is_live(card) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("card should settle");
}

async fn wait_for_bytes<P: SessionProvider, E: AttemptExecutor>(
    runtime: &Runtime<P, E>,
    card: RecordId,
    expected: u64,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if runtime
                .snapshot(card)
                .await
                .is_some_and(|record| record.bytes == ByteCount::new(expected))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the expected progress should be projected");
}

async fn wait_for_state<P: SessionProvider, E: AttemptExecutor>(
    runtime: &Runtime<P, E>,
    card: RecordId,
    expected: ProductState,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if runtime
                .snapshot(card)
                .await
                .is_some_and(|record| record.state == expected)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the expected state should be projected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evidence_failure_is_non_authoritative() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let evidence = HostileEvidence::new();
    let runtime = Runtime::start_with_evidence(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
        evidence.clone(),
    );

    let (session, outcome) = create_session(root, Direction::Receive, 0x20);
    let card = session.record().identity.card;
    let transfer = session.record().identity.transfer;
    runtime.admit(session, outcome).unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while evidence.calls() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the deliberately slow sink should receive the first event");

    // The evidence worker is blocked above. More than its bounded lane capacity
    // is emitted, proving saturation/drops cannot backpressure the card actor.
    for transferred in 1..=96 {
        executor
            .signal(
                transfer,
                ExecutorSignal::Event(AttemptEventKind::Progress {
                    transferred: ByteCount::new(transferred),
                }),
            )
            .await;
    }
    executor
        .signal(transfer, ExecutorSignal::CommitCrossed)
        .await;
    executor
        .signal(
            transfer,
            ExecutorSignal::Event(AttemptEventKind::Terminal(OutcomeCode::Completed)),
        )
        .await;

    settle(&runtime, card).await;
    assert_eq!(durable_state(root, card).state, ProductState::Completed);

    // Once released, the sink returns a typed full error, panics, and then
    // reports itself closed. All are contained after durable completion too.
    evidence.release();
    tokio::time::timeout(Duration::from_secs(5), async {
        while evidence.calls() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued evidence should exercise errors and panic containment");
    assert_eq!(durable_state(root, card).state, ProductState::Completed);
    runtime.shutdown().await;
}

// ---- the required lifecycle proof ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_lease_shutdown_hibernate_restore() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    // --- exclusive lease + shared handle (idempotent bootstrap) ---
    let (session, outcome) = create_session(root, Direction::Send, 0x10);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    assert!(runtime.is_live(card));
    // A live card yields a derived read snapshot.
    assert_eq!(runtime.snapshot(card).await.unwrap().identity.card, card);
    // A second acquire of a live card is refused (single writer), never a second
    // writer and never a block.
    assert_eq!(runtime.restore(card), Err(AcquireError::AlreadyLive));
    // A clone is the same handle onto the same registry.
    let mirror = runtime.clone();
    assert!(mirror.is_live(card));

    // --- shutdown preserves the transfer; it does NOT cancel ---
    // One live card, torn down cleanly within the grace (nothing force-aborted).
    assert_eq!(
        runtime.shutdown().await,
        ShutdownReport {
            cards: 1,
            forced: 0
        }
    );
    assert!(!runtime.is_live(card));
    // Idempotent: a second shutdown finds nothing live.
    assert_eq!(runtime.shutdown().await, ShutdownReport::default());

    // --- owns no truth: a fresh runtime over the SAME durable store restores it ---
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );
    runtime.restore(card).unwrap();
    // Restore reconciles + commits, then the quiescent card hibernates (owns no
    // truth): read the reconstructed lifecycle from the durable store.
    settle(&runtime, card).await;
    assert!(!runtime.is_live(card));
    let restored = durable_state(root, card);
    assert_eq!(restored.identity.card, card);
    // A lost attempt becomes Paused — never a fabricated terminal or false completion.
    assert!(matches!(restored.state, ProductState::Paused(_)));
    assert!(matches!(restored.quiescence, Quiescence::Quiescent));

    // --- live completion delivers a GENUINE ack, then hibernates ---
    let (session, outcome) = create_session(root, Direction::Receive, 0x60);
    let done_card = session.record().identity.card;
    let done_transfer = session.record().identity.transfer;
    executor.set(done_transfer, Script::Complete);
    runtime.admit(session, outcome).unwrap();
    settle(&runtime, done_card).await;
    assert!(!runtime.is_live(done_card));
    // The durable store holds the Completed truth; Quiescent proves the GENUINE
    // RetirementAck was delivered (the only path from Retiring to Quiescent).
    let completed = durable_state(root, done_card);
    assert_eq!(completed.state, ProductState::Completed);
    assert!(matches!(completed.quiescence, Quiescence::Quiescent));

    runtime.shutdown().await;
}

// ---- supervision: a panicking card actor does not wedge the runtime ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervision_panic_does_not_wedge_runtime() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Send, 0x10);
    let card = session.record().identity.card;
    let transfer = session.record().identity.transfer;
    executor.set(transfer, Script::PanicOnStart);
    runtime.admit(session, outcome).unwrap();
    // The actor panics dispatching StartAttempt; its guard releases the lease and
    // permit on unwind, so the registry cleans up.
    settle(&runtime, card).await;
    assert!(!runtime.is_live(card));
    assert_eq!(runtime.live_cards(), 0);

    // The runtime is still fully usable.
    let (session, outcome) = create_session(root, Direction::Receive, 0x60);
    let survivor = session.record().identity.card;
    let survivor_transfer = session.record().identity.transfer;
    executor.set(survivor_transfer, Script::Complete);
    runtime.admit(session, outcome).unwrap();
    settle(&runtime, survivor).await;
    assert_eq!(durable_state(root, survivor).state, ProductState::Completed);
    runtime.shutdown().await;
}

// ---- admission: over-cap is refused; a freed permit admits again ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_rejects_over_cap() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(1),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (first, first_outcome) = create_session(root, Direction::Send, 0x10);
    let first_card = first.record().identity.card;
    runtime.admit(first, first_outcome).unwrap();

    // The single permit is taken.
    let (second, second_outcome) = create_session(root, Direction::Send, 0x40);
    assert_eq!(
        runtime.admit(second, second_outcome),
        Err(AcquireError::AtCapacity)
    );

    // Freeing the first card's permit (cancel → retire → hibernate) admits again.
    let commander = runtime
        .subscribe(first_card, NonZeroUsize::new(4).unwrap())
        .unwrap();
    let verdict = runtime
        .submit_command(
            &commander,
            CommandId::from_bytes([0xC1; 16]),
            ProductCommand::Cancel,
        )
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("a fresh command id is accepted");
    };
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));
    settle(&runtime, first_card).await;

    let (third, third_outcome) = create_session(root, Direction::Send, 0x90);
    let third_card = third.record().identity.card;
    runtime.admit(third, third_outcome).unwrap();
    assert!(runtime.is_live(third_card));

    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detach_reattach_epoch_backpressure() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Send, 0x10);
    let card = session.record().identity.card;
    let transfer = session.record().identity.transfer;
    runtime.admit(session, outcome).unwrap();

    let mut first = runtime
        .subscribe(card, NonZeroUsize::new(3).unwrap())
        .unwrap();
    let first_epoch = first.epoch();
    let initial = first.recv().await.unwrap().unwrap();
    assert_eq!(initial.epoch, first_epoch);
    assert!(matches!(
        initial.kind,
        CardUpdateKind::Snapshot(record) if record.identity.card == card
    ));
    wait_for_state(&runtime, card, ProductState::Transferring).await;

    // A stalled frontend retains one latest progress projection, not the whole
    // burst, and the total queue remains bounded.
    for transferred in 1..=40 {
        executor
            .signal(
                transfer,
                ExecutorSignal::Event(AttemptEventKind::Progress {
                    transferred: ByteCount::new(transferred),
                }),
            )
            .await;
    }
    wait_for_bytes(&runtime, card, 40).await;
    assert!(first.pending_len() <= first.capacity().get());
    let latest = first.recv().await.unwrap().unwrap();
    assert!(matches!(
        latest.kind,
        CardUpdateKind::Progress(record) if record.bytes == ByteCount::new(40)
    ));
    assert_eq!(first.try_recv(), Err(TryRecvError::Empty));

    // Detach is just a drop. The actor continues to accept progress and mutate
    // only through its normal L3 reducer path.
    drop(first);
    executor
        .signal(
            transfer,
            ExecutorSignal::Event(AttemptEventKind::Progress {
                transferred: ByteCount::new(80),
            }),
        )
        .await;
    wait_for_bytes(&runtime, card, 80).await;
    assert!(runtime.is_live(card));

    // Reattach receives no predecessor backlog: it has a fresh epoch and one
    // current snapshot containing progress produced while detached.
    let mut second = runtime
        .subscribe(card, NonZeroUsize::new(3).unwrap())
        .unwrap();
    assert_ne!(second.epoch(), first_epoch);
    let current = second.recv().await.unwrap().unwrap();
    assert_eq!(current.epoch, second.epoch());
    assert!(matches!(
        current.kind,
        CardUpdateKind::Snapshot(record) if record.bytes == ByteCount::new(80)
    ));
    assert_eq!(second.try_recv(), Err(TryRecvError::Empty));

    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_events_and_duties_never_dropped() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Receive, 0x60);
    let card = session.record().identity.card;
    let transfer = session.record().identity.transfer;
    runtime.admit(session, outcome).unwrap();

    // Capacity three is one coalesced projection slot plus exactly two reserved
    // lossless slots: one terminal transition and one capability duty.
    let mut subscription = runtime
        .subscribe(card, NonZeroUsize::new(3).unwrap())
        .unwrap();
    let mut deliberately_lagged = runtime.subscribe(card, NonZeroUsize::MIN).unwrap();
    assert!(matches!(
        subscription.recv().await.unwrap().unwrap().kind,
        CardUpdateKind::Snapshot(_)
    ));
    assert!(matches!(
        deliberately_lagged.recv().await.unwrap().unwrap().kind,
        CardUpdateKind::Snapshot(_)
    ));
    wait_for_state(&runtime, card, ProductState::Transferring).await;

    for transferred in 1..=40 {
        executor
            .signal(
                transfer,
                ExecutorSignal::Event(AttemptEventKind::Progress {
                    transferred: ByteCount::new(transferred),
                }),
            )
            .await;
    }
    wait_for_bytes(&runtime, card, 40).await;
    assert_eq!(subscription.pending_len(), 1);

    executor
        .signal(transfer, ExecutorSignal::CommitCrossed)
        .await;
    executor
        .signal(
            transfer,
            ExecutorSignal::Event(AttemptEventKind::Terminal(OutcomeCode::Completed)),
        )
        .await;
    settle(&runtime, card).await;

    // Even though the replaceable lane was backpressured, both lossless updates
    // survive and the queue reaches, but never exceeds, its declared bound.
    assert_eq!(subscription.pending_len(), subscription.capacity().get());
    let mut terminal_count = 0;
    let mut duty_count = 0;
    while subscription.pending_len() != 0 {
        match subscription.try_recv().unwrap().kind {
            CardUpdateKind::Terminal(record) => {
                terminal_count += 1;
                assert_eq!(record.state, ProductState::Completed);
            }
            CardUpdateKind::CapabilityDuty { duty, .. } => {
                duty_count += 1;
                assert_eq!(duty.provenance.card, card);
            }
            CardUpdateKind::State(record) => {
                assert_eq!(record.state, ProductState::Completed);
                assert!(record.quiescence.is_quiescent());
            }
            CardUpdateKind::Snapshot(_) | CardUpdateKind::Progress(_) => {}
        }
    }
    assert_eq!(terminal_count, 1);
    assert_eq!(duty_count, 1);
    assert_eq!(subscription.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(
        deliberately_lagged.recv().await.unwrap_err().missed,
        LosslessUpdateKind::Terminal
    );

    // The actor is hibernated, but a fresh epoch still starts from current
    // terminal truth and replays each unique outstanding duty.
    let old_epoch = subscription.epoch();
    drop(subscription);
    let mut reattached = runtime
        .subscribe(card, NonZeroUsize::new(3).unwrap())
        .unwrap();
    assert_ne!(reattached.epoch(), old_epoch);
    assert!(matches!(
        reattached.recv().await.unwrap().unwrap().kind,
        CardUpdateKind::Snapshot(record) if record.state == ProductState::Completed
    ));
    assert!(matches!(
        reattached.recv().await.unwrap().unwrap().kind,
        CardUpdateKind::CapabilityDuty { duty, .. } if duty.provenance.card == card
    ));
    assert_eq!(reattached.try_recv(), Err(TryRecvError::Empty));

    runtime.shutdown().await;
}

/// A card still waiting for its source is still ASKING for one.
///
/// The source duty is discharged by the source becoming ready, not by having
/// been raised: until then every fresh attachment is re-told about it, which is
/// what makes a duty the service dropped — or one raised while nothing was
/// listening — recoverable rather than lost. A discharge rule that fired on
/// issue would make the ask a one-shot that no reattach can replay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outstanding_source_duty_is_replayed_to_every_attachment() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider { root: root.into() },
        ScriptedExecutor::new(Script::RunUntilStop),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );
    let (session, initial) = create_awaiting_session(root, 0x2b);
    let card = session.record().identity.card;
    assert_eq!(session.record().state, ProductState::Preparing);
    runtime.admit(session, initial).unwrap();

    for attachment in 0..3 {
        let mut subscription = runtime
            .subscribe(card, NonZeroUsize::new(4).unwrap())
            .unwrap();
        // Bounded on purpose: a duty that never arrives must FAIL this test,
        // not park it. An unbounded await turns a lost ask into a hang, which
        // is the least useful way a gate can report one.
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(5), subscription.recv())
                .await
                .expect("the attachment was told nothing within its bound")
                .unwrap()
                .unwrap()
        };
        // The ask travels in the SNAPSHOT, not as a duty. Every attachment is
        // told it in full — the lifecycle plus the exact acquisition an offer
        // must name — because the snapshot is current truth rather than a
        // replay of events this attachment missed.
        let CardUpdateKind::Snapshot(record) = next().await.kind else {
            panic!("attachment {attachment} did not open with a snapshot");
        };
        assert_eq!(record.state, ProductState::Preparing);
        let SourceLifecycle::AwaitingSelection(gate) = &record.source else {
            panic!("attachment {attachment} was not told the card needs a source");
        };
        assert!(gate.accepts_an_offer());
        let acquisition = record.current_acquisition();
        assert_eq!(acquisition.card(), card);
        assert_eq!(
            acquisition.generation(),
            durable_state(root, card).generation
        );
    }

    // The duty that IS raised — taking hold of a document once one has been
    // chosen — cannot be driven from here yet: the host refuses a source offer
    // on the command lane (`host.rs`), so nothing can move this card to
    // `Acquiring` through the runtime. That commissioning is covered at the
    // product level by
    // `creating_a_card_that_needs_a_document_commissions_no_platform_work`.

    // The other direction — a card whose source IS ready — raises no such duty
    // in the first place, so an attachment to one is never told to pick a file.
    let (ready, initial) = create_session(root, Direction::Send, 0x2c);
    let ready_card = ready.record().identity.card;
    runtime.admit(ready, initial).unwrap();
    let mut watching = runtime
        .subscribe(ready_card, NonZeroUsize::new(4).unwrap())
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), watching.recv())
            .await
            .expect("the attachment opens with its snapshot")
            .unwrap()
            .unwrap()
            .kind,
        CardUpdateKind::Snapshot(record) if record.state == ProductState::Connecting
    ));
    assert_eq!(watching.try_recv(), Err(TryRecvError::Empty));

    runtime.shutdown().await;
}

// ---- F-A: a removed/tombstoned card evicts its projection (no leak) ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_card_evicts_projection() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Send, 0x10);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    // A hibernating card keeps its projection (renders at rest); a REMOVED card is
    // tombstoned and gone, so its projection must not leak for the process life.
    let commander = runtime
        .subscribe(card, NonZeroUsize::new(4).unwrap())
        .unwrap();
    runtime
        .submit_command(
            &commander,
            CommandId::from_bytes([0xC2; 16]),
            ProductCommand::Remove,
        )
        .await
        .unwrap();
    drop(commander);
    settle(&runtime, card).await;
    assert!(durable_state(root, card).facts.remove_requested);
    assert!(matches!(
        runtime.subscribe(card, NonZeroUsize::new(2).unwrap()),
        Err(SubscribeError::UnknownCard)
    ));

    runtime.shutdown().await;
}

// ---- F-B: a re-issued duty supersedes; the outstanding set stays bounded ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reissued_duty_supersedes_not_accumulates() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = ScriptedExecutor::new(Script::RunUntilStop);
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Receive, 0x60);
    let card = session.record().identity.card;
    let transfer = session.record().identity.transfer;
    executor.set(transfer, Script::Complete);
    runtime.admit(session, outcome).unwrap();
    // A completed receiver leaves exactly one outstanding PostReceipt duty.
    settle(&runtime, card).await;

    // Restoring a completed receiver (proof not yet delivered) RE-EMITS the receipt
    // duty with a fresh request id. Supersede-by-action must keep exactly ONE
    // outstanding duty, not accumulate a second.
    runtime.restore(card).unwrap();
    settle(&runtime, card).await;

    let mut sub = runtime
        .subscribe(card, NonZeroUsize::new(4).unwrap())
        .unwrap();
    let mut duty_count = 0;
    loop {
        match sub.try_recv() {
            Ok(update) => {
                if matches!(update.kind, CardUpdateKind::CapabilityDuty { .. }) {
                    duty_count += 1;
                }
            }
            Err(TryRecvError::Empty) => break,
            Err(other) => panic!("unexpected receive error: {other:?}"),
        }
    }
    assert_eq!(duty_count, 1);

    runtime.shutdown().await;
}

// ---- capturing executor: records the signal forwarded down the stop channel ----

#[derive(Clone)]
struct CapturingExecutor {
    signal: Arc<Mutex<Option<StopSignal>>>,
}

impl CapturingExecutor {
    fn new() -> Self {
        Self {
            signal: Arc::new(Mutex::new(None)),
        }
    }

    async fn captured(&self) -> StopSignal {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(signal) = *self.signal.lock().unwrap() {
                    break signal;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the executor should receive a stop signal")
    }
}

impl AttemptExecutor for CapturingExecutor {
    fn start(&self, _launch: AttemptLaunch) -> AttemptExecution {
        let (signal_tx, signals) = mpsc::channel(4);
        let (stop, token) = stop_channel();
        let captured = Arc::clone(&self.signal);
        tokio::spawn(async move {
            // Reach an active, pausable state so the reducer retires the live worker.
            let _ = signal_tx
                .send(ExecutorSignal::Event(AttemptEventKind::Phase(
                    Phase::Transferring,
                )))
                .await;
            let signal = token.stopped().await;
            *captured.lock().unwrap() = Some(signal);
            let _ = signal_tx.send(ExecutorSignal::Stopped).await;
        });
        AttemptExecution { signals, stop }
    }
}

/// The reducer-authorized retirement intent reaches the executor's stop token
/// unchanged: the runtime forwards Pause vs Cancel through the L4 stop channel
/// so a real executor can tell a resume-preserving stop from a discarding one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_intent_reaches_the_executor() {
    for (command, expected) in [
        (ProductCommand::Pause, RetirementIntent::Pause),
        (ProductCommand::Cancel, RetirementIntent::Cancel),
    ] {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let executor = CapturingExecutor::new();
        let runtime = Runtime::start(
            config(8),
            OpStoreProvider {
                root: root.to_path_buf(),
            },
            executor.clone(),
            PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
        );

        let (session, outcome) = create_session(root, Direction::Send, 0x10);
        let card = session.record().identity.card;
        runtime.admit(session, outcome).unwrap();
        // Pause/Cancel only retire a live attempt from an active state.
        wait_for_state(&runtime, card, ProductState::Transferring).await;

        let commander = runtime
            .subscribe(card, NonZeroUsize::new(4).unwrap())
            .unwrap();
        runtime
            .submit_command(&commander, CommandId::from_bytes([0xA1; 16]), command)
            .await
            .unwrap();

        assert_eq!(executor.captured().await, StopSignal::Retire(expected));
        runtime.shutdown().await;
    }
}

/// Tearing the process down is NOT a transfer cancellation. Shutdown drops the
/// card's stop handle without any reducer-authorized retirement, and the
/// executor must see that difference — a `Detached` teardown preserves
/// resumable state where a `Cancel` would tell the transport to discard it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teardown_reaches_the_executor_as_detached_not_cancel() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let executor = CapturingExecutor::new();
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        executor.clone(),
        PlatformPorts::new(NoSourceStaging, MemorySources, MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Send, 0x11);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    wait_for_state(&runtime, card, ProductState::Transferring).await;

    runtime.shutdown().await;
    assert_eq!(executor.captured().await, StopSignal::Detached);
}

/// Refuses every source, and remembers what it was asked for.
#[derive(Clone, Default)]
struct AbsentSources(Arc<std::sync::Mutex<Vec<SourceLocator>>>);

impl PreparedSourceResolver for AbsentSources {
    fn resolve(
        &self,
        locator: SourceLocator,
        _identity: StagedIdentity,
    ) -> Result<PreparedSource, SourceResolveError> {
        self.0.lock().unwrap().push(locator);
        Err(SourceResolveError::Absent)
    }
}

/// A resolver that always opens an empty in-memory source.
///
/// These suites drive the LIFECYCLE, not the bytes: what they need is for a send
/// to be able to start. Answering `Unsupported` would fail every send before it
/// began, which is correct behaviour and the wrong thing to be testing here.
#[derive(Clone, Copy, Debug, Default)]
struct MemorySources;

struct EmptySource;

impl envoix_capabilities::SourceSession for EmptySource {
    fn read_at(
        &mut self,
        _offset: envoix_types::ByteCount,
        _destination: &mut [u8],
    ) -> Result<usize, envoix_capabilities::SourceReadError> {
        Ok(0)
    }
}

impl PreparedSourceResolver for MemorySources {
    fn resolve(
        &self,
        _locator: SourceLocator,
        identity: StagedIdentity,
    ) -> Result<PreparedSource, SourceResolveError> {
        Ok(PreparedSource::new(Box::new(EmptySource), identity))
    }
}

/// A send whose source this process cannot open does not start a transport, and
/// does not park pretending to connect.
///
/// It asks for the source NAMED BY ITS OWN COMMITTED RECORD — the assertion on
/// the locator is what proves the card derived it from `Ready` rather than from
/// anything the executor could have supplied — and when that source is gone it
/// fails the one way the person can act on: the lifecycle leaves `Ready`, so the
/// `RePickSource` the outcome recommends is a command the card will accept.
///
/// Opening a link first would spend a connection to discover on the first read
/// that there are no bytes behind it.
#[tokio::test]
async fn a_send_that_cannot_open_its_source_fails_instead_of_connecting() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let sources = AbsentSources::default();
    let runtime = Runtime::start(
        config(8),
        OpStoreProvider {
            root: root.to_path_buf(),
        },
        // The executor must never be reached: nothing should start.
        CapturingExecutor::new(),
        PlatformPorts::new(NoSourceStaging, sources.clone(), MemorySinks),
    );

    let (session, outcome) = create_session(root, Direction::Send, 0x70);
    let card = session.record().identity.card;
    let expected = *session
        .record()
        .source
        .key()
        .expect("a staged sender names its acquisition");
    runtime.admit(session, outcome).unwrap();

    wait_for_state(&runtime, card, ProductState::Failed).await;
    runtime.shutdown().await;

    assert_eq!(
        sources.0.lock().unwrap().as_slice(),
        [SourceLocator::PersistedProvider {
            acquisition: expected
        }],
        "the card asked for a source other than the one its record established"
    );
    let record = durable_state(root, card);
    assert!(
        !record.source.is_ready(),
        "a source that could not be opened is still ready: {:?}",
        record.source
    );
    assert_eq!(
        record.outcome.as_ref().and_then(|outcome| outcome.recovery),
        Some(envoix_outcomes::Recovery::RePickSource)
    );
}

/// A resolver that always opens an empty in-memory destination.
///
/// The same reasoning as `MemorySources`, for the other direction: these suites
/// drive the LIFECYCLE, and a receiving card is the one that reaches a live
/// attempt at creation. Answering `Unsupported` would fail every receive before
/// it began — correct behaviour for a build with no bulk storage, and the wrong
/// thing to be testing here.
#[derive(Clone, Copy, Debug, Default)]
struct MemorySinks;

impl PreparedSinkResolver for MemorySinks {
    fn open(&self, _blob: envoix_blob_api::BlobKey) -> Result<PreparedReceiveSink, SinkOpenError> {
        Ok(PreparedReceiveSink::new(Box::new(EmptySink)))
    }
}

/// Accepts nothing, and CANNOT seal.
///
/// Not an omission: `SealedArtifact` has no public constructor, so only the real
/// store can mint one. A double that could would let a test prove a card owns
/// bytes that were never written — which is exactly the property the witness
/// exists to hold. Nothing in these suites writes, so the gap is honest.
struct EmptySink;

impl envoix_blob_api::SinkSession for EmptySink {
    fn resume(&mut self) -> Result<envoix_types::DurablePrefix, envoix_blob_api::BlobError> {
        unimplemented!("no lifecycle case here writes bytes")
    }

    fn read_partial_at(
        &mut self,
        _offset: envoix_types::ByteCount,
        _destination: &mut [u8],
    ) -> Result<usize, envoix_blob_api::BlobError> {
        unimplemented!("no lifecycle case here writes bytes")
    }

    fn append(
        &mut self,
        _offset: envoix_types::ByteCount,
        _bytes: &[u8],
    ) -> Result<(), envoix_blob_api::BlobError> {
        unimplemented!("no lifecycle case here writes bytes")
    }

    fn checkpoint(
        &mut self,
        _prefix: envoix_types::DurablePrefix,
    ) -> Result<(), envoix_blob_api::BlobError> {
        unimplemented!("no lifecycle case here writes bytes")
    }

    fn reset(&mut self) -> Result<(), envoix_blob_api::BlobError> {
        unimplemented!("no lifecycle case here writes bytes")
    }

    fn seal(
        self: Box<Self>,
        _expected_size: envoix_types::ByteCount,
        _digest: envoix_types::ContentHash,
    ) -> Result<envoix_blob_api::SealedArtifact, envoix_blob_api::BlobError> {
        unimplemented!("only the real store can mint a witness")
    }
}
