use envoix_attempt_api::{AttemptEventKind, AttemptPlan, RetirementIntent};
use envoix_product::{CommittedSession, RecordStore};
use envoix_types::RecordId;
use tokio::sync::{mpsc, oneshot};

/// Restores the durable authority for a card from the operation store.
///
/// The runtime owns no transfer truth: it obtains a card's `CommittedSession`
/// (record + record-store) only through this port, so the durable store stays
/// the single source of truth. The concrete op-store binding lives in the
/// composition root (an L2 concern), never in this crate.
pub trait SessionProvider: Send + Sync + 'static {
    /// The record-store bound to one card's durable operation store.
    type Store: RecordStore + Send + 'static;

    /// The durable session for `card`, or `None` if the card is absent.
    fn restore(&self, card: RecordId) -> Option<CommittedSession<Self::Store>>;
}

/// Drives one transport-independent attempt, injected by the composition root.
///
/// RT1 owns the C7 `AttemptSupervisor`; the executor only produces raw signals.
/// The real iroh executor is wired at the host — see the crate docs.
pub trait AttemptExecutor: Send + Sync + 'static {
    /// Begin executing `plan`, returning the runtime's view of its signals.
    fn start(&self, plan: AttemptPlan) -> AttemptExecution;
}

/// The runtime's handle onto one running attempt.
pub struct AttemptExecution {
    /// Signals the executor emits until it stops.
    pub signals: mpsc::Receiver<ExecutorSignal>,
    /// Requests the executor stop and release its lease and handles.
    pub stop: StopHandle,
}

/// One observation from an executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorSignal {
    /// A phase / progress / terminal observation for the reducer. A `Terminal`
    /// also records the outcome with the supervisor so a later retirement can
    /// resolve to the true outcome.
    Event(AttemptEventKind),
    /// The executor crossed its single irreversible commit point.
    CommitCrossed,
    /// The executor stopped and released its lease and handles.
    Stopped,
}

/// Why an executor is being stopped.
///
/// A process/card teardown is NOT a transfer cancellation: the two must stay
/// distinguishable at the executor, or shutting the host down would tell a
/// live transport to discard resumable state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopSignal {
    /// The reducer authorized this retirement (pause, cancel, or finalize).
    Retire(RetirementIntent),
    /// The card actor or the process is going away with no authorized
    /// retirement: stop, but preserve everything resumable.
    Detached,
}

/// Requests an executor stop. Dropping it also requests a stop, so a torn-down
/// card never leaves its executor running.
#[derive(Debug)]
pub struct StopHandle {
    signal: Option<oneshot::Sender<StopSignal>>,
}

impl StopHandle {
    /// Requests the paired executor stop with the reducer-authorized intent.
    /// Idempotent.
    pub fn stop(&mut self, intent: RetirementIntent) {
        self.send(StopSignal::Retire(intent));
    }

    fn send(&mut self, signal: StopSignal) {
        if let Some(sender) = self.signal.take() {
            let _ = sender.send(signal);
        }
    }
}

impl Drop for StopHandle {
    fn drop(&mut self) {
        self.send(StopSignal::Detached);
    }
}

/// The executor's side of a stop channel.
#[derive(Debug)]
pub struct StopToken {
    signal: oneshot::Receiver<StopSignal>,
}

impl StopToken {
    /// Resolves to the requested stop. A dropped runtime handle is a teardown,
    /// never an authorized cancellation.
    pub async fn stopped(self) -> StopSignal {
        self.signal.await.unwrap_or(StopSignal::Detached)
    }
}

/// Creates a paired stop handle (runtime side) and token (executor side).
pub fn stop_channel() -> (StopHandle, StopToken) {
    let (sender, receiver) = oneshot::channel();
    (
        StopHandle {
            signal: Some(sender),
        },
        StopToken { signal: receiver },
    )
}
