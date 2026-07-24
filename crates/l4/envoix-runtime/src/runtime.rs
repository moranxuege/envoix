use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use envoix_attempt_api::AttemptStamp;
use envoix_capabilities::Duty;
use envoix_evidence::{EvidenceProgress, EvidenceRecord, EvidenceSink, EvidenceValue};
use envoix_product::{
    ApplyOutcome, CapabilityAction, CommittedSession, LedgerHit, ProductCommand, ProductInput,
    TransferRecord,
};
use envoix_types::{CommandId, RecordId};
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::card::{CardActor, CardMessage};
use crate::command::{CommandTicket, CommandVerdict, FrontendVerdict};
use crate::config::RuntimeConfig;
use crate::error::{AcquireError, CommandRejected};
use crate::evidence::EvidencePublisher;
use crate::port::{AttemptExecutor, SessionProvider};
use crate::subscription::{
    CardSubscription, RecordUpdateKind, SubscribeError, SubscriptionEpoch, SubscriptionPublisher,
    subscription_channel,
};

const INBOX_CAPACITY: usize = 64;

/// Why one delivery round of a frontend command did not land.
enum Undelivered {
    /// Typed intake refusal — final.
    Refused(CommandRejected),
    /// Lost a hibernate/spawn race; the message survives for the next round.
    Raced,
}

/// A live card's registry slot: its inbox, and — once spawned — its supervised
/// task handle. Holding the handle here (rather than a side list) makes shutdown
/// draining atomic: whatever the registry holds is exactly what shutdown joins.
struct CardEntry {
    inbox: mpsc::Sender<CardMessage>,
    handle: Option<JoinHandle<()>>,
}

/// Process-lifetime derived state for a known card. It remains after the actor
/// hibernates, but owns no authority: every field is copied from a committed L3
/// record or a committed, idempotent L3 duty effect.
struct CardProjection {
    record: TransferRecord,
    outstanding_duties: Vec<(Duty, CapabilityAction)>,
    subscribers: Vec<SubscriptionPublisher>,
    /// The newest attachment epoch — the card's commander. Only commands
    /// stamped with it pass intake; a reattach supersedes older authority.
    commander: Option<SubscriptionEpoch>,
}

/// Whether the committed record shows this capability duty already discharged, so
/// the projection can prune it (a reattach must never re-deliver a completed
/// duty). Exhaustive over `CapabilityAction`, so a new duty kind must add a
/// discharge rule here.
fn duty_discharged(action: CapabilityAction, record: &TransferRecord) -> bool {
    match action {
        CapabilityAction::PostReceipt => record.facts.proof_delivered,
    }
}

/// The result of a `shutdown`: how many live cards were torn down, and how many
/// of their tasks had to be force-aborted after missing the grace deadline
/// (`forced == 0` is a clean, fully-joined teardown).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShutdownReport {
    pub cards: usize,
    pub forced: usize,
}

/// The admission gate and the leased registry under ONE lock, so `reserve`,
/// `spawn_card`, and `shutdown` serialize: a card can never be spawned into the
/// registry after shutdown has drained it (which would let its task escape the
/// join), and admission is refused atomically with the drain.
struct Inner {
    stopped: bool,
    next_epoch: u64,
    cards: HashMap<RecordId, CardEntry>,
    projections: HashMap<RecordId, CardProjection>,
}

/// Generic-free runtime state, shared by every card actor via `Arc` so an actor
/// can release its own lease without knowing the port types.
pub(crate) struct Shared {
    pub(crate) config: RuntimeConfig,
    pub(crate) handle: Handle,
    inner: Mutex<Inner>,
    admission: Arc<Semaphore>,
    evidence: EvidencePublisher,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Poison-tolerant: this crate exists to CONTAIN actor panics, so a panic
        // that ever unwinds while this lock is held must not brick the whole
        // runtime (a poisoned `expect` would, and would double-panic in the
        // actor's release-on-unwind path).
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn release(&self, card: RecordId) {
        self.lock().cards.remove(&card);
    }

