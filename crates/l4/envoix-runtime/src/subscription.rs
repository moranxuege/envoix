use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use envoix_capabilities::Duty;
use envoix_product::{CapabilityAction, TransferRecord};
use envoix_types::RecordId;
use tokio::sync::Notify;

/// One process-local frontend attachment generation.
///
/// Epochs are unique within a [`Runtime`](crate::Runtime). A reattach always
/// receives a new value, so a binding can reject work drained from an older
/// attachment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubscriptionEpoch(u64);

impl SubscriptionEpoch {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SubscriptionEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Why a frontend could not attach to a card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscribeError {
    /// The runtime has no derived projection for this card. Admit or restore it
    /// first so the durable authority can seed the projection.
    UnknownCard,
    /// Shutdown has started; the runtime no longer opens subscriptions.
    RuntimeStopped,
    /// The process-local epoch counter has been exhausted.
    EpochExhausted,
}

impl fmt::Display for SubscribeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCard => formatter.write_str("the runtime has no projection for the card"),
            Self::RuntimeStopped => formatter.write_str("the runtime has stopped"),
            Self::EpochExhausted => formatter.write_str("the subscription epoch is exhausted"),
        }
    }
}

impl std::error::Error for SubscribeError {}

/// The lossless update class that could not fit in a subscriber's reserved
/// lane. The stream closes after surfacing this signal; reattach opens a fresh
/// epoch seeded from current truth and outstanding duties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LosslessUpdateKind {
    Terminal,
    CapabilityDuty,
}

/// A typed notification that a subscriber can no longer receive a complete
/// lossless tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubscriptionLag {
    pub epoch: SubscriptionEpoch,
    pub missed: LosslessUpdateKind,
}

impl fmt::Display for SubscriptionLag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "subscription epoch {} lagged before a {:?} update",
            self.epoch, self.missed
        )
    }
}

impl std::error::Error for SubscriptionLag {}

/// One typed native-side update for a card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CardUpdateKind {
    /// Current truth delivered once at the start of every epoch. While this
    /// update is pending, newer replaceable record updates refresh it in place.
    Snapshot(TransferRecord),
    /// A replaceable progress projection. Only the latest pending progress/state
    /// projection is retained for a slow subscriber.
    Progress(TransferRecord),
    /// A replaceable non-terminal state projection.
    State(TransferRecord),
    /// A lossless terminal transition.
    Terminal(TransferRecord),
    /// A lossless, idempotent capability duty.
    CapabilityDuty {
        duty: Duty,
        action: CapabilityAction,
    },
}

/// An update stamped with the attachment epoch and card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardUpdate {
    pub epoch: SubscriptionEpoch,
    pub card: RecordId,
    pub kind: CardUpdateKind,
}

/// Non-blocking receive status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TryRecvError {
    Empty,
    Closed,
    Lagged(SubscriptionLag),
}

/// A detachable, per-card frontend subscription.
///
/// Dropping this value detaches immediately: the runtime keeps only a weak
/// publisher and transfer ownership is entirely unaffected.
pub struct CardSubscription {
    epoch: SubscriptionEpoch,
    card: RecordId,
    capacity: NonZeroUsize,
    queue: Arc<SubscriberQueue>,
}

impl CardSubscription {
    pub const fn epoch(&self) -> SubscriptionEpoch {
        self.epoch
    }

    pub const fn card(&self) -> RecordId {
        self.card
    }

    pub const fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    /// Number of updates currently retained. It never exceeds [`Self::capacity`].
    pub fn pending_len(&self) -> usize {
        self.queue.lock().pending_len()
    }

