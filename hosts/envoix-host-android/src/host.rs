use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use envoix_bindings::bridge::{
    CreateSpec, FrontendIntent, SubmitDecodeError, SubmitSpec, acceptance_frame, completion_frame,
    create_result_frame, decode_intent,
};
use envoix_bindings::command::{
    CardCreatedView, CreateOutcomeView, CreateRefusalView, encode_command_frame,
};
use envoix_bindings::read::{ReadError, ReadFrame, encode_read_frame};
use envoix_bindings::{
    card_update_frame, closed_frame, evidence_frame, lag_frame, subscribe_rejected_frame,
};
use envoix_capabilities::{Admission, DutyLedger, Registration};
use envoix_evidence::{EvidenceRecord, EvidenceSink, EvidenceSinkError, SessionKey, TimelineStore};
use envoix_platform_android::{DutyAdapter, IssueDecision, WorkOrder, WorkReport, platform_work};
#[cfg(feature = "e2e-instrumentation")]
use envoix_product::ProductState;
use envoix_product::{
    ApplyOutcome, CommitStatus, CommittedSession, IdentityError, NewTransfer, RecordDecode,
    SystemIdentitySource, decode_record,
};
use envoix_runtime::{
    CardSubscription, CardUpdateKind, CommandRejected, CommandVerdict, Runtime, RuntimeConfig,
    SubscribeError, TransferRecord, TryRecvError,
};
use envoix_storage_local::LocalStorage;
use envoix_types::{CommandId, Direction, RecordId};

use crate::create;
use crate::executor::PreparedIrohExecutor;
use crate::provider::HostProvider;
use crate::store::HostStore;
use crate::stores::CardStores;

/// How often the frame pump polls its subscriptions. A host lane, not a UI
/// animation clock: latency here only delays observer refresh.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// How much diagnostics one process keeps. Bounded on both axes, and overflow
/// is not silence: the timeline says `degraded` and counts what it dropped, so
/// an observer can never mistake a trimmed timeline for a complete one.
const EVIDENCE_ENTRIES_PER_SESSION: usize = 64;
const EVIDENCE_SESSIONS: usize = 16;

/// Why the host could not boot.
#[derive(Debug)]
pub enum BootError {
    Storage,
    Runtime,
}

/// Identifies ONE frontend attachment.
///
/// [`Host::open_lane`] mints a fresh token and supersedes every earlier one in
/// the same instant. The frame queue is destructive, so a consumer that cannot
/// be told apart from its successor is a consumer that can eat the snapshot the
/// successor is waiting for; carrying the token on every poll makes that
/// unrepresentable rather than a matter of which thread wakes first.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AttachmentToken(u64);

impl AttachmentToken {
    /// The token no attachment ever holds: what the lane carries before it has
    /// attached, and what an unrecognised value from the wire becomes.
    pub const NONE: Self = Self(0);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// What one poll of the frame lane found.
///
/// `Superseded` is a REFUSAL, not an emptiness: nothing was consumed, and the
/// caller holding that token will never consume anything again. The frontend
/// needs to be able to tell the two apart, or a replaced pump spins forever.
#[derive(Debug, Eq, PartialEq)]
pub enum FramePoll {
    /// One encoded contract frame, addressed to the attachment that asked.
    Frame(Vec<u8>),
    /// The attachment is current and nothing is queued right now.
    Drained,
    /// A newer attachment holds the lane; this token consumes nothing.
    Superseded,
}

/// Why the authority refused an intent before it could produce a correlated
/// command-contract answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentRejection {
    /// The bytes were not a valid frontend-originated intent. No command or
    /// create reached its authority handler, so no durable effect can exist.
    Contract,
}