    /// Evicts a card's derived projection. Hibernation deliberately PRESERVES the
    /// projection (so an at-rest card still renders and reattaches to current
    /// truth); this is called only when a card is durably removed / tombstoned and
    /// no longer exists, so its cache must not leak for the process lifetime.
    pub(crate) fn evict_projection(&self, card: RecordId) {
        self.lock().projections.remove(&card);
        self.evidence.evict_card(card);
    }

    pub(crate) fn observe_record(&self, kind: RecordUpdateKind, record: TransferRecord) {
        let card = record.identity.card;
        let session = AttemptStamp {
            card,
            generation: record.generation,
        };
        let evidence = match kind {
            RecordUpdateKind::Progress => {
                EvidenceValue::progress(EvidenceProgress::new(record.bytes, record.total))
            }
            RecordUpdateKind::State => EvidenceValue::phase(record.phase),
            RecordUpdateKind::Terminal => record.outcome.as_ref().map_or_else(
                || EvidenceValue::phase(record.phase),
                EvidenceValue::outcome,
            ),
        };
        let subscribers = {
            let mut inner = self.lock();
            let projection = inner
                .projections
                .entry(card)
                .or_insert_with(|| CardProjection {
                    record: record.clone(),
                    outstanding_duties: Vec::new(),
                    subscribers: Vec::new(),
                    commander: None,
                });
            projection.record = record.clone();
            // Prune any duty the committed record now shows discharged, so a
            // reattach re-delivers only genuinely-outstanding duties.
            projection
                .outstanding_duties
                .retain(|(_, action)| !duty_discharged(*action, &record));
            projection
                .subscribers
                .retain(SubscriptionPublisher::is_attached);
            projection.subscribers.clone()
        };
        for subscriber in subscribers {
            subscriber.publish_record(kind, record.clone());
        }
        self.evidence
            .publish(EvidenceRecord::new(session, evidence));
    }

    pub(crate) fn observe_duty(&self, card: RecordId, duty: Duty, action: CapabilityAction) {
        let subscribers = {
            let mut inner = self.lock();
            let Some(projection) = inner.projections.get_mut(&card) else {
                return;
            };
            // Supersede by action: keep only the LATEST duty per action, so a duty
            // re-issued across restores replaces rather than accumulates — the
            // outstanding set stays bounded by the distinct actions.
            projection
                .outstanding_duties
                .retain(|(_, existing)| *existing != action);
            projection.outstanding_duties.push((duty, action));
            projection
                .subscribers
                .retain(SubscriptionPublisher::is_attached);
            projection.subscribers.clone()
        };
        for subscriber in subscribers {
            subscriber.publish_duty(duty, action);
        }
    }

    fn snapshot(&self, card: RecordId) -> Option<TransferRecord> {
        self.lock()
            .projections
            .get(&card)
            .map(|projection| projection.record.clone())
    }

    fn subscribe(
        &self,
        card: RecordId,
        capacity: NonZeroUsize,
    ) -> Result<CardSubscription, SubscribeError> {
        let mut inner = self.lock();
        if inner.stopped {
            return Err(SubscribeError::RuntimeStopped);
        }
        if !inner.projections.contains_key(&card) {
            return Err(SubscribeError::UnknownCard);
        }
        let epoch = SubscriptionEpoch::new(inner.next_epoch);
        inner.next_epoch = inner
            .next_epoch
            .checked_add(1)
            .ok_or(SubscribeError::EpochExhausted)?;
        let projection = inner
            .projections
            .get_mut(&card)
            .expect("the projection was checked under the held lock");
        let (publisher, subscription) = subscription_channel(
            epoch,
            card,
            capacity,
            projection.record.clone(),
            &projection.outstanding_duties,
        );
        projection
            .subscribers
            .retain(SubscriptionPublisher::is_attached);
        projection.subscribers.push(publisher);
        projection.commander = Some(epoch);
        Ok(subscription)
    }

    /// The card's current commander (newest attachment) epoch.
    pub(crate) fn commander_epoch(&self, card: RecordId) -> Option<SubscriptionEpoch> {
        self.lock().projections.get(&card)?.commander
    }

