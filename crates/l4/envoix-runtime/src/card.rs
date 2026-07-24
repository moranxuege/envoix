use std::sync::Arc;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptStamp, AttemptSupervisor, EventAdmission,
    RetirementAckResult,
};
use envoix_product::{
    ApplyOutcome, CommandApplied, CommittedSession, IdentityError, LedgerHit, ProductCommand,
    ProductEffect, ProductInput, ProductState, Quiescence, RecordStore,
};
use envoix_types::{CommandId, RecordId};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::command::{CommandCompletion, FrontendVerdict};
use crate::error::CommandRejected;
use crate::port::{AttemptExecution, AttemptExecutor, ExecutorSignal, StopHandle};
use crate::runtime::Shared;
use crate::subscription::{RecordUpdateKind, SubscriptionEpoch};

/// Everything one card actor accepts on its single inbox: frontend commands
/// from the intake, control from the runtime API, and executor signals
/// forwarded by the per-attempt pump. Merging all onto one channel keeps the
/// actor loop a plain `recv` — no `select!`.
pub(crate) enum CardMessage {
    /// An identified, epoch-stamped mutating command from a frontend
    /// attachment. Internal inputs never use this variant.
    Frontend {
        epoch: SubscriptionEpoch,
        id: CommandId,
        command: ProductCommand,
        acceptance: oneshot::Sender<Result<FrontendVerdict, CommandRejected>>,
        completion: oneshot::Sender<CommandCompletion>,
    },
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
    // `Option` so `Drop` can free the admission permit BEFORE the registry
    // entry is removed — see the `Drop` impl.
    permit: Option<OwnedSemaphorePermit>,
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
        // store for this card while ours is still alive. Free the admission
        // permit NEXT, then remove the registry entry — the other order lets a
        // racing `reserve` on a full runtime see the slot free while the
        // semaphore is still held (a spurious final `AtCapacity`; the
        // permit-first order instead yields `AlreadyLive`, which callers retry).
        // Dropping `current` stops the executor and aborts its pump. This one
        // path covers hibernation, shutdown, AND a panic unwind through the
        // actor task.
        self.current = None;
        self.session = None;
        self.permit = None;
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
            permit: Some(permit),
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
        loop {
            // Drain everything already queued BEFORE deciding to hibernate, so
            // a command delivered to a freshly restored (or resting) card is
            // processed rather than lost to an instant at-rest exit — this is
            // what makes restore-with-queued-command lazy delivery work.
            match self.inbox_rx.try_recv() {
                Ok(message) => {
                    if self.handle(message) {
                        break;
                    }
                    continue;
                }
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            if self.at_rest() {
                break;
            }
            let Some(message) = self.inbox_rx.recv().await else {
                break;
            };
            if self.handle(message) {
                break;
            }
        }
        // Hibernation deliberately preserves the derived projection so an at-rest
        // card still renders; but a durably-removed / tombstoned card no longer
        // exists, so evict its projection before the actor drops.
        if self.session().record().facts.remove_requested {
            self.shared.evict_projection(self.card);
        }
        // Actor drops here: store closed, registry lease + admission permit freed.
    }