/// The Android composition root: one process-wide runtime owner.
///
/// The Kotlin foreground service constructs exactly one `Host` per process
/// and drives it over the JNI lane: contract frames out (`poll_frame`),
/// platform work orders out (`poll_work`), frontend intents in
/// (`intent`), duty reports in (`report_duty`).
pub struct Host {
    tokio: tokio::runtime::Runtime,
    runtime: Arc<Runtime<HostProvider, PreparedIrohExecutor>>,
    stores: CardStores,
    /// Process index of frontend create identities, on TWO keys, because one
    /// key cannot answer both retries.
    ///
    /// The request id answers a retry within one process: the frontend forms an
    /// id with the intent and reuses it, so a resend of an unanswered ask is
    /// recognisably the same ask.
    ///
    /// The rendezvous ENDPOINT answers the retry it cannot. The frontend keeps
    /// nothing durable, so after a process death the user re-pastes the invite
    /// and mints a FRESH id — and only the rendezvous says the two asks are the
    /// same thing. That is the reported defect: a lost answer, a restart, and a
    /// retry produced two cards frozen to one room.
    ///
    /// The endpoint is `(room, direction)` and not the room, because a room has
    /// two ends and one device may legitimately hold both: joining an invite it
    /// published itself is a real case, and keying on the room alone answers
    /// that join with the SENDING card. Two joins of one invite are one card; a
    /// send and a join of one room are two. A send mints a fresh room each
    /// time, so this key never merges two sends.
    ///
    /// The mutex serializes the check+create boundary so two concurrent copies
    /// of one request cannot both observe absence.
    creates: Mutex<CreateIndex>,
    shared: Arc<Shared>,
}

/// The two keys a create is remembered by. See `Host::creates`.
#[derive(Default)]
struct CreateIndex {
    by_request: HashMap<CommandId, CreateOutcomeView>,
    by_endpoint: HashMap<(String, Direction), CreateOutcomeView>,
}

impl CreateIndex {
    /// The answer an identical create already produced, if one did.
    fn existing(
        &self,
        request: CommandId,
        endpoint: Option<&(String, Direction)>,
    ) -> Option<CreateOutcomeView> {
        self.by_request
            .get(&request)
            .or_else(|| self.by_endpoint.get(endpoint?))
            .cloned()
    }

    /// Remembers one durable card. First-writer-wins, so replaying the boot
    /// scan or re-observing a card never moves a key onto a younger card.
    fn remember(
        &mut self,
        request: CommandId,
        endpoint: Option<(String, Direction)>,
        outcome: CreateOutcomeView,
    ) {
        self.by_request.entry(request).or_insert(outcome.clone());
        if let Some(endpoint) = endpoint {
            self.by_endpoint.entry(endpoint).or_insert(outcome);
        }
    }
}

struct Shared {
    state: Mutex<SharedState>,
    evidence: Arc<Evidence>,
}

/// The host's diagnostics projection (RT3) and the sessions the current
/// attachment has not been told about yet.
///
/// Evidence is downstream truth — it never re-enters the reducer — and it
/// reaches the frontend the way card updates do: coalesced by the pump, one
/// frame per changed session rather than one per record.
///
/// No thread ever holds the store lock and this one at the same time. The
/// runtime's evidence worker takes the store's, then this one; both readers
/// below take this one, let it go, and only then ask the store.
struct Evidence {
    store: TimelineStore,
    unpublished: Mutex<HashSet<SessionKey>>,
}

impl Evidence {
    fn new() -> Self {
        Self {
            store: TimelineStore::new(
                NonZeroUsize::new(EVIDENCE_ENTRIES_PER_SESSION).expect("nonzero"),
                NonZeroUsize::new(EVIDENCE_SESSIONS).expect("nonzero"),
            ),
            unpublished: Mutex::new(HashSet::new()),
        }
    }

    fn unpublished(&self) -> MutexGuard<'_, HashSet<SessionKey>> {
        self.unpublished
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks every retained session for publication. A fresh attachment has
    /// been told nothing, and the timelines it needs were recorded long before
    /// it opened.
    fn reseed(&self) {
        let sessions = self.store.sessions();
        self.unpublished().extend(sessions);
    }