    fn close_subscriptions(inner: &mut Inner) -> Vec<SubscriptionPublisher> {
        inner
            .projections
            .values_mut()
            .flat_map(|projection| projection.subscribers.drain(..))
            .collect()
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(PoisonError::into_inner);
        for subscriber in Self::close_subscriptions(inner) {
            subscriber.close();
        }
    }
}

/// The process-lifetime owner. Cloning yields another handle onto the same
/// runtime (idempotent bootstrap: there is one shared state, obtained once at
/// `start` and shared by clone).
pub struct Runtime<P: SessionProvider, E: AttemptExecutor> {
    shared: Arc<Shared>,
    provider: Arc<P>,
    executor: Arc<E>,
}

impl<P: SessionProvider, E: AttemptExecutor> Clone for Runtime<P, E> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            provider: self.provider.clone(),
            executor: self.executor.clone(),
        }
    }
}

impl<P: SessionProvider, E: AttemptExecutor> Runtime<P, E> {
    /// Builds the runtime. Must be called within a Tokio runtime; the host owns
    /// the reactor and RT1 holds its handle.
    pub fn start(config: RuntimeConfig, provider: P, executor: E) -> Self {
        Self::start_inner(config, provider, executor, EvidencePublisher::default())
    }

    /// Builds the runtime with a typed evidence sink.
    ///
    /// The sink runs behind a fixed-capacity, non-blocking lane. Its latency,
    /// failures, panics, disconnection, or saturation cannot affect admission,
    /// reduction, durable commits, commands, or shutdown.
    pub fn start_with_evidence<S: EvidenceSink>(
        config: RuntimeConfig,
        provider: P,
        executor: E,
        sink: S,
    ) -> Self {
        Self::start_inner(config, provider, executor, EvidencePublisher::new(sink))
    }

    fn start_inner(
        config: RuntimeConfig,
        provider: P,
        executor: E,
        evidence: EvidencePublisher,
    ) -> Self {
        let admission = Arc::new(Semaphore::new(config.max_live_cards.get()));
        Self {
            shared: Arc::new(Shared {
                config,
                handle: Handle::current(),
                inner: Mutex::new(Inner {
                    stopped: false,
                    next_epoch: 1,
                    cards: HashMap::new(),
                    projections: HashMap::new(),
                }),
                admission,
                evidence,
            }),
            provider: Arc::new(provider),
            executor: Arc::new(executor),
        }
    }

    /// Brings a freshly created card live: seeds the actor with its creation
    /// outcome (which typically starts the first attempt).
    pub fn admit(
        &self,
        session: CommittedSession<P::Store>,
        initial: ApplyOutcome,
    ) -> Result<(), AcquireError> {
        let card = session.record().identity.card;
        let (permit, inbox, inbox_rx) = self.reserve(card)?;
        self.spawn_card(card, session, initial, permit, inbox, inbox_rx);
        Ok(())
    }

    /// Lazily restores a durable card and brings it live. It feeds
    /// `ProductInput::Restore` and lets the reducer's `on_restore` reconcile the
    /// lifecycle — the runtime never fabricates quiescence, synthesizes a worker
    /// terminal, or reconstructs an ack on restore (L3 is the one authority).
    pub fn restore(&self, card: RecordId) -> Result<(), AcquireError> {
        // Reserve first so a concurrent restore/admit of the same card is
        // refused, then read the durable session outside the registry lock.
        let (permit, inbox, inbox_rx) = self.reserve(card)?;
        let mut session = match self.provider.restore(card) {
            Some(session) => session,
            None => {
                self.shared.release(card);
                return Err(AcquireError::Absent);
            }
        };
        let initial = match session.apply(ProductInput::Restore) {
            Ok(outcome) => outcome,
            Err(_) => {
                self.shared.release(card);
                return Err(AcquireError::Internal);
            }
        };
        self.spawn_card(card, session, initial, permit, inbox, inbox_rx);
        Ok(())
    }

