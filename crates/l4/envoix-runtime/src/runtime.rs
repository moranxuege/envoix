use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use envoix_product::{
    ApplyOutcome, CommittedSession, ProductCommand, ProductInput, ProductState, TransferRecord,
};
use envoix_types::RecordId;
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::card::{CardActor, CardMessage};
use crate::config::RuntimeConfig;
use crate::error::{AcquireError, CommandError};
use crate::port::{AttemptExecutor, SessionProvider};

const INBOX_CAPACITY: usize = 64;

/// A live card's registry slot: its inbox, and — once spawned — its supervised
/// task handle. Holding the handle here (rather than a side list) makes shutdown
/// draining atomic: whatever the registry holds is exactly what shutdown joins.
struct CardEntry {
    inbox: mpsc::Sender<CardMessage>,
    handle: Option<JoinHandle<()>>,
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
    cards: HashMap<RecordId, CardEntry>,
}

/// Generic-free runtime state, shared by every card actor via `Arc` so an actor
/// can release its own lease without knowing the port types.
pub(crate) struct Shared {
    pub(crate) config: RuntimeConfig,
    pub(crate) handle: Handle,
    inner: Mutex<Inner>,
    admission: Arc<Semaphore>,
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
        let admission = Arc::new(Semaphore::new(config.max_live_cards.get()));
        Self {
            shared: Arc::new(Shared {
                config,
                handle: Handle::current(),
                inner: Mutex::new(Inner {
                    stopped: false,
                    cards: HashMap::new(),
                }),
                admission,
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

    /// Delivers a command to a live card. Errors if the card is not live; the
    /// caller restores it first (lazy restore is an explicit step).
    pub async fn command(
        &self,
        card: RecordId,
        command: ProductCommand,
    ) -> Result<ProductState, CommandError> {
        let inbox = self.inbox(card).ok_or(CommandError::NotLive)?;
        let (reply, response) = oneshot::channel();
        inbox
            .send(CardMessage::Command(command, reply))
            .await
            .map_err(|_| CommandError::NotLive)?;
        response.await.map_err(|_| CommandError::NotLive)?
    }

    /// A derived read snapshot of a live card's record, or `None` if not live.
    pub async fn snapshot(&self, card: RecordId) -> Option<TransferRecord> {
        let inbox = self.inbox(card)?;
        let (reply, response) = oneshot::channel();
        inbox.send(CardMessage::Snapshot(reply)).await.ok()?;
        response.await.ok()
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
        let drained: Vec<CardEntry> = {
            let mut inner = self.shared.lock();
            inner.stopped = true;
            inner.cards.drain().map(|(_, entry)| entry).collect()
        };
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
