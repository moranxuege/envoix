use envoix_attempt_api::{AttemptEventKind, AttemptPlan, RetirementIntent};
use envoix_product::{CommittedSession, ContentHash, RecordStore, SourceStagingPlan};
use envoix_types::{ArtifactId, ByteCount, RecordId};
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

/// Establishes what a card's chosen document actually contains, injected by the
/// composition root.
///
/// The `Source` prefix is deliberate. `StagingSink` is the RECEIVE side's
/// checkpoint/append port (`envoix-transfer`), and `DutyKind::Staging` is an
/// unrelated dormant capability arm. Two collisions, avoided by naming.
///
/// The work is a read-through: for the streaming case it writes nothing at all
/// and produces a counted total and a digest, which is what makes `Ready` mean
/// "we know these bytes" rather than "we once observed a length". A copy is the
/// exceptional path — a grant a restart would lose, or a source that cannot
/// seek — and it is the same signals with bytes landing somewhere.
pub trait SourceStagingExecutor: Send + Sync + 'static {
    /// Begin establishing `plan`, returning the runtime's view of its signals.
    fn start(&self, plan: SourceStagingPlan) -> SourceStagingExecution;
}

/// A platform that stages no sources at all.
///
/// Every plan fails, immediately and honestly: this host cannot read a document,
/// so a card that reaches `Staging` on it returns to asking for one rather than
/// waiting on a worker that will never report. Shipped rather than left to each
/// caller for the reason `NoRecordStore` is — a stub every test writes for
/// itself is a stub every test can get subtly wrong, and "it never answers" is
/// the wrong one.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSourceStaging;

impl SourceStagingExecutor for NoSourceStaging {
    fn start(&self, _plan: SourceStagingPlan) -> SourceStagingExecution {
        let (signals_tx, signals) = mpsc::channel(2);
        let (stop, _token) = stop_channel();
        // Sent before the receiver is handed back, which the capacity above
        // makes non-blocking: a worker that answered nothing would leave the
        // card retiring forever.
        let _ = signals_tx.try_send(SourceStagingSignal::Failed);
        let _ = signals_tx.try_send(SourceStagingSignal::Stopped);
        SourceStagingExecution { signals, stop }
    }
}

/// The runtime's handle onto one running source-staging worker.
pub struct SourceStagingExecution {
    pub signals: mpsc::Receiver<SourceStagingSignal>,
    pub stop: StopHandle,
}

/// One observation from a source-staging worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceStagingSignal {
    /// Bytes observed so far. Coalesced by the same bounded lane the attempt's
    /// progress uses: a multi-gigabyte read must not flood anything, and the
    /// newest count is the only one that matters.
    Progress(ByteCount),
    /// The source was read through WHERE IT LIES, and nothing was copied.
    ///
    /// `total` is COUNTED, never the provider's claim, and `digest` identifies
    /// the bytes that were counted — without it "staged" would mean only "once
    /// observed a length", and a provider could swap the document across a
    /// restart.
    Streamed {
        total: ByteCount,
        digest: ContentHash,
    },
    /// The bytes were copied into an artifact this app owns outright.
    ///
    /// A separate arm from [`Self::Streamed`] because the two establish
    /// different POSSESSION, and the record says which: a card backed by an
    /// owned artifact reopens its own bytes, while a provider-backed one must
    /// revalidate a grant. One arm for both let a worker that only read the
    /// source through satisfy a copy plan, and the card then rested at `Ready`
    /// claiming an artifact nobody had written.
    ///
    /// The `ArtifactId` is NOT proof of a copy — `ArtifactId::from_bytes` is
    /// public, so any executor can name an artifact it never wrote. What this
    /// arm buys today is that a worker must STATE which operation it performed,
    /// which is enough to stop the reducer inferring possession from the plan it
    /// commissioned. Proof needs a witness the bulk store alone can mint, binding
    /// the artifact to a durable seal, and that arrives with the store.
    Copied {
        total: ByteCount,
        digest: ContentHash,
        artifact: ArtifactId,
    },
    /// The source could not be read through. Distinct from an acquisition
    /// failure: the platform DID hold it, and reading is what failed.
    Failed,
    /// The worker stopped and released its handles.
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

    /// Whether a stop has been requested, without waiting for one.
    ///
    /// A worker doing BLOCKING work — reading a multi-gigabyte file through —
    /// can only notice a stop between chunks. Awaiting would mean it noticed by
    /// finishing, which is not noticing.
    pub fn requested(&mut self) -> Option<StopSignal> {
        match self.signal.try_recv() {
            Ok(signal) => Some(signal),
            // A dropped handle is a teardown: the runtime that owned this worker
            // is gone, so stopping is the only honest thing left.
            Err(oneshot::error::TryRecvError::Closed) => Some(StopSignal::Detached),
            Err(oneshot::error::TryRecvError::Empty) => None,
        }
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
