use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use envoix_bindings::bridge::{
    SubmitDecodeError, acceptance_frame, completion_frame, decode_submit,
};
use envoix_bindings::command::encode_command_frame;
use envoix_bindings::read::encode_read_frame;
use envoix_bindings::{card_update_frame, lag_frame};
use envoix_capabilities::{Admission, DutyLedger, Registration};
use envoix_platform_android::{DutyAdapter, IssueDecision, WorkOrder, WorkReport, platform_work};
use envoix_product::{
    ApplyOutcome, CommittedSession, IdentityError, NewTransfer, SystemIdentitySource,
};
use envoix_runtime::{
    CardSubscription, CardUpdateKind, CommandRejected, CommandVerdict, Runtime, RuntimeConfig,
    TransferRecord, TryRecvError,
};
use envoix_storage_local::LocalStorage;
use envoix_types::{ByteCount, Direction, OfferedName, RecordId};

use crate::executor::PreparedIrohExecutor;
use crate::provider::HostProvider;
use crate::store::HostStore;
use crate::stores::CardStores;

/// How often the frame pump polls its subscriptions. A host lane, not a UI
/// animation clock: latency here only delays observer refresh.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Why the host could not boot.
#[derive(Debug)]
pub enum BootError {
    Storage,
    Runtime,
}

/// The Android composition root: one process-wide runtime owner.
///
/// The Kotlin foreground service constructs exactly one `Host` per process
/// and drives it over the JNI lane: contract frames out (`poll_frame`),
/// platform work orders out (`poll_work`), command submissions in
/// (`submit`), duty reports in (`report_duty`).
pub struct Host {
    tokio: tokio::runtime::Runtime,
    runtime: Arc<Runtime<HostProvider, PreparedIrohExecutor>>,
    stores: CardStores,
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<SharedState>,
}

#[derive(Default)]
struct SharedState {
    subscriptions: HashMap<RecordId, CardSubscription>,
    ledger: DutyLedger,
    adapter: DutyAdapter,
    /// Encoded read/command contract frames awaiting the frontend lane.
    frames: VecDeque<Vec<u8>>,
    /// Encoded platform work orders awaiting the service executor.
    work: VecDeque<Vec<u8>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, SharedState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Bounded frame/work backlogs. An observer that never drains loses OLDEST
/// coalescible truth first; durable state is never affected (Pillar 7).
const MAX_QUEUED: usize = 1024;

fn push_bounded(queue: &mut VecDeque<Vec<u8>>, bytes: Vec<u8>) {
    if queue.len() == MAX_QUEUED {
        queue.pop_front();
    }
    queue.push_back(bytes);
}

impl Host {
    /// Boots the process-wide host: starts the runtime, restores every
    /// durable card, then drains each card's destructive outbox (AFTER
    /// restore — never inside it), then attaches the frame pump.
    pub fn boot(root: &Path) -> Result<Self, BootError> {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| BootError::Runtime)?;
        let config = RuntimeConfig::new(
            NonZeroUsize::new(16).expect("nonzero"),
            Duration::from_secs(5),
            NonZeroUsize::new(3).expect("nonzero"),
        );
        let stores = CardStores::new(root.to_path_buf());
        let provider = HostProvider::new(stores.clone(), NonZeroUsize::new(3).expect("nonzero"));
        let runtime = {
            let _guard = tokio.enter();
            Arc::new(Runtime::start(
                config,
                provider,
                PreparedIrohExecutor::default(),
            ))
        };
        let host = Self {
            tokio,
            runtime,
            stores,
            shared: Arc::new(Shared {
                state: Mutex::new(SharedState::default()),
            }),
        };

        // Enumeration only: this backend handle lists the durable cards and is
        // dropped immediately. Every WRITER goes through `CardStores`.
        let cards = match LocalStorage::open(host.stores.root()) {
            Ok(storage) => storage.cards().map_err(|_| BootError::Storage)?,
            // A first boot has no storage root yet; nothing to restore.
            Err(_) => Vec::new(),
        };
        for card in cards {
            // Absent/corrupt cards stay quarantined in storage; a restore
            // refusal must not fail the boot of every other card.
            let _ = host.runtime.restore(card);
            host.drain_outbox(card);
            host.attach(card);
        }
        host.spawn_pump();
        Ok(host)
    }