    /// Submits one identified mutating command through the durable intake.
    ///
    /// Provenance is the live attachment itself: `origin` must be the card's
    /// NEWEST subscription (its commander) — a bare epoch integer cannot be
    /// submitted, and a superseded or torn-down attachment's commands are
    /// rejected typed. The returned verdict is ACCEPTANCE only; the committed
    /// completion arrives separately through the [`CommandVerdict::Accepted`]
    /// ticket. A command identity already in the committed ledger answers
    /// [`CommandVerdict::Duplicate`] without reducing anything — including for
    /// hibernated cards, straight from committed truth. A fresh command to a
    /// hibernated card lazily restores it with the command pre-queued, so the
    /// actor processes it before any hibernate decision.
    ///
    /// # The exact supersession guarantee
    /// A superseded attachment's commands are inert BEFORE acceptance: the
    /// gate and the actor's commander re-check both reject them typed. A
    /// command that was already ACCEPTED when the reattach landed still
    /// commits — its submitter honestly observes `Accepted` then `Committed`,
    /// and the new attachment's stream sees the resulting state update.
    /// Strict post-acceptance inertness would require the commander
    /// generation to join the record write; that is a deliberate non-goal
    /// (BN3 documents this guarantee to hosts).
    pub async fn submit_command(
        &self,
        origin: &CardSubscription,
        id: CommandId,
        command: ProductCommand,
    ) -> Result<CommandVerdict, CommandRejected> {
        let card = origin.card();
        {
            let inner = self.shared.lock();
            if inner.stopped {
                return Err(CommandRejected::RuntimeStopped);
            }
            let Some(projection) = inner.projections.get(&card) else {
                return Err(CommandRejected::UnknownCard);
            };
            if projection.commander != Some(origin.epoch()) {
                return Err(CommandRejected::StaleEpoch);
            }
            match projection.record.command_ledger.disposition(id, command) {
                Some(LedgerHit::Duplicate { state }) => {
                    return Ok(CommandVerdict::Duplicate { state });
                }
                Some(LedgerHit::Conflict { .. }) => {
                    return Err(CommandRejected::Conflict);
                }
                None => {}
            }
        }
        // Deliver to the live actor, or lazily restore the card with the
        // message pre-queued. Each round builds fresh reply channels so a
        // race can be retried with the message rebuilt. Three rounds absorb
        // two independent races, each of which resolves within one round:
        // a send that fails (actor just exited, receiver dropped), a restore
        // reservation lost to a concurrent spawn, or a send that SUCCEEDED
        // into an actor that exited without draining (the buffered message —
        // never linearized — died with the inbox, so redelivery cannot
        // double-apply: the ledger dedups a committed identity).
        for _ in 0..3 {
            let (acceptance_tx, acceptance_rx) = oneshot::channel();
            let (completion_tx, completion_rx) = oneshot::channel();
            let message = CardMessage::Frontend {
                epoch: origin.epoch(),
                id,
                command,
                acceptance: acceptance_tx,
                completion: completion_tx,
            };
            let outcome = match self.inbox(card) {
                Some(inbox) => inbox.send(message).await.map_err(|_| Undelivered::Raced),
                None => self.restore_with_message(card, message),
            };
            match outcome {
                Ok(()) => match acceptance_rx.await {
                    // The message died undelivered with an exiting actor's
                    // inbox (its sender was never touched); retry the round.
                    Err(_) => continue,
                    Ok(Err(rejected)) => return Err(rejected),
                    Ok(Ok(FrontendVerdict::Duplicate { state })) => {
                        return Ok(CommandVerdict::Duplicate { state });
                    }
                    Ok(Ok(FrontendVerdict::Accepted)) => {
                        return Ok(CommandVerdict::Accepted(CommandTicket {
                            completion: completion_rx,
                        }));
                    }
                },
                Err(Undelivered::Refused(rejected)) => return Err(rejected),
                Err(Undelivered::Raced) => {}
            }
        }
        Err(CommandRejected::Interrupted)
    }