    /// Waits for the next update. A typed lag closes this epoch and requires a
    /// fresh attach; `Ok(None)` means the runtime closed the subscription.
    pub async fn recv(&mut self) -> Result<Option<CardUpdate>, SubscriptionLag> {
        loop {
            let notified = self.queue.notify.notified();
            match self.queue.pop() {
                QueuePop::Update(kind) => {
                    return Ok(Some(CardUpdate {
                        epoch: self.epoch,
                        card: self.card,
                        kind: *kind,
                    }));
                }
                QueuePop::Lagged(missed) => {
                    return Err(SubscriptionLag {
                        epoch: self.epoch,
                        missed,
                    });
                }
                QueuePop::Closed => return Ok(None),
                QueuePop::Empty => notified.await,
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<CardUpdate, TryRecvError> {
        match self.queue.pop() {
            QueuePop::Update(kind) => Ok(CardUpdate {
                epoch: self.epoch,
                card: self.card,
                kind: *kind,
            }),
            QueuePop::Lagged(missed) => Err(TryRecvError::Lagged(SubscriptionLag {
                epoch: self.epoch,
                missed,
            })),
            QueuePop::Closed => Err(TryRecvError::Closed),
            QueuePop::Empty => Err(TryRecvError::Empty),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RecordUpdateKind {
    Progress,
    State,
    Terminal,
}

#[derive(Clone)]
pub(crate) struct SubscriptionPublisher {
    queue: Weak<SubscriberQueue>,
}

impl SubscriptionPublisher {
    pub(crate) fn is_attached(&self) -> bool {
        self.queue.strong_count() != 0
    }

    pub(crate) fn publish_record(&self, kind: RecordUpdateKind, record: TransferRecord) {
        let Some(queue) = self.queue.upgrade() else {
            return;
        };
        match kind {
            RecordUpdateKind::Progress => queue.push_replaceable(CardUpdateKind::Progress(record)),
            RecordUpdateKind::State => queue.push_replaceable(CardUpdateKind::State(record)),
            RecordUpdateKind::Terminal => queue.push_terminal(record),
        }
    }

    pub(crate) fn publish_duty(&self, duty: Duty, action: CapabilityAction) {
        if let Some(queue) = self.queue.upgrade() {
            queue.push_lossless(
                CardUpdateKind::CapabilityDuty { duty, action },
                LosslessUpdateKind::CapabilityDuty,
            );
        }
    }

    pub(crate) fn close(&self) {
        if let Some(queue) = self.queue.upgrade() {
            queue.close();
        }
    }
}

pub(crate) fn subscription_channel(
    epoch: SubscriptionEpoch,
    card: RecordId,
    capacity: NonZeroUsize,
    record: TransferRecord,
    duties: &[(Duty, CapabilityAction)],
) -> (SubscriptionPublisher, CardSubscription) {
    let queue = Arc::new(SubscriberQueue {
        capacity: capacity.get(),
        state: Mutex::new(QueueState {
            initial: Some(record),
            lossless: VecDeque::new(),
            latest: None,
            lagged: None,
            closed: false,
        }),
        notify: Notify::new(),
    });
    let publisher = SubscriptionPublisher {
        queue: Arc::downgrade(&queue),
    };
    let subscription = CardSubscription {
        epoch,
        card,
        capacity,
        queue,
    };
    for &(duty, action) in duties {
        publisher.publish_duty(duty, action);
    }
    (publisher, subscription)
}

struct SubscriberQueue {
    capacity: usize,
    state: Mutex<QueueState>,
    notify: Notify,
}

impl SubscriberQueue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pop(&self) -> QueuePop {
        let mut state = self.lock();
        if let Some(missed) = state.lagged.take() {
            return QueuePop::Lagged(missed);
        }
        if let Some(record) = state.initial.take() {
            return QueuePop::Update(Box::new(CardUpdateKind::Snapshot(record)));
        }
        if let Some(update) = state.lossless.pop_front() {
            return QueuePop::Update(Box::new(update));
        }
        if let Some(update) = state.latest.take() {
            return QueuePop::Update(Box::new(update));
        }
        if state.closed {
            QueuePop::Closed
        } else {
            QueuePop::Empty
        }
    }

    fn push_replaceable(&self, update: CardUpdateKind) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        if let Some(initial) = state.initial.as_mut() {
            let record = match update {
                CardUpdateKind::Progress(record) | CardUpdateKind::State(record) => record,
                _ => unreachable!("only replaceable record updates enter this path"),
            };
            *initial = record;
        } else {
            state.latest = Some(update);
        }
        drop(state);
        self.notify.notify_one();
    }

    fn push_terminal(&self, record: TransferRecord) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        if let Some(initial) = state.initial.as_mut() {
            *initial = record.clone();
        }
        // The terminal record supersedes any older replaceable progress.
        state.latest = None;
        self.push_lossless_locked(
            &mut state,
            CardUpdateKind::Terminal(record),
            LosslessUpdateKind::Terminal,
        );
        drop(state);
        self.notify.notify_one();
    }

    fn push_lossless(&self, update: CardUpdateKind, kind: LosslessUpdateKind) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        self.push_lossless_locked(&mut state, update, kind);
        drop(state);
        self.notify.notify_one();
    }

    fn push_lossless_locked(
        &self,
        state: &mut QueueState,
        update: CardUpdateKind,
        kind: LosslessUpdateKind,
    ) {
        // One slot is reserved for the initial/latest replaceable projection.
        // With capacity one, every lossless event correctly becomes typed lag.
        let lossless_capacity = self.capacity.saturating_sub(1);
        if state.lossless.len() == lossless_capacity {
            state.initial = None;
            state.lossless.clear();
            state.latest = None;
            state.lagged = Some(kind);
            state.closed = true;
        } else {
            state.lossless.push_back(update);
        }
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        drop(state);
        self.notify.notify_one();
    }
}

struct QueueState {
    initial: Option<TransferRecord>,
    lossless: VecDeque<CardUpdateKind>,
    latest: Option<CardUpdateKind>,
    lagged: Option<LosslessUpdateKind>,
    closed: bool,
}

impl QueueState {
    fn pending_len(&self) -> usize {
        usize::from(self.initial.is_some())
            + self.lossless.len()
            + usize::from(self.latest.is_some())
    }
}

enum QueuePop {
    // Boxed: `CardUpdateKind` carries a whole record, dwarfing the flag
    // variants (clippy::large_enum_variant).
    Update(Box<CardUpdateKind>),
    Lagged(LosslessUpdateKind),
    Empty,
    Closed,
}