    /// The sessions to publish now, in a stable order.
    fn take_unpublished(&self) -> Vec<SessionKey> {
        let mut sessions: Vec<SessionKey> = std::mem::take(&mut *self.unpublished())
            .into_iter()
            .collect();
        sessions.sort_unstable_by_key(|session| (session.card.get(), session.generation.get()));
        sessions
    }
}

/// The runtime's write-only end of the evidence lane.
struct EvidenceIntake(Arc<Evidence>);

impl EvidenceSink for EvidenceIntake {
    fn record(&self, record: EvidenceRecord) -> Result<(), EvidenceSinkError> {
        let session = record.session();
        self.0.store.record(record)?;
        self.0.unpublished().insert(session);
        Ok(())
    }

    fn evict_card(&self, card: RecordId) -> Result<(), EvidenceSinkError> {
        self.0.store.evict_card(card)?;
        self.0.unpublished().retain(|session| session.card != card);
        Ok(())
    }
}

#[derive(Default)]
struct SharedState {
    /// The attachment that currently owns the frame lane. It starts at
    /// [`AttachmentToken::NONE`], so a frontend that polls without attaching is
    /// superseded from the outset.
    attachment: AttachmentToken,
    /// Every card this host has admitted or restored. An attach that the
    /// runtime refused is still a card the host knows about, so the next
    /// attachment retries it instead of losing it silently.
    known: BTreeSet<RecordId>,
    subscriptions: HashMap<RecordId, CardSubscription>,
    ledger: DutyLedger,
    adapter: DutyAdapter,
    /// Encoded read/command contract frames awaiting the frontend lane.
    frames: VecDeque<Vec<u8>>,
    /// Encoded platform work orders awaiting the service executor.
    work: VecDeque<Vec<u8>>,
    /// Cards whose durable removal ended the platform capability they owned.
    /// The service consumes this lane and releases the matching persistable
    /// document grant; boot reseeds it from a removal record after a crash.
    source_releases: VecDeque<RecordId>,
    /// One release delivery per card per process. A process death deliberately
    /// forgets this set, because replay is how the pop-before-release crash
    /// window closes.
    source_releases_seen: HashSet<RecordId>,
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

/// Projects one read frame onto the lane without erasing a typed codec
/// refusal. Every producer passed here is an in-process bounded projection; an
/// encoder error is therefore a broken host/contract invariant, not hostile
/// frontend input. Panicking makes that defect loud at its origin instead of
/// letting the observer wait forever for a frame the host silently discarded.
fn push_read_frame(queue: &mut VecDeque<Vec<u8>>, frame: &ReadFrame) {
    push_encoded_read(queue, encode_read_frame(frame));
}

fn push_encoded_read(queue: &mut VecDeque<Vec<u8>>, encoded: Result<Vec<u8>, ReadError>) {
    let bytes = encoded.unwrap_or_else(|error| {
        panic!("read projection rejected by typed codec: {error:?}");
    });
    push_bounded(queue, bytes);
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
        let evidence = Arc::new(Evidence::new());
        let runtime = {
            let _guard = tokio.enter();
            Arc::new(Runtime::start_with_evidence(
                config,
                provider,
                PreparedIrohExecutor::default(),
                EvidenceIntake(Arc::clone(&evidence)),
            ))
        };
        let host = Self {
            tokio,
            runtime,
            stores,
            creates: Mutex::new(CreateIndex::default()),
            shared: Arc::new(Shared {
                state: Mutex::new(SharedState::default()),
                evidence,
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
            if let Some((request_id, endpoint, outcome)) = host.create_receipt(card) {
                host.creates
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remember(request_id, endpoint, outcome);
            }
            if host
                .durable_record(card)
                .is_some_and(|record| record.facts.remove_requested)
            {
                enqueue_source_release(&mut host.shared.lock(), card);
            }
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
    ///
    /// A refusal is typed truth, not silence: the reason crosses to the
    /// frontend as a `subscribe_rejected` frame and the card stays known, so
    /// the next attachment tries again.
    pub fn attach(&self, card: RecordId) {
        let subscription = self.subscribe(card);
        install(&mut self.shared.lock(), card, subscription);
    }

    fn subscribe(&self, card: RecordId) -> Result<CardSubscription, SubscribeError> {
        let capacity = NonZeroUsize::new(64).expect("nonzero");
        self.runtime.subscribe(card, capacity)
    }

    /// Opens a fresh frontend attachment over the whole lane.
    ///
    /// The frontend owns no lifetime (Pillar 7): this starts nothing, stops
    /// nothing and touches no durable state. It discards the backlog the
    /// superseded attachment never drained and re-subscribes every known card,
    /// so each card's stream restarts at a NEW epoch that opens with the
    /// snapshot the contract promises — which is what makes a re-attached
    /// frontend usable at all.
    ///
    /// There is deliberately no matching detach. A frontend that goes away
    /// simply stops polling, so "leave" is not a verb it can spell and cannot
    /// be the thing that affects a transfer.
    ///
    /// The returned token is the attachment's identity, and the ONE thing that
    /// can consume from the lane afterwards. Every subscription is opened
    /// before the lock is taken and the whole set is installed under it, so the
    /// pump never observes a lane that is half-way through being reopened.
    pub fn open_lane(&self) -> AttachmentToken {
        let cards: Vec<RecordId> = self.shared.lock().known.iter().copied().collect();
        let fresh: Vec<(RecordId, Result<CardSubscription, SubscribeError>)> = cards
            .into_iter()
            .map(|card| (card, self.subscribe(card)))
            .collect();
        let mut state = self.shared.lock();
        state.attachment = AttachmentToken(state.attachment.0 + 1);
        state.frames.clear();
        // The diagnostics this process holds are older than this attachment,
        // so nothing would ever re-state them: an observer that saw only what
        // changed after it arrived would show an empty log next to a card with
        // a whole history.
        self.shared.evidence.reseed();
        for (card, subscription) in fresh {
            install(&mut state, card, subscription);
        }
        state.attachment
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

    /// One encoded contract frame for the attachment `token` identifies.
    ///
    /// The queue is destructive, so the token is checked BEFORE anything is
    /// popped: a superseded consumer cannot take the snapshot its successor is
    /// waiting for, whatever order the two threads happen to run in.
    pub fn poll_frame(&self, token: AttachmentToken) -> FramePoll {
        let mut state = self.shared.lock();
        if token != state.attachment {
            return FramePoll::Superseded;
        }
        state
            .frames
            .pop_front()
            .map_or(FramePoll::Drained, FramePoll::Frame)
    }

    /// One encoded platform work order for the service executor, if any.
    pub fn poll_work(&self) -> Option<Vec<u8>> {
        self.shared.lock().work.pop_front()
    }

    /// One card whose durable removal ended ownership of its persistable source
    /// grant. Delivery is at least once across process generations and once
    /// within one: releasing an Android URI grant is itself idempotent.
    pub fn poll_source_release(&self) -> Option<RecordId> {
        self.shared.lock().source_releases.pop_front()
    }

    /// Takes one frontend-originated intent frame and returns the encoded
    /// answer: an acceptance for a command on an existing card, or a create
    /// result for a request that one be made.
    ///
    /// Both intents ride ONE contract, one lane and one native verb, because
    /// both are the same thing — a frontend asking the authority for something
    /// and being told what happened.
    pub fn intent(&self, bytes: &[u8]) -> Result<Vec<u8>, IntentRejection> {
        match decode_intent(bytes) {
            Ok(FrontendIntent::Command(spec)) => Ok(self.submit_command(spec)),
            Ok(FrontendIntent::Create(spec)) => Ok(self.create(&spec)),
            // The acquisition protocol's intake, decodable but not yet
            // answerable: the reducer edge that accepts an offer lands with the
            // source-acquisition step, and there is no typed "not yet" on this
            // verb to answer with.
            //
            // So today this is refused, and the refusal is honest rather than
            // right: a frontend that sent a well-formed offer would be told its
            // frame was a contract breach. Nothing sends one — the frontend
            // still has no source flow — and the alternative, inventing an
            // answer the product cannot stand behind, is worse. Stated here so
            // the next step replaces it rather than discovering it.
            Ok(FrontendIntent::SourceOffer(_)) => Err(IntentRejection::Contract),
            // Hostile or non-intent bytes carry no usable request id. This is
            // nevertheless a typed authority refusal, not zero bytes that a
            // frontend can only misreport as a lost answer.
            Err(SubmitDecodeError::Frame(_) | SubmitDecodeError::NotAnIntent) => {
                Err(IntentRejection::Contract)
            }
        }
    }

    /// Creates one card from a validated intent and answers with its durable
    /// verdict.
    ///
    /// The order is the whole point (`SF02`, Pillar 5): the invite is judged,
    /// the identity is minted, the record is COMMITTED, and only then is the
    /// card brought live — so the first attempt, or the request for a source
    /// handle, is authorized by a write that already landed. `created` is
    /// therefore a claim about the disk, not about intake.
    ///
    /// A card that commits but cannot be admitted (a stopped or full runtime)
    /// is still created: the record exists, and the lane says so in its own
    /// words by refusing the subscription rather than by unmaking the card.
    fn create(&self, spec: &CreateSpec) -> Vec<u8> {
        // The lock covers lookup through initial commit. It is deliberately
        // separate from `SharedState`: no frame/store lock-order is introduced,
        // and concurrent deliveries of one id cannot both allocate a card.
        let mut creates = self.creates.lock().unwrap_or_else(PoisonError::into_inner);
        // The endpoint is only knowable once the invite has been judged, so the
        // request-id lookup happens first and the endpoint lookup after
        // planning — both still inside the one lock that covers the commit.
        let outcome = if let Some(outcome) = creates.existing(spec.request_id, None) {
            outcome
        } else {
            match create::plan(spec) {
                Ok(transfer) => {
                    let endpoint = transfer
                        .pairing
                        .as_ref()
                        .map(|pairing| (pairing.code().to_owned(), transfer.direction));
                    match creates.existing(spec.request_id, endpoint.as_ref()) {
                        Some(outcome) => outcome,
                        None => {
                            let outcome = self.commit_new_card(spec.request_id, transfer);
                            creates.remember(spec.request_id, endpoint, outcome.clone());
                            outcome
                        }
                    }
                }
                // A refusal allocates nothing and is a pure function of the
                // request, so a repeat re-derives the identical answer and
                // there is nothing to remember.
                Err(refusal) => CreateOutcomeView::Refused(refusal),
            }
        };
        encode_command_frame(&create_result_frame(spec.request_id, outcome)).unwrap_or_default()
    }

    fn commit_new_card(&self, request_id: CommandId, transfer: NewTransfer) -> CreateOutcomeView {
        let store = HostStore::deferred(self.stores.clone());
        let created = CommittedSession::create_identified(
            transfer,
            request_id,
            &mut SystemIdentitySource,
            store,
            NonZeroUsize::new(3).expect("nonzero"),
        );
        let Ok((session, initial)) = created else {
            return CreateOutcomeView::Refused(CreateRefusalView::Internal);
        };
        let card = session.record().identity.card;
        let outcome = create_outcome(initial.commit, card);
        if matches!(outcome, CreateOutcomeView::Created(_)) {
            self.runtime.admit(session, initial).ok();
            self.attach(card);
        }
        outcome
    }

    /// Rebuilds one successful create result from the same durable record that
    /// authorized the card, on both keys. Missing/pre-F2b/corrupt records have
    /// no create identity to claim and therefore cannot poison the retry index.
    ///
    /// The endpoint has to be rebuilt here too: it is the key that survives the
    /// frontend, so an index restored on the request id alone would answer
    /// nothing after the restart it exists for.
    #[allow(clippy::type_complexity)]
    fn create_receipt(
        &self,
        card: RecordId,
    ) -> Option<(CommandId, Option<(String, Direction)>, CreateOutcomeView)> {
        let record = self.durable_record(card)?;
        let request_id = *record.create_request_id?;
        let endpoint = record
            .pairing
            .as_ref()
            .map(|pairing| (pairing.code().to_owned(), record.direction));
        Some((
            request_id,
            endpoint,
            CreateOutcomeView::Created(CardCreatedView {
                card: format!("{:016x}", card.get()),
            }),
        ))
    }

    fn durable_record(&self, card: RecordId) -> Option<TransferRecord> {
        let store = HostStore::opened(self.stores.clone(), card)?;
        let RecordDecode::Loaded(record) = decode_record(&store.latest()?).ok()? else {
            return None;
        };
        Some(*record)
    }

    fn submit_command(&self, spec: SubmitSpec) -> Vec<u8> {
        let shared = Arc::clone(&self.shared);
        let runtime = Arc::clone(&self.runtime);
        self.tokio.block_on(async move {
            // Take the subscription out for the await so no lock is held
            // across it; the pump simply skips the card for one tick. The
            // attachment it was taken under is remembered with it: putting it
            // back is only correct while it is still the card's commander.
            let (attachment, taken) = {
                let mut state = shared.lock();
                (state.attachment, state.subscriptions.remove(&spec.card))
            };
            let acceptance = match taken {
                Some(subscription) if subscription.epoch().get() == spec.epoch => {
                    let acceptance = runtime
                        .submit_command(&subscription, spec.command_id, spec.command)
                        .await;
                    restore(&shared, attachment, spec.card, subscription);
                    acceptance
                }
                Some(subscription) => {
                    restore(&shared, attachment, spec.card, subscription);
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
    pub fn create_for_e2e(&self) -> Result<RecordId, IdentityError> {
        let transfer = NewTransfer {
            // A RECEIVER, because this instrumentation wants a card with a live
            // attempt to kill and restore — and receiving is now the only way
            // to have one at creation. A sender must be given a document first,
            // so it would sit at rest and prove less about durability than the
            // card this used to make.
            direction: Direction::Receive,
            participation: envoix_product::RoomParticipation::Minted,
            pairing: None,
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

    /// Debug/e2e only: the card's LATEST COMMITTED state, read back out of the
    /// operation store rather than out of the runtime. Instrumentation that
    /// asserts against a projection cannot tell a durable write from a change
    /// that only ever existed in memory; this can.
    #[cfg(feature = "e2e-instrumentation")]
    pub fn durable_state_for_e2e(&self, card: RecordId) -> &'static str {
        let Some(operation) = self.stores.open(card) else {
            return "absent";
        };
        let operation = operation.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(encoded) = operation.latest_record() else {
            return "absent";
        };
        let Ok(RecordDecode::Loaded(record)) = decode_record(encoded) else {
            return "unreadable";
        };
        match record.state {
            ProductState::Preparing => "preparing",
            ProductState::Waiting => "waiting",
            ProductState::Connecting => "connecting",
            ProductState::Verifying => "verifying",
            ProductState::Transferring => "transferring",
            ProductState::Confirming => "confirming",
            ProductState::Paused(_) => "paused",
            ProductState::Unconfirmed => "unconfirmed",
            ProductState::Completed => "completed",
            ProductState::Failed => "failed",
            ProductState::Cancelled => "cancelled",
        }
    }

    /// Stops the runtime; durable truth is on disk, the process may die.
    pub fn shutdown(self) {
        let runtime = Arc::clone(&self.runtime);
        self.tokio.block_on(async move {
            runtime.shutdown().await;
        });
    }
}

/// What a create request is told, given how its record write ended.
///
/// `created` is a claim about the DISK, so it is made only by a barrier that
/// actually crossed. An escalated write means no card exists: answering with a
/// card id would hand the frontend an identity for something no reboot will
/// ever find. Stated once, here, so there is one rule to hold to that.
fn create_outcome(commit: CommitStatus, card: RecordId) -> CreateOutcomeView {
    if commit.authorizing_commit_succeeded() {
        CreateOutcomeView::Created(CardCreatedView {
            card: format!("{:016x}", card.get()),
        })
    } else {
        CreateOutcomeView::Refused(CreateRefusalView::StorageFault)
    }
}

/// Records one subscribe outcome against the shared state, under a lock the
/// caller already holds. A refusal is typed truth: the reason crosses to the
/// frontend as a `subscribe_rejected` frame.
fn install(
    state: &mut SharedState,
    card: RecordId,
    subscription: Result<CardSubscription, SubscribeError>,
) {
    match subscription {
        Ok(subscription) => {
            state.known.insert(card);
            state.subscriptions.insert(card, subscription);
        }
        Err(error) => {
            state.subscriptions.remove(&card);
            // A card the runtime holds no projection for is nothing to
            // observe, so it stops being one of ours; a stopped or exhausted
            // runtime is a reason to try again next time.
            if matches!(error, SubscribeError::UnknownCard) {
                state.known.remove(&card);
            } else {
                state.known.insert(card);
            }
            push_read_frame(&mut state.frames, &subscribe_rejected_frame(card, error));
        }
    }
}

/// Puts a subscription taken out for an await back, but only while it is still
/// the card's commander: a concurrent `open_lane` installs a fresh-epoch
/// subscription, and overwriting it with this one would lose that epoch's
/// snapshot and freeze the card at an epoch nothing feeds.
fn restore(
    shared: &Shared,
    attachment: AttachmentToken,
    card: RecordId,
    subscription: CardSubscription,
) {
    let mut state = shared.lock();
    if state.attachment == attachment && !state.subscriptions.contains_key(&card) {
        state.subscriptions.insert(card, subscription);
    }
}

/// Drains every subscription once: contract frames to the frame lane, duty
/// updates through the adapter to the work lane.
fn pump_once(shared: &Shared) {
    let mut state = shared.lock();
    let SharedState {
        attachment: _,
        known: _,
        subscriptions,
        ledger,
        adapter,
        frames,
        work,
        source_releases,
        source_releases_seen,
    } = &mut *state;
    let mut closed: Vec<(RecordId, u64)> = Vec::new();
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
                        if record.facts.remove_requested && source_releases_seen.insert(update.card)
                        {
                            source_releases.push_back(update.card);
                        }
                    }
                    let frame = card_update_frame(update.epoch.get(), update.card, &update.kind);
                    push_read_frame(frames, &frame);
                }
                Err(TryRecvError::Empty) => break,
                // The runtime ended this epoch. Surfacing it once — and
                // dropping the dead subscription — is what stops the lane from
                // pretending a card is still being observed.
                Err(TryRecvError::Closed) => {
                    closed.push((*card, subscription.epoch().get()));
                    break;
                }
                Err(TryRecvError::Lagged(lag)) => {
                    let frame = lag_frame(lag.epoch.get(), *card, lag.missed);
                    push_read_frame(frames, &frame);
                    break;
                }
            }
        }
    }
    for (card, epoch) in closed {
        subscriptions.remove(&card);
        push_read_frame(frames, &closed_frame(epoch, card));
    }
    for session in shared.evidence.take_unpublished() {
        // A session the store has since evicted has no timeline left to state,
        // and the contract carries no "forgotten" frame — inventing one would
        // be telling the observer something the authority never said.
        if let Some(timeline) = shared.evidence.store.snapshot(session) {
            push_read_frame(frames, &evidence_frame(&timeline));
        }
    }
}

fn enqueue_source_release(state: &mut SharedState, card: RecordId) {
    if state.source_releases_seen.insert(card) {
        state.source_releases.push_back(card);
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

#[cfg(test)]
mod tests {
    use envoix_product::CommitFailure;

    use super::*;

    /// Regression for the old `if let Ok(bytes)` pump shape: the codec's typed
    /// error must be visible at the failure site, never converted into an
    /// absent frame.
    #[test]
    #[should_panic(expected = "read projection rejected by typed codec: FrameTooLarge")]
    fn a_rejected_read_projection_fails_loudly() {
        push_encoded_read(&mut VecDeque::new(), Err(ReadError::FrameTooLarge));
    }

    /// A card is "created" only when its record write crossed the barrier.
    ///
    /// The sweep is exhaustive over `CommitStatus`, including the escalated
    /// arm's `failed_state_persisted` both ways: a best-effort write of the
    /// FAILURE state is not the record that was asked for, so it is still not
    /// a card. Nothing else in this crate can exercise a failing store, which
    /// is exactly why the decision is one function with its own gate.
    #[test]
    fn a_create_is_only_created_when_its_record_committed() {
        let card = RecordId::new(0xf2b);
        let hex = "0000000000000f2b";
        for commit in [
            CommitStatus::Vacuous,
            CommitStatus::Committed { attempts: 1 },
            CommitStatus::Committed { attempts: 3 },
        ] {
            assert_eq!(
                create_outcome(commit, card),
                CreateOutcomeView::Created(CardCreatedView {
                    card: hex.to_owned()
                }),
                "{commit:?} committed, so a card exists"
            );
        }
        let mut refused = 0;
        for failed_state_persisted in [false, true] {
            for failure in [
                CommitFailure::Store(envoix_product::CommitError),
                CommitFailure::Encode(envoix_product::RecordCodecError::MalformedBody),
            ] {
                let commit = CommitStatus::Escalated {
                    attempts: 3,
                    failure,
                    failed_state_persisted,
                };
                assert_eq!(
                    create_outcome(commit, card),
                    CreateOutcomeView::Refused(CreateRefusalView::StorageFault),
                    "{commit:?} never landed the record, so nothing was created"
                );
                refused += 1;
            }
        }
        assert_eq!(refused, 4, "both escalation axes were swept");
        // `NotRequired` cannot arise from a create — creation always changes
        // durable state — but the rule still has to answer, and refusing is the
        // answer that cannot invent a card.
        assert_eq!(
            create_outcome(CommitStatus::NotRequired, card),
            CreateOutcomeView::Refused(CreateRefusalView::StorageFault)
        );
    }

    /// `submit_command` takes the card's subscription out for its await so no
    /// lock is held across it. If the frontend reopens the lane in that window, the
    /// subscription put back afterwards is the SUPERSEDED one: the fresh
    /// epoch's snapshot is lost and the card freezes at an epoch nothing feeds.
    /// F1b made `open_lane` frontend-triggerable at any instant, so the window
    /// is real; the reinsert is guarded on still being the card's commander.
    #[test]
    fn a_superseded_subscription_never_overwrites_the_cards_commander() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Host::boot(root.path()).expect("the host boots");
        let card = host.create_for_e2e().expect("a durable card is created");

        let attachment = host.open_lane();
        let taken = host
            .shared
            .lock()
            .subscriptions
            .remove(&card)
            .expect("the card has a live subscription");
        // The frontend re-attaches while the await is in flight.
        let reopened = host.open_lane();
        let fresh = host.shared.lock().subscriptions[&card].epoch().get();
        assert_ne!(fresh, taken.epoch().get());

        restore(&host.shared, attachment, card, taken);
        assert_eq!(
            host.shared.lock().subscriptions[&card].epoch().get(),
            fresh,
            "the superseded subscription overwrote the card's commander"
        );

        // With no reopen in the window the subscription simply goes back.
        let taken = host
            .shared
            .lock()
            .subscriptions
            .remove(&card)
            .expect("still subscribed");
        let epoch = taken.epoch().get();
        restore(&host.shared, reopened, card, taken);
        assert_eq!(host.shared.lock().subscriptions[&card].epoch().get(), epoch);
    }
}