    /// At-least-once replay of the card's pending destructive operations,
    /// through the card's ONE live store (the restored actor already holds it,
    /// so the two writers serialize). `settle_destructive` is idempotent,
    /// refuses to lose the last good copy, and executes+confirms in a single
    /// durable image, so a crash mid-drain replays harmlessly on the next boot.
    fn drain_outbox(&self, card: RecordId) {
        let Some(operation) = self.stores.open(card) else {
            return;
        };
        let mut operation = operation.lock().unwrap_or_else(PoisonError::into_inner);
        for pending in operation.replayable_outbox() {
            let _ = operation.settle_destructive(pending);
        }
    }

    /// Opens (or refreshes) the host's own observer subscription for `card`.
    /// The newest attachment is the card's commander (BN2), so this is also
    /// the epoch under which frontend submissions are authorized.
    pub fn attach(&self, card: RecordId) {
        let capacity = NonZeroUsize::new(64).expect("nonzero");
        if let Ok(subscription) = self.runtime.subscribe(card, capacity) {
            self.shared.lock().subscriptions.insert(card, subscription);
        }
    }

    fn spawn_pump(&self) {
        let shared = Arc::clone(&self.shared);
        self.tokio.spawn(async move {
            loop {
                tokio::time::sleep(PUMP_INTERVAL).await;
                pump_once(&shared);
            }
        });
    }

    /// One encoded contract frame for the frontend lane, if any is queued.
    pub fn poll_frame(&self) -> Option<Vec<u8>> {
        self.shared.lock().frames.pop_front()
    }

    /// One encoded platform work order for the service executor, if any.
    pub fn poll_work(&self) -> Option<Vec<u8>> {
        self.shared.lock().work.pop_front()
    }

    /// Submits one frontend command frame. Returns the encoded acceptance
    /// frame; for accepted commands the committed completion follows on the
    /// frame lane (acceptance is never proof of effect — Pillar 3).
    pub fn submit(&self, bytes: &[u8]) -> Vec<u8> {
        let spec = match decode_submit(bytes) {
            Ok(spec) => spec,
            Err(SubmitDecodeError::Frame(_) | SubmitDecodeError::NotASubmit) => {
                // Hostile or non-submit bytes carry no usable command id; the
                // caller violated the contract and gets nothing correlatable.
                return Vec::new();
            }
        };
        let shared = Arc::clone(&self.shared);
        let runtime = Arc::clone(&self.runtime);
        self.tokio.block_on(async move {
            // Take the subscription out for the await so no lock is held
            // across it; the pump simply skips the card for one tick.
            let taken = shared.lock().subscriptions.remove(&spec.card);
            let acceptance = match taken {
                Some(subscription) if subscription.epoch().get() == spec.epoch => {
                    let acceptance = runtime
                        .submit_command(&subscription, spec.command_id, spec.command)
                        .await;
                    shared.lock().subscriptions.insert(spec.card, subscription);
                    acceptance
                }
                Some(subscription) => {
                    shared.lock().subscriptions.insert(spec.card, subscription);
                    Err(CommandRejected::StaleEpoch)
                }
                None => Err(CommandRejected::UnknownCard),
            };
            let frame = acceptance_frame(spec.command_id, &acceptance);
            if let Ok(CommandVerdict::Accepted(ticket)) = acceptance {
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let completion = ticket.completed().await;
                    let frame = completion_frame(spec.command_id, completion);
                    if let Ok(bytes) = encode_command_frame(&frame) {
                        push_bounded(&mut shared.lock().frames, bytes);
                    }
                });
            }
            encode_command_frame(&frame).unwrap_or_default()
        })
    }

    /// Admits one platform work report through the C6 ledger (exactly-once)
    /// and settles the adapter's dispatch state. Returns whether the report
    /// was admitted fresh.
    pub fn report_duty(&self, bytes: &[u8]) -> bool {
        let Ok(report) = WorkReport::decode(bytes) else {
            return false;
        };
        let result = report.to_result();
        let mut state = self.shared.lock();
        match state.ledger.admit(result) {
            Admission::Fresh(admitted) => {
                state.adapter.settle(admitted.duty().provenance);
                // Feeding the admitted result back into the product reducer
                // (ProductInput::ReceiptPosted) is the F2 mutating-frontend
                // slice: the runtime exposes no duty-result intake yet.
                true
            }
            _ => false,
        }
    }

    /// Debug/e2e-only: creates one durable card so packaged process-death
    /// instrumentation has real state to kill and restore. Never part of the
    /// frontend contract (creation arrives with the F-phase flows).
    pub fn create_for_e2e(&self, name: &str, total: u64) -> Result<RecordId, IdentityError> {
        let transfer = NewTransfer {
            direction: Direction::Send,
            offered_name: OfferedName::from_untrusted(name),
            total: ByteCount::new(total),
            source: envoix_product::SourceDecision::Ready,
        };
        let store = HostStore::deferred(self.stores.clone());
        let (session, initial): (CommittedSession<HostStore>, ApplyOutcome) =
            CommittedSession::create(
                transfer,
                &mut SystemIdentitySource,
                store,
                NonZeroUsize::new(3).expect("nonzero"),
            )?;
        let card = session.record().identity.card;
        let _ = self.runtime.admit(session, initial);
        self.attach(card);
        Ok(card)
    }

    /// The cards this process generation has live attachments for — the
    /// restored truth itself, not a file count. Debug/e2e instrumentation
    /// reads it to prove a card came back from durable storage.
    pub fn live_cards(&self) -> Vec<RecordId> {
        self.shared.lock().subscriptions.keys().copied().collect()
    }

    /// Stops the runtime; durable truth is on disk, the process may die.
    pub fn shutdown(self) {
        let runtime = Arc::clone(&self.runtime);
        self.tokio.block_on(async move {
            runtime.shutdown().await;
        });
    }
}

