use std::sync::Arc;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptStamp, AttemptSupervisor, EventAdmission,
    RetirementAckResult,
};
use envoix_product::{
    ApplyOutcome, CommittedSession, IdentityError, ProductCommand, ProductEffect, ProductInput,
    ProductState, Quiescence, RecordStore, TransferRecord,
};
use envoix_types::RecordId;
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::error::CommandError;
use crate::port::{AttemptExecution, AttemptExecutor, ExecutorSignal, StopHandle};
use crate::runtime::Shared;

/// Everything one card actor accepts on its single inbox: control from the
/// runtime API, and executor signals forwarded by the per-attempt pump. Merging
/// both onto one channel keeps the actor loop a plain `recv` — no `select!`.
pub(crate) enum CardMessage {
    Command(
        ProductCommand,
        oneshot::Sender<Result<ProductState, CommandError>>,
    ),
    Snapshot(oneshot::Sender<TransferRecord>),
    Shutdown(oneshot::Sender<()>),
    Signal {
        stamp: AttemptStamp,
        signal: ExecutorSignal,
    },
}

/// The live attempt's control: its stop handle plus the pump task forwarding its
/// signals. Dropping it — on stop, supersession, or actor teardown — both
/// requests the executor stop (via `stop`) AND aborts the pump, so no forwarder
/// task ever outlives the attempt it serves (even an executor that ignores stop).
struct AttemptMeta {
    stamp: AttemptStamp,
    stop: StopHandle,
    pump: AbortHandle,
}