    /// Restores a hibernated card with `message` already queued on its fresh
    /// inbox, so the actor cannot hibernate past it. Uses the identical
    /// restore contract as [`Self::restore`] (feed `ProductInput::Restore`,
    /// let the hardened reducer reconcile).
    ///
    /// Blocking bound: `provider.restore` + the restore reduction run
    /// synchronous store I/O inline in the submitter's async task (the same
    /// shape as host-called `admit`/`restore`). BN4 routes this through the
    /// host's blocking seam; until then a slow store stalls this task for one
    /// card-restore, bounded by the store's own commit budget.
    fn restore_with_message(
        &self,
        card: RecordId,
        message: CardMessage,
    ) -> Result<(), Undelivered> {
        let (permit, inbox, inbox_rx) = match self.reserve(card) {
            Ok(reserved) => reserved,
            // A concurrent admit/restore won the lease; its inbox is (or is
            // about to be) observable — retry the send round.
            Err(AcquireError::AlreadyLive) => return Err(Undelivered::Raced),
            Err(AcquireError::NotAdmitting) => {
                return Err(Undelivered::Refused(CommandRejected::RuntimeStopped));
            }
            Err(AcquireError::AtCapacity) => {
                return Err(Undelivered::Refused(CommandRejected::AtCapacity));
            }
            Err(AcquireError::Absent | AcquireError::Internal) => {
                return Err(Undelivered::Refused(CommandRejected::Internal));
            }
        };
        let mut session = match self.provider.restore(card) {
            Some(session) => session,
            None => {
                self.shared.release(card);
                return Err(Undelivered::Refused(CommandRejected::Internal));
            }
        };
        let initial = match session.apply(ProductInput::Restore) {
            Ok(outcome) => outcome,
            Err(_) => {
                self.shared.release(card);
                return Err(Undelivered::Refused(CommandRejected::Internal));
            }
        };
        match inbox.try_send(message) {
            Ok(()) => {
                self.spawn_card(card, session, initial, permit, inbox, inbox_rx);
                Ok(())
            }
            // The registry publishes the inbox before this send, so enough
            // concurrent submits can fill it first. The card must still spawn
            // (those queued messages need their actor); our message retries
            // into the now-live inbox on the next round.
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.spawn_card(card, session, initial, permit, inbox, inbox_rx);
                Err(Undelivered::Raced)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                unreachable!("the reserved inbox receiver is held on this stack")
            }
        }
    }

    /// A derived read snapshot of a known card's latest committed record.
    ///
    /// The projection remains available when a terminal/quiescent actor
    /// hibernates. Durable L3 state remains authoritative.
    pub async fn snapshot(&self, card: RecordId) -> Option<TransferRecord> {
        self.shared.snapshot(card)
    }

    /// Attaches a bounded, per-card frontend stream.
    ///
    /// Every successful call opens a fresh epoch and seeds it from current
    /// derived truth plus all outstanding capability duties. Dropping the
    /// returned handle is the entire detach operation and cannot affect a card
    /// actor or transfer state.
    pub fn subscribe(
        &self,
        card: RecordId,
        capacity: NonZeroUsize,
    ) -> Result<CardSubscription, SubscribeError> {
        self.shared.subscribe(card, capacity)
    }

    pub fn is_live(&self, card: RecordId) -> bool {
        self.shared.lock().cards.contains_key(&card)
    }

    pub fn live_cards(&self) -> usize {
        self.shared.lock().cards.len()
    }

    /// Stops admitting, tears down every live worker within the grace, joins the
    /// supervised tasks, and releases all leases. Idempotent and safe to call
    /// twice (a second call finds nothing live and returns the default report).
    /// It does NOT cancel transfers — durable truth is preserved and each card
    /// resumes via `restore` on the next start.
    pub async fn shutdown(&self) -> ShutdownReport {
        // Atomically stop admission AND take ownership of every live card, so no
        // in-flight `admit`/`restore` can spawn an actor that escapes this
        // teardown: a `spawn_card` racing this sees `stopped` and aborts instead.
        let (drained, subscribers): (Vec<CardEntry>, Vec<SubscriptionPublisher>) = {
            let mut inner = self.shared.lock();
            inner.stopped = true;
            let cards = inner.cards.drain().map(|(_, entry)| entry).collect();
            let subscribers = Shared::close_subscriptions(&mut inner);
            (cards, subscribers)
        };
        for subscriber in subscribers {
            subscriber.close();
        }
        let cards = drained.len();

        // Phase 1: signal every card, then await the replies. ONE shared deadline
        // bounds BOTH the sends and the reply awaits, so a wedged actor (a full
        // inbox, or one that never replies) cannot stall teardown.
        let reply_deadline = Instant::now() + self.shared.config.shutdown_grace;
        let mut replies = Vec::new();
        for entry in &drained {
            let (reply, response) = oneshot::channel();
            if let Ok(Ok(())) = timeout_at(
                reply_deadline,
                entry.inbox.send(CardMessage::Shutdown(reply)),
            )
            .await
            {
                replies.push(response);
            }
        }
        for response in replies {
            let _ = timeout_at(reply_deadline, response).await;
        }

        // Phase 2: join every task under ONE shared deadline; abort the stragglers.
        let join_deadline = Instant::now() + self.shared.config.shutdown_grace;
        let mut forced = 0;
        for entry in drained {
            let Some(handle) = entry.handle else { continue };
            let abort = handle.abort_handle();
            if timeout_at(join_deadline, handle).await.is_err() {
                abort.abort();
                forced += 1;
            }
        }
        ShutdownReport { cards, forced }
    }

    fn inbox(&self, card: RecordId) -> Option<mpsc::Sender<CardMessage>> {
        self.shared
            .lock()
            .cards
            .get(&card)
            .map(|entry| entry.inbox.clone())
    }

    /// Acquires the single-writer lease and the admission permit for `card` under
    /// one lock, inserting a not-yet-spawned registry entry. Refuses a card that
    /// is already live or a runtime that has stopped admitting.
    fn reserve(
        &self,
        card: RecordId,
    ) -> Result<
        (
            OwnedSemaphorePermit,
            mpsc::Sender<CardMessage>,
            mpsc::Receiver<CardMessage>,
        ),
        AcquireError,
    > {
        let mut inner = self.shared.lock();
        if inner.stopped {
            return Err(AcquireError::NotAdmitting);
        }
        if inner.cards.contains_key(&card) {
            return Err(AcquireError::AlreadyLive);
        }
        // Take the permit only AFTER the lease is guaranteed, so an already-live
        // acquire never transiently consumes a permit another card needs.
        let permit = self
            .shared
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| AcquireError::AtCapacity)?;
        let (inbox, inbox_rx) = mpsc::channel(INBOX_CAPACITY);
        inner.cards.insert(
            card,
            CardEntry {
                inbox: inbox.clone(),
                handle: None,
            },
        );
        Ok((permit, inbox, inbox_rx))
    }

    fn spawn_card(
        &self,
        card: RecordId,
        session: CommittedSession<P::Store>,
        initial: ApplyOutcome,
        permit: OwnedSemaphorePermit,
        inbox: mpsc::Sender<CardMessage>,
        inbox_rx: mpsc::Receiver<CardMessage>,
    ) {
        self.shared
            .observe_record(RecordUpdateKind::State, session.record().clone());
        let actor = CardActor::new(
            self.shared.clone(),
            self.executor.clone(),
            card,
            permit,
            session,
            inbox,
            inbox_rx,
            initial,
        );
        // Spawn UNDER the registry lock, and only if the entry is still present,
        // so a concurrent `shutdown` can never drain a spawned-but-unrecorded actor
        // (which would then escape the join). `spawn` only schedules the future —
        // it neither blocks nor awaits — so holding the sync guard across it is
        // safe, and never self-contends: the actor takes this lock only at its very
        // end, via `release`, long after we drop the guard.
        let mut inner = self.shared.lock();
        if inner.cards.contains_key(&card) {
            let handle = self.shared.handle.spawn(actor.run());
            inner
                .cards
                .get_mut(&card)
                .expect("the entry is present under the held lock")
                .handle = Some(handle);
        } else {
            // Shutdown drained the registry between `reserve` and here: the entry
            // is already gone and the actor was never spawned. Dropping it releases
            // the admission permit; the durable record (already committed) stays
            // authoritative and resumes via `restore` on the next start.
            drop(inner);
            drop(actor);
        }
    }
}
