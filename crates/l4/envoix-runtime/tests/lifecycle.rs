//! RT1 lifecycle proof over the real operation store + supervisor.
//!
//! The executor is scripted (a controllable signal producer), but the runtime's
//! `AttemptSupervisor` is real, so every `RetirementAck` fed to the reducer is a
//! genuine, non-forgeable token.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use envoix_attempt_api::{AttemptEventKind, AttemptPlan};
use envoix_operation_store::OperationStore;
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_product::{
    CommitError, CommittedSession, IdentityError, IdentitySource, NewTransfer, ProductCommand,
    ProductState, Quiescence, RecordDecode, RecordStore, SourceDecision, TransferRecord,
    decode_record,
};
use envoix_runtime::{
    AcquireError, AttemptExecution, AttemptExecutor, ExecutorSignal, Runtime, RuntimeConfig,
    SessionProvider, ShutdownReport, stop_channel,
};
use envoix_storage_api::Durability;
use envoix_storage_local::LocalStorage;
use envoix_types::{ByteCount, Direction, OfferedName, RecordId, TransferId};
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
            record,
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
    default: Script,
}

impl ScriptedExecutor {
    fn new(default: Script) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(HashMap::new())),
            default,
        }
    }

    fn set(&self, transfer: TransferId, script: Script) {
        self.scripts.lock().unwrap().insert(transfer, script);
    }
}

impl AttemptExecutor for ScriptedExecutor {
    fn start(&self, plan: AttemptPlan) -> AttemptExecution {
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

fn create_session(
    root: &Path,
    direction: Direction,
    seed: u8,
) -> (Session, envoix_product::ApplyOutcome) {
    let transfer = NewTransfer {
        direction,
        offered_name: OfferedName::from_untrusted("payload.bin"),
        total: ByteCount::new(1024),
        source: SourceDecision::Ready,
    };
    CommittedSession::create(
        transfer,
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
        RecordDecode::Loaded(record) => record,
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
    runtime
        .command(first_card, ProductCommand::Cancel)
        .await
        .unwrap();
    settle(&runtime, first_card).await;

    let (third, third_outcome) = create_session(root, Direction::Send, 0x90);
    let third_card = third.record().identity.card;
    runtime.admit(third, third_outcome).unwrap();
    assert!(runtime.is_live(third_card));

    runtime.shutdown().await;
}
