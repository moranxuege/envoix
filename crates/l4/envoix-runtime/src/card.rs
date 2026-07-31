use std::sync::Arc;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor, EventAdmission,
    RetirementAckResult, RetirementIntent,
};
use envoix_capabilities::AdmittedSourceResult;
use envoix_outcomes::OutcomeCode;
use envoix_product::{
    AcceptedSourceOffer, ApplyOutcome, CommandApplied, CommittedSession, ContentHash,
    IdentityError, LedgerHit, ProductCommand, ProductEffect, ProductInput, ProductState,
    Quiescence, RecordStore, SourceLifecycle, SourceOfferAnswer, SourcePossession, StagedContent,
    TransferContent,
};
use envoix_types::{ByteCount, CommandId, Direction, RecordId};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::command::{CommandCompletion, FrontendVerdict};
use crate::error::CommandRejected;
use crate::launch::{AttemptLaunch, PreparedSourceResolver, SourceLocator, StagedIdentity};
use crate::port::{
    AttemptExecution, AttemptExecutor, ExecutorSignal, SourceStagingExecution,
    SourceStagingExecutor, SourceStagingSignal, StopHandle,
};
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
    /// A document offered to the acquisition this card published.
    ///
    /// Answered SYNCHRONOUSLY and once: the frontend is holding a platform
    /// resource under that key and must be told whether to release it, so
    /// there is no acceptance/completion pair here — silence would leak the
    /// pick and invite a blind retry.
    SourceOffer {
        epoch: SubscriptionEpoch,
        offer: Box<AcceptedSourceOffer>,
        answer: oneshot::Sender<Result<SourceOfferAnswer, CommandRejected>>,
    },
    /// The platform's admitted answer about an acquisition this card asked for.
    ///
    /// Internal, not frontend-originated: it carries an `AdmittedSourceResult`,
    /// which only a `DutyLedger` can mint, so there is no epoch gate and no
    /// commander check — the authority commissioned this work itself.
    SourceSettled {
        /// The admitted answer itself, by value.
        ///
        /// It used to travel in a shared cell, so that a move-only token could
        /// be handed to one of several delivery rounds and emptiness would say
        /// which round took it. That made an actor which died between taking and
        /// committing indistinguishable from one that succeeded — the cell was
        /// empty either way — and the caller reported the loss as a delivery.
        /// Exactly-once is the ledger's guarantee and the reducer's, not this
        /// message's, so a copy per round is free and can be repeated.
        result: AdmittedSourceResult,
        /// Acked once the result has been APPLIED and committed, not merely
        /// received. An ack for anything less discharges a duty whose answer
        /// nothing acted on.
        applied: oneshot::Sender<()>,
    },
    Shutdown(oneshot::Sender<()>),
    Signal {
        stamp: AttemptStamp,
        signal: ExecutorSignal,
    },
    /// One observation from the source-staging worker. Stamped like an
    /// attempt's, so a signal from a superseded generation is dropped by the
    /// same rule rather than by a second one.
    SourceStagingSignal {
        stamp: AttemptStamp,
        signal: SourceStagingSignal,
    },
}

/// The live source-staging worker's control, shaped exactly like the attempt's
/// and for the same reason: dropping it both requests the stop and aborts the
/// pump, so no forwarder outlives the worker it serves.
struct StagingMeta {
    stamp: AttemptStamp,
    stop: StopHandle,
    pump: AbortHandle,
}