/// Drains every subscription once: contract frames to the frame lane, duty
/// updates through the adapter to the work lane.
fn pump_once(shared: &Shared) {
    let mut state = shared.lock();
    let SharedState {
        subscriptions,
        ledger,
        adapter,
        frames,
        work,
    } = &mut *state;
    for (card, subscription) in subscriptions.iter_mut() {
        loop {
            match subscription.try_recv() {
                Ok(update) => {
                    if let CardUpdateKind::CapabilityDuty { duty, action: _ } = &update.kind {
                        // Registration is the ledger's authority check: only a
                        // duty of the card's CURRENT generation, not already
                        // outstanding or discharged, may reach the service.
                        if ledger.register(*duty) != Registration::Registered {
                            continue;
                        }
                        if let Some(item) = platform_work(*duty)
                            && let Ok(order) = WorkOrder::for_duty(*duty, item)
                            && let IssueDecision::Dispatch(order) = adapter.issue(order)
                            && let Ok(bytes) = order.encode()
                        {
                            push_bounded(work, bytes);
                        }
                        continue;
                    }
                    // The card's committed record is the ledger's generation
                    // authority. Every epoch opens with the snapshot, so the
                    // generation is established before any duty can register.
                    if let Some(record) = observed_record(&update.kind) {
                        ledger.advance_generation(update.card, record.generation);
                    }
                    let frame = card_update_frame(update.epoch.get(), update.card, &update.kind);
                    if let Ok(bytes) = encode_read_frame(&frame) {
                        push_bounded(frames, bytes);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(lag)) => {
                    let frame = lag_frame(lag.epoch.get(), *card, lag.missed);
                    if let Ok(bytes) = encode_read_frame(&frame) {
                        push_bounded(frames, bytes);
                    }
                    break;
                }
            }
        }
    }
}

/// The committed record an update carries, if any.
const fn observed_record(kind: &CardUpdateKind) -> Option<&TransferRecord> {
    match kind {
        CardUpdateKind::Snapshot(record)
        | CardUpdateKind::Progress(record)
        | CardUpdateKind::State(record)
        | CardUpdateKind::Terminal(record) => Some(record),
        CardUpdateKind::CapabilityDuty { .. } => None,
    }
}