    /// Handles one inbox message; `true` means the actor loop must break.
    fn handle(&mut self, message: CardMessage) -> bool {
        match message {
            CardMessage::Frontend {
                epoch,
                id,
                command,
                acceptance,
                completion,
            } => {
                self.on_frontend(epoch, id, command, acceptance, completion);
                false
            }
            CardMessage::Shutdown(reply) => {
                // Stop the live worker without mutating product truth: a
                // process stop is not a transfer cancel (Pillar 7). Dropping
                // the current attempt stops its executor + pump; Restore
                // reconciles the still-non-quiescent record on the next start.
                self.current = None;
                let _ = reply.send(());
                true
            }
            CardMessage::Signal { stamp, signal } => {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|meta| meta.stamp == stamp)
                {
                    self.on_signal(stamp, signal);
                }
                false
            }
        }
    }

    /// The command intake's linearization point. Re-checks the commander epoch
    /// (a reattach between the gate and here supersedes the command), answers
    /// duplicates from the committed ledger, then sends acceptance BEFORE the
    /// commit barrier runs and the completion only after it resolves.
    fn on_frontend(
        &mut self,
        epoch: SubscriptionEpoch,
        id: CommandId,
        command: ProductCommand,
        acceptance: oneshot::Sender<Result<FrontendVerdict, CommandRejected>>,
        completion: oneshot::Sender<CommandCompletion>,
    ) {
        if self.shared.commander_epoch(self.card) != Some(epoch) {
            let _ = acceptance.send(Err(CommandRejected::Superseded));
            return;
        }
        match self
            .session()
            .record()
            .command_ledger
            .disposition(id, command)
        {
            Some(LedgerHit::Duplicate { state }) => {
                let _ = acceptance.send(Ok(FrontendVerdict::Duplicate { state }));
                let _ = completion.send(CommandCompletion::Committed { state });
                return;
            }
            Some(LedgerHit::Conflict { .. }) => {
                let _ = acceptance.send(Err(CommandRejected::Conflict));
                return;
            }
            None => {}
        }
        let _ = acceptance.send(Ok(FrontendVerdict::Accepted));
        let resolved = self
            .apply_frontend(id, command)
            .unwrap_or(CommandCompletion::Internal);
        let _ = completion.send(resolved);
    }

    /// Applies an accepted frontend command through the exactly-once barrier,
    /// then projects and dispatches exactly like any other reduction.
    fn apply_frontend(
        &mut self,
        id: CommandId,
        command: ProductCommand,
    ) -> Result<CommandCompletion, IdentityError> {
        let previous_state = self.session().record().state;
        let applied = self.session_mut().apply_command(id, command)?;
        let outcome = match applied {
            CommandApplied::Applied { outcome } => outcome,
            // The ledger was checked just above on this same single-threaded
            // actor; answering the recorded disposition keeps this total.
            CommandApplied::Duplicate { state } => {
                return Ok(CommandCompletion::Committed { state });
            }
            // Equally unreachable after the same-actor check; typed rather
            // than a lying disposition if the invariant ever breaks.
            CommandApplied::Conflict { .. } => return Ok(CommandCompletion::Internal),
        };
        let state = outcome.state;
        let committed = outcome.commit.authorizing_commit_succeeded();
        let record = self.session().record().clone();
        let update_kind = if state != previous_state && is_terminal_state(state) {
            RecordUpdateKind::Terminal
        } else {
            RecordUpdateKind::State
        };
        self.shared.observe_record(update_kind, record);
        self.dispatch(outcome);
        Ok(if committed {
            CommandCompletion::Committed { state }
        } else {
            CommandCompletion::CommitFailed { state }
        })
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
        let previous_state = self.session().record().state;
        let mut update_kind = match &input {
            ProductInput::StageProgress { .. } => RecordUpdateKind::Progress,
            ProductInput::AttemptObserved(event)
                if matches!(event.event().kind, AttemptEventKind::Progress { .. }) =>
            {
                RecordUpdateKind::Progress
            }
            ProductInput::AttemptObserved(event)
                if matches!(event.event().kind, AttemptEventKind::Terminal(_)) =>
            {
                RecordUpdateKind::Terminal
            }
            _ => RecordUpdateKind::State,
        };
        let outcome = self.session_mut().apply(input)?;
        let state = outcome.state;
        let record = self.session().record().clone();
        if state != previous_state && is_terminal_state(state) {
            update_kind = RecordUpdateKind::Terminal;
        }
        // Committed L3 truth is projected before effects are dispatched. This
        // preserves terminal-before-duty ordering without ever awaiting a
        // subscriber.
        self.shared.observe_record(update_kind, record);
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
            ProductEffect::CapabilityDuty { duty, action } => {
                self.shared.observe_duty(self.card, duty, action);
                // Executing the platform duty and admitting its result are BN /
                // host concerns. RT2 exposes the committed duty losslessly.
            }
            ProductEffect::StartConfirmTimer { .. }
            | ProductEffect::StopConfirmTimer { .. }
            | ProductEffect::StartMailboxPoll { .. }
            | ProductEffect::StopMailboxPoll { .. }
            | ProductEffect::StorageIntent { .. } => {
                // Deferred typed seams: confirm timers / mailbox poll and the
                // storage executor + P4 destructive-outbox drainer.
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

fn is_terminal_state(state: ProductState) -> bool {
    matches!(
        state,
        ProductState::Paused(_)
            | ProductState::Unconfirmed
            | ProductState::Completed
            | ProductState::Failed
            | ProductState::Cancelled
    )
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