impl Drop for StagingMeta {
    fn drop(&mut self) {
        self.pump.abort();
    }
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
    /// Injected as `dyn` rather than a third generic: the trait is object-safe,
    /// starting one worker per card is not a hot path, and a generic here would
    /// have rippled through every `Runtime<..>` signature in the workspace for
    /// no property the compiler was not already giving us.
    staging_executor: Arc<dyn SourceStagingExecutor>,
    /// How this card opens the source it is about to send.
    sources: Arc<dyn PreparedSourceResolver>,
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
    staging: Option<StagingMeta>,
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
    #[expect(
        clippy::too_many_arguments,
        reason = "every one is a distinct injected dependency or lease; grouping \
                  them would hide which the actor OWNS from which it borrows"
    )]
    pub(crate) fn new(
        shared: Arc<Shared>,
        executor: Arc<E>,
        staging_executor: Arc<dyn SourceStagingExecutor>,
        sources: Arc<dyn PreparedSourceResolver>,
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
            staging_executor,
            sources,
            card,
            permit: Some(permit),
            session: Some(session),
            supervisor: AttemptSupervisor::new(),
            inbox,
            inbox_rx,
            current: None,
            staging: None,
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
            CardMessage::SourceOffer {
                epoch,
                offer,
                answer,
            } => {
                self.on_source_offer(epoch, *offer, answer);
                false
            }
            CardMessage::SourceSettled { result, applied } => {
                // Acknowledged only when the answer was actually applied and
                // committed. Acknowledging regardless discharged the duty for a
                // result the card never took, and the host had no way to learn
                // it: the platform is told its work is done and the card sits in
                // `Acquiring` with nothing outstanding.
                if self.apply(ProductInput::SourceSettled(result)).is_ok() {
                    let _ = applied.send(());
                }
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
            CardMessage::SourceStagingSignal { stamp, signal } => {
                if self
                    .staging
                    .as_ref()
                    .is_some_and(|meta| meta.stamp == stamp)
                {
                    self.on_staging_signal(stamp, signal);
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
            Some(LedgerHit::Conflict { applied }) => {
                let _ = acceptance.send(Ok(FrontendVerdict::Conflict { applied }));
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

    /// The source offer's linearization point.
    ///
    /// The authority classifies the offer FIRST, on this single-threaded actor,
    /// so the answer describes the record the offer will be applied to. Only
    /// `Accepted` mutates; every other answer is the caller's to be told about,
    /// not the record's to absorb.
    ///
    /// `Accepted` is reported only after the commit holds. A frontend told its
    /// document was accepted releases nothing and waits for the card to move —
    /// so answering before the barrier would strand a pick against a card that
    /// never took it.
    fn on_source_offer(
        &mut self,
        epoch: SubscriptionEpoch,
        offer: AcceptedSourceOffer,
        answer: oneshot::Sender<Result<SourceOfferAnswer, CommandRejected>>,
    ) {
        if self.shared.commander_epoch(self.card) != Some(epoch) {
            let _ = answer.send(Err(CommandRejected::Superseded));
            return;
        }
        let classified = self.session().record().answer_source_offer(&offer);
        if classified != SourceOfferAnswer::Accepted {
            let _ = answer.send(Ok(classified));
            return;
        }
        let Ok(outcome) = self
            .session_mut()
            .apply(ProductInput::SourceOffered { offer })
        else {
            let _ = answer.send(Err(CommandRejected::Interrupted));
            return;
        };
        if !outcome.commit.authorizing_commit_succeeded() {
            let _ = answer.send(Err(CommandRejected::StorageFault));
            return;
        }
        let record = self.session().record().clone();
        self.shared.observe_record(RecordUpdateKind::State, record);
        self.dispatch(outcome);
        let _ = answer.send(Ok(SourceOfferAnswer::Accepted));
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
                let Some(launch) = self.launch(plan) else {
                    // A send whose source this process cannot open. Nothing runs
                    // — opening a transport to send bytes we do not have would
                    // spend a connection to fail on the first read — and the card
                    // is told the one thing it can act on: the source is not
                    // usable, so ask for one again. `classify_terminal` moves the
                    // lifecycle off `Ready` for exactly this code, which is what
                    // makes the offered re-pick an allowed command.
                    //
                    // Routed through `on_signal` so it takes the same supervisor
                    // admission and the same retirement handshake a real
                    // executor's terminal does; `current` stays `None`, so the
                    // retirement the reducer then asks for acks immediately.
                    self.on_signal(
                        plan.stamp,
                        ExecutorSignal::Event(AttemptEventKind::Terminal(
                            OutcomeCode::SourceUnreadable,
                        )),
                    );
                    return;
                };
                let AttemptExecution { signals, stop } = self.executor.start(launch);
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
                    Some(meta) if meta.stamp == stamp => meta.stop.stop(intent),
                    // No live worker (already gone): the ack is safe to mint now.
                    _ => self.try_ack(stamp),
                }
            }
            ProductEffect::StartSourceStaging { plan } => {
                let SourceStagingExecution { signals, stop } = self.staging_executor.start(plan);
                let pump = spawn_staging_pump(
                    &self.shared.handle,
                    self.inbox.clone(),
                    plan.stamp,
                    signals,
                );
                self.staging = Some(StagingMeta {
                    stamp: plan.stamp,
                    stop,
                    pump,
                });
            }
            ProductEffect::RetireStaging { stamp } => match self.staging.as_mut() {
                // Live worker: ask it to stop, and acknowledge only when it
                // reports `Stopped` — so the source handles are proven released
                // before the reducer sees the retirement. This used to be
                // acknowledged synchronously, which was honest only while no
                // worker existed.
                Some(meta) if meta.stamp == stamp => meta.stop.stop(RetirementIntent::Finalize),
                _ => {
                    let _ = self.apply(ProductInput::StagingRetired { stamp });
                }
            },
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

    /// One observation from the source-staging worker, as a product input.
    ///
    /// The reducer owns every guard — a stale stamp, a card that is no longer
    /// preparing, a total below the progress already reported. This only
    /// translates, and it drops the worker on `Stopped` so the retirement the
    /// reducer asked for is acknowledged with the handles proven released.
    fn on_staging_signal(&mut self, stamp: AttemptStamp, signal: SourceStagingSignal) {
        let Some(input) = (match signal {
            SourceStagingSignal::Progress(transferred) => {
                Some(ProductInput::StageProgress { stamp, transferred })
            }
            // Both completions, differing only in the POSSESSION they carry —
            // read through where it lies, or copied into an artifact this app
            // owns. It travels with the result rather than being inferred from
            // the plan the authority commissioned, because only the worker knows
            // what it actually did.
            SourceStagingSignal::Streamed { total, digest } => {
                self.stage_complete(stamp, total, digest, SourcePossession::Streamed)
            }
            // The seal IS the length and the digest, so nothing else is passed
            // alongside it to disagree with.
            SourceStagingSignal::Derived(sealed) => self.stage_complete(
                stamp,
                sealed.length(),
                sealed.digest(),
                SourcePossession::Derived(sealed),
            ),
            SourceStagingSignal::Failed => Some(ProductInput::StageFailed { stamp }),
            SourceStagingSignal::Stopped => {
                self.staging = None;
                let _ = self.apply(ProductInput::StagingRetired { stamp });
                return;
            }
        }) else {
            return;
        };
        let _ = self.apply(input);
    }

    /// One staging completion, whichever possession it achieved.
    ///
    /// `None` when the card is no longer staging: the reducer would refuse the
    /// input anyway, and the name it needs comes from the accepted offer, which
    /// only a staging card has.
    fn stage_complete(
        &self,
        stamp: AttemptStamp,
        total: ByteCount,
        digest: ContentHash,
        possession: SourcePossession,
    ) -> Option<ProductInput> {
        // The name staging establishes is the OUTPUT's, commissioned when the
        // offer was accepted. A worker does not get to name what it produced —
        // and for a derivation the output is not any one input, so inheriting
        // the first input's name would call an archive after one of its members.
        let SourceLifecycle::Staging { offer, .. } = &self.session().record().source else {
            return None;
        };
        Some(ProductInput::StageComplete {
            stamp,
            content: StagedContent::new(
                TransferContent::new(offer.output_name().clone(), total),
                digest,
            ),
            possession,
        })
    }

    /// The launch for one attempt, or `None` when a send cannot open its source.
    ///
    /// Resolved HERE, in the same step that starts the attempt, from the record
    /// this actor has already committed. Anything watching for `Ready` from
    /// outside would be reading the right fact at the wrong place: projections
    /// are drained asynchronously, so nothing orders that observation against
    /// this dispatch.
    fn launch(&self, plan: AttemptPlan) -> Option<AttemptLaunch> {
        if plan.direction == Direction::Receive {
            return AttemptLaunch::receiving(plan);
        }
        let record = self.session().record();
        let SourceLifecycle::Ready { content, .. } = &record.source else {
            return None;
        };
        let identity = StagedIdentity {
            total: content.total(),
            digest: content.content_hash(),
        };
        let locator = SourceLocator::of(&record.source)?;
        let source = self.sources.resolve(locator, identity).ok()?;
        AttemptLaunch::sending(plan, source)
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
fn spawn_staging_pump(
    handle: &Handle,
    inbox: mpsc::Sender<CardMessage>,
    stamp: AttemptStamp,
    mut signals: mpsc::Receiver<SourceStagingSignal>,
) -> AbortHandle {
    handle
        .spawn(async move {
            while let Some(signal) = signals.recv().await {
                if inbox
                    .send(CardMessage::SourceStagingSignal { stamp, signal })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
        .abort_handle()
}

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