impl Drop for AttemptMeta {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// One card's supervised owner. Holds the durable session, the C7 supervisor,
/// and the current attempt's control. On drop (normal exit OR panic unwind) it
/// closes the store, stops the attempt, and releases the registry lease + permit.
pub(crate) struct CardActor<R: RecordStore, E: AttemptExecutor> {
    shared: Arc<Shared>,
    executor: Arc<E>,
    card: RecordId,
    _permit: OwnedSemaphorePermit,
    // `Option` so `Drop` can close the store (releasing its backend write lease)
    // BEFORE the registry entry is removed — see the `Drop` impl.
    session: Option<CommittedSession<R>>,
    supervisor: AttemptSupervisor,
    inbox: mpsc::Sender<CardMessage>,
    inbox_rx: mpsc::Receiver<CardMessage>,
    current: Option<AttemptMeta>,
    initial: Option<ApplyOutcome>,
}

impl<R: RecordStore, E: AttemptExecutor> Drop for CardActor<R, E> {
    fn drop(&mut self) {
        // Ordering matters. Close the card's durable store FIRST (dropping the
        // session releases its per-instance backend write lease), so a concurrent
        // `restore` that observes the freed registry slot cannot open a second
        // store for this card while ours is still alive. Then remove the registry
        // entry. Dropping `current` stops the executor and aborts its pump;
        // `_permit` frees the admission permit. This one path covers hibernation,
        // shutdown, AND a panic unwind through the actor task.
        self.current = None;
        self.session = None;
        self.shared.release(self.card);
    }
}

impl<R: RecordStore + Send + 'static, E: AttemptExecutor> CardActor<R, E> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        shared: Arc<Shared>,
        executor: Arc<E>,
        card: RecordId,
        permit: OwnedSemaphorePermit,
        session: CommittedSession<R>,
        inbox: mpsc::Sender<CardMessage>,
        inbox_rx: mpsc::Receiver<CardMessage>,
        initial: ApplyOutcome,
    ) -> Self {
        Self {
            shared,
            executor,
            card,
            _permit: permit,
            session: Some(session),
            supervisor: AttemptSupervisor::new(),
            inbox,
            inbox_rx,
            current: None,
            initial: Some(initial),
        }
    }

    pub(crate) async fn run(mut self) {
        if let Some(initial) = self.initial.take() {
            self.dispatch(initial);
        }
        while !self.at_rest() {
            let Some(message) = self.inbox_rx.recv().await else {
                break;
            };
            match message {
                CardMessage::Command(command, reply) => {
                    let result = self
                        .apply(ProductInput::Command(command))
                        .map_err(|_| CommandError::Internal);
                    let _ = reply.send(result);
                }
                CardMessage::Snapshot(reply) => {
                    let _ = reply.send(self.session().record().clone());
                }
                CardMessage::Shutdown(reply) => {
                    // Stop the live worker without mutating product truth: a
                    // process stop is not a transfer cancel (Pillar 7). Dropping
                    // the current attempt stops its executor + pump; Restore
                    // reconciles the still-non-quiescent record on the next start.
                    self.current = None;
                    let _ = reply.send(());
                    break;
                }
                CardMessage::Signal { stamp, signal } => {
                    if self
                        .current
                        .as_ref()
                        .is_some_and(|meta| meta.stamp == stamp)
                    {
                        self.on_signal(stamp, signal);
                    }
                }
            }
        }
        // Actor drops here: store closed, registry lease + admission permit freed.
    }

    /// A card at a quiescent resting state with no live worker needs no actor:
    /// its truth is durable. Exiting the loop hibernates it (lazy re-restore on
    /// the next reference).
    fn at_rest(&self) -> bool {
        self.current.is_none()
            && matches!(self.session().record().quiescence, Quiescence::Quiescent)
    }

    fn session(&self) -> &CommittedSession<R> {
        self.session
            .as_ref()
            .expect("the session is present for the actor's whole run")
    }

    fn session_mut(&mut self) -> &mut CommittedSession<R> {
        self.session
            .as_mut()
            .expect("the session is present for the actor's whole run")
    }

    fn apply(&mut self, input: ProductInput) -> Result<ProductState, IdentityError> {
        let outcome = self.session_mut().apply(input)?;
        let state = outcome.state;
        self.dispatch(outcome);
        Ok(state)
    }

    fn dispatch(&mut self, outcome: ApplyOutcome) {
        for effect in outcome
            .released_immediately
            .into_iter()
            .chain(outcome.released_after_commit)
        {
            self.on_effect(effect);
        }
    }

    fn on_effect(&mut self, effect: ProductEffect) {
        match effect {
            ProductEffect::StartAttempt { plan } => {
                // The runtime owns the supervisor and does the linearization.
                let _ = self.supervisor.open(plan);
                let AttemptExecution { signals, stop } = self.executor.start(plan);
                let pump = spawn_pump(&self.shared.handle, self.inbox.clone(), plan.stamp, signals);
                self.current = Some(AttemptMeta {
                    stamp: plan.stamp,
                    stop,
                    pump,
                });
            }
            ProductEffect::RetireAttempt { stamp, intent } => {
                let _ = self.supervisor.request_retirement(stamp, intent);
                match self.current.as_mut() {
                    // Live worker: ask it to stop; ack when it reports Stopped so
                    // the lease is proven released before the reducer sees the ack.
                    Some(meta) if meta.stamp == stamp => meta.stop.stop(),
                    // No live worker (already gone): the ack is safe to mint now.
                    _ => self.try_ack(stamp),
                }
            }
            ProductEffect::RetireStaging { stamp } => {
                // RT1 runs no separate staging worker, so its retirement is
                // acknowledged synchronously. A real staging executor is deferred.
                let _ = self.apply(ProductInput::StagingRetired { stamp });
            }
            ProductEffect::StartConfirmTimer { .. }
            | ProductEffect::StopConfirmTimer { .. }
            | ProductEffect::StartMailboxPoll { .. }
            | ProductEffect::StopMailboxPoll { .. }
            | ProductEffect::CapabilityDuty { .. }
            | ProductEffect::StorageIntent { .. } => {
                // Deferred typed seams: confirm timers / mailbox poll (RT2 + P6
                // integration) and the capability / storage executors + the P4
                // destructive-outbox drainer (BN / a dedicated RT slice). RT1 owns
                // lifetime, not these mechanisms.
            }
        }
    }

    fn on_signal(&mut self, stamp: AttemptStamp, signal: ExecutorSignal) {
        match signal {
            ExecutorSignal::Event(kind) => {
                if let AttemptEventKind::Terminal(code) = kind {
                    // Record the terminal so a later Finalize retirement resolves
                    // to the TRUE outcome instead of mistranslating to cancel.
                    let _ = self.supervisor.resolve_terminal(stamp, code);
                }
                if let EventAdmission::Accepted(admitted) =
                    self.supervisor.observe(AttemptEvent { stamp, kind })
                {
                    let _ = self.apply(ProductInput::AttemptObserved(admitted));
                }
            }
            ExecutorSignal::CommitCrossed => {
                let _ = self.supervisor.cross_commit_point(stamp);
            }
            ExecutorSignal::Stopped => {
                self.current = None;
                self.try_ack(stamp);
            }
        }
    }

    fn try_ack(&mut self, stamp: AttemptStamp) {
        if let RetirementAckResult::Acknowledged(ack) =
            self.supervisor.acknowledge_retirement(stamp)
        {
            let _ = self.apply(ProductInput::AttemptRetired(ack));
        }
    }
}

/// Forwards one attempt's executor signals onto the actor's inbox, tagged with
/// the attempt stamp so a superseded attempt's stragglers are ignored. Returns
/// the pump's abort handle so its owning [`AttemptMeta`] can guarantee teardown
/// even if the executor never closes its signal stream.
fn spawn_pump(
    handle: &Handle,
    inbox: mpsc::Sender<CardMessage>,
    stamp: AttemptStamp,
    mut signals: mpsc::Receiver<ExecutorSignal>,
) -> AbortHandle {
    handle
        .spawn(async move {
            while let Some(signal) = signals.recv().await {
                if inbox
                    .send(CardMessage::Signal { stamp, signal })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
        .abort_handle()
}
