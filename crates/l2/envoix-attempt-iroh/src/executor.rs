use std::sync::{Arc, Mutex};
use std::time::Duration;

use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, AttemptSupervisor,
    CommitOperationResult, OpenResult, RetirementAck, RetirementAckResult, RetirementIntent,
    RetirementRequestResult, TerminalResolutionResult,
};
use envoix_auth::{
    self, AuthError, ExportedKeyingMaterial, MAX_AUTH_PAYLOAD, MonotonicMillis as AuthMillis,
};
use envoix_outcomes::{OutcomeCode, Phase};
use envoix_pairing::{DataPlaneToken, EntropySource};
use envoix_protocol::{
    Abort, ContentHash, Frame, MAX_FRAME_SIZE, ProtocolReason, ResumeMode, decode_frame,
    encode_frame,
};
use envoix_session_iroh::{
    AuthFailureBudget, CloseOrdering, IrohListener, PathObservation, SessionCancellation,
    SessionError, SessionLink, SessionTimeouts,
};
use envoix_transfer::{
    self, ClaimedComplete, MachineFailure, MonotonicMillis as TransferMillis, ReceiverStep,
    SenderRequest, SenderStep, SourceReader, StagingSink, TransferError,
};
use envoix_types::{ByteCount, Direction, OfferedName};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::AttemptError;

pub type SharedAttemptSupervisor = Arc<Mutex<AttemptSupervisor>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttemptTimeouts {
    authentication: Duration,
    transfer_idle: Duration,
    completion_ack: Duration,
    session: SessionTimeouts,
}

impl AttemptTimeouts {
    pub fn new(
        authentication: Duration,
        transfer_idle: Duration,
        completion_ack: Duration,
        session: SessionTimeouts,
    ) -> Result<Self, AttemptError> {
        if authentication.is_zero() || transfer_idle.is_zero() || completion_ack.is_zero() {
            return Err(AttemptError::InvalidTimeout);
        }
        Ok(Self {
            authentication,
            transfer_idle,
            completion_ack,
            session,
        })
    }

    pub const fn authentication(self) -> Duration {
        self.authentication
    }

    pub const fn transfer_idle(self) -> Duration {
        self.transfer_idle
    }

    pub const fn completion_ack(self) -> Duration {
        self.completion_ack
    }

    pub const fn session(self) -> SessionTimeouts {
        self.session
    }
}

#[derive(Clone, Debug)]
pub struct AttemptTransferSpec {
    pub offered_name: OfferedName,
    pub file_size: ByteCount,
    pub chunk_size: ByteCount,
    /// The receiver's durable claim about a prior run. Receive only.
    pub claimed_complete: Option<ClaimedComplete>,
    /// What the source must hash to — staging's own digest. Send only, and
    /// required for a send: without it the sender declares the hash of whatever
    /// it happened to read, so a swapped document completes as if it were the
    /// chosen one. A send that cannot state it does not start.
    pub content_hash: Option<ContentHash>,
    pub timeouts: AttemptTimeouts,
}

#[derive(Clone)]
pub struct AttemptControl {
    stamp: AttemptStamp,
    supervisor: SharedAttemptSupervisor,
    stop_sender: mpsc::UnboundedSender<RetirementIntent>,
    retirement_changed: Arc<Notify>,
}

impl AttemptControl {
    pub fn request(
        &self,
        intent: RetirementIntent,
    ) -> Result<RetirementRequestResult, AttemptError> {
        let result = self
            .supervisor
            .lock()
            .map_err(|_| AttemptError::SupervisorPoisoned)?
            .request_retirement(self.stamp, intent);
        if matches!(
            result,
            RetirementRequestResult::Requested | RetirementRequestResult::AlreadyRequested
        ) {
            if matches!(intent, RetirementIntent::Pause | RetirementIntent::Cancel) {
                let _ = self.stop_sender.send(intent);
            }
            self.retirement_changed.notify_waiters();
        }
        Ok(result)
    }
}

pub struct AttemptHandle {
    open_result: OpenResult,
    control: AttemptControl,
    events: mpsc::UnboundedReceiver<AttemptEvent>,
    paths: mpsc::UnboundedReceiver<PathObservation>,
    task: Option<JoinHandle<Result<RetirementAck, AttemptError>>>,
}

impl AttemptHandle {
    pub const fn open_result(&self) -> OpenResult {
        self.open_result
    }

    pub fn control(&self) -> AttemptControl {
        self.control.clone()
    }

    pub async fn next_event(&mut self) -> Option<AttemptEvent> {
        self.events.recv().await
    }

    pub async fn next_path(&mut self) -> Option<PathObservation> {
        self.paths.recv().await
    }

    pub async fn wait_ack(&mut self) -> Result<RetirementAck, AttemptError> {
        let task = self.task.take().ok_or(AttemptError::TaskStopped)?;
        task.await.map_err(|_| AttemptError::TaskStopped)?
    }
}

impl Drop for AttemptHandle {
    fn drop(&mut self) {
        if self.task.is_some() {
            let _ = self.control.request(RetirementIntent::Cancel);
        }
    }
}

pub fn spawn_sender<L, S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    source: S,
    mut link: L,
    supervisor: SharedAttemptSupervisor,
    entropy: E,
) -> Result<AttemptHandle, AttemptError>
where
    L: SessionLink + 'static,
    S: SourceReader + Send + 'static,
    E: EntropySource + Send + 'static,
{
    if plan.direction != Direction::Send {
        return Err(AttemptError::WrongDirection);
    }
    let open_result = open_attempt(&supervisor, plan)?;
    let paths = link.take_path_observations();
    let (events_sender, events) = mpsc::unbounded_channel();
    let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
    let retirement_changed = Arc::new(Notify::new());
    let control = AttemptControl {
        stamp: plan.stamp,
        supervisor: supervisor.clone(),
        stop_sender,
        retirement_changed: retirement_changed.clone(),
    };
    let task = tokio::spawn(async move {
        execute_sender(
            plan,
            spec,
            token,
            source,
            Box::new(link),
            entropy,
            supervisor,
            events_sender,
            stop_receiver,
            retirement_changed,
        )
        .await
    });
    Ok(AttemptHandle {
        open_result,
        control,
        events,
        paths,
        task: Some(task),
    })
}

pub fn spawn_receiver<L, S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    sink: S,
    mut link: L,
    supervisor: SharedAttemptSupervisor,
    entropy: E,
) -> Result<AttemptHandle, AttemptError>
where
    L: SessionLink + 'static,
    S: StagingSink + Send + 'static,
    E: EntropySource + Send + 'static,
{
    if plan.direction != Direction::Receive {
        return Err(AttemptError::WrongDirection);
    }
    let open_result = open_attempt(&supervisor, plan)?;
    let paths = link.take_path_observations();
    let (events_sender, events) = mpsc::unbounded_channel();
    let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
    let retirement_changed = Arc::new(Notify::new());
    let control = AttemptControl {
        stamp: plan.stamp,
        supervisor: supervisor.clone(),
        stop_sender,
        retirement_changed: retirement_changed.clone(),
    };
    let task = tokio::spawn(async move {
        execute_receiver(
            plan,
            spec,
            token,
            sink,
            Box::new(link),
            entropy,
            supervisor,
            events_sender,
            stop_receiver,
            retirement_changed,
        )
        .await
    });
    Ok(AttemptHandle {
        open_result,
        control,
        events,
        paths,
        task: Some(task),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_iroh_receiver<S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    sink: S,
    listener: IrohListener,
    auth_failures: AuthFailureBudget,
    supervisor: SharedAttemptSupervisor,
    entropy: E,
) -> Result<AttemptHandle, AttemptError>
where
    S: StagingSink + Send + 'static,
    E: EntropySource + Send + 'static,
{
    if plan.direction != Direction::Receive {
        return Err(AttemptError::WrongDirection);
    }
    let open_result = open_attempt(&supervisor, plan)?;
    let (paths_sender, paths) = mpsc::unbounded_channel();
    let (events_sender, events) = mpsc::unbounded_channel();
    let (stop_sender, stop_receiver) = mpsc::unbounded_channel();
    let retirement_changed = Arc::new(Notify::new());
    let control = AttemptControl {
        stamp: plan.stamp,
        supervisor: supervisor.clone(),
        stop_sender,
        retirement_changed: retirement_changed.clone(),
    };
    let task = tokio::spawn(async move {
        execute_iroh_receiver(
            plan,
            spec,
            token,
            sink,
            listener,
            auth_failures,
            entropy,
            supervisor,
            events_sender,
            paths_sender,
            stop_receiver,
            retirement_changed,
        )
        .await
    });
    Ok(AttemptHandle {
        open_result,
        control,
        events,
        paths,
        task: Some(task),
    })
}

fn open_attempt(
    supervisor: &SharedAttemptSupervisor,
    plan: AttemptPlan,
) -> Result<OpenResult, AttemptError> {
    let result = supervisor
        .lock()
        .map_err(|_| AttemptError::SupervisorPoisoned)?
        .open(plan);
    if matches!(result, OpenResult::Opened | OpenResult::Superseded) {
        Ok(result)
    } else {
        Err(AttemptError::CannotOpen(result))
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_sender<S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    mut source: S,
    mut link: Box<dyn SessionLink>,
    mut entropy: E,
    supervisor: SharedAttemptSupervisor,
    events: mpsc::UnboundedSender<AttemptEvent>,
    mut stop: mpsc::UnboundedReceiver<RetirementIntent>,
    retirement_changed: Arc<Notify>,
) -> Result<RetirementAck, AttemptError>
where
    S: SourceReader + Send + 'static,
    E: EntropySource + Send + 'static,
{
    let clock = AttemptClock::new();
    emit(
        &events,
        plan.stamp,
        AttemptEventKind::Phase(Phase::Authenticating),
    );
    let terminal = match authenticate_sender(
        &mut *link,
        &token,
        &clock,
        spec.timeouts,
        &mut stop,
        &mut entropy,
    )
    .await
    {
        Ok(()) => {
            emit(
                &events,
                plan.stamp,
                AttemptEventKind::Phase(Phase::Transferring),
            );
            transfer_sender(
                plan,
                &spec,
                &mut source,
                &mut *link,
                &clock,
                &supervisor,
                &events,
                &mut stop,
            )
            .await
        }
        Err(terminal) => terminal,
    };
    finish_attempt(
        plan.stamp,
        terminal,
        link,
        source,
        token,
        supervisor,
        events,
        retirement_changed,
        spec.timeouts.session(),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_receiver<S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    mut sink: S,
    mut link: Box<dyn SessionLink>,
    mut entropy: E,
    supervisor: SharedAttemptSupervisor,
    events: mpsc::UnboundedSender<AttemptEvent>,
    mut stop: mpsc::UnboundedReceiver<RetirementIntent>,
    retirement_changed: Arc<Notify>,
) -> Result<RetirementAck, AttemptError>
where
    S: StagingSink + Send + 'static,
    E: EntropySource + Send + 'static,
{
    let clock = AttemptClock::new();
    emit(
        &events,
        plan.stamp,
        AttemptEventKind::Phase(Phase::Authenticating),
    );
    let terminal = match authenticate_receiver(
        &mut *link,
        &token,
        &clock,
        spec.timeouts,
        &mut stop,
        &mut entropy,
    )
    .await
    {
        Ok(()) => {
            emit(
                &events,
                plan.stamp,
                AttemptEventKind::Phase(Phase::Transferring),
            );
            transfer_receiver(
                plan,
                &spec,
                &mut sink,
                &mut *link,
                &clock,
                &supervisor,
                &events,
                &mut stop,
            )
            .await
        }
        Err(terminal) => terminal,
    };
    finish_attempt(
        plan.stamp,
        terminal,
        link,
        sink,
        token,
        supervisor,
        events,
        retirement_changed,
        spec.timeouts.session(),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_iroh_receiver<S, E>(
    plan: AttemptPlan,
    spec: AttemptTransferSpec,
    token: DataPlaneToken,
    mut sink: S,
    listener: IrohListener,
    mut auth_failures: AuthFailureBudget,
    mut entropy: E,
    supervisor: SharedAttemptSupervisor,
    events: mpsc::UnboundedSender<AttemptEvent>,
    paths: mpsc::UnboundedSender<PathObservation>,
    mut stop: mpsc::UnboundedReceiver<RetirementIntent>,
    retirement_changed: Arc<Notify>,
) -> Result<RetirementAck, AttemptError>
where
    S: StagingSink + Send + 'static,
    E: EntropySource + Send + 'static,
{
    let clock = AttemptClock::new();
    let cancellation = SessionCancellation::new();
    emit(
        &events,
        plan.stamp,
        AttemptEventKind::Phase(Phase::Authenticating),
    );

    let mut listener = Some(listener);
    let mut link = loop {
        let active_listener = listener
            .as_ref()
            .expect("listener remains present until authentication succeeds");
        let candidate = tokio::select! {
            biased;
            intent = stop.recv() => {
                let terminal = Terminal::retired(
                    intent.unwrap_or(RetirementIntent::Cancel)
                );
                let listener = listener.take().expect("listener is present");
                let _ = listener.close(spec.timeouts.session()).await;
                drop(paths);
                return finish_released_attempt(
                    plan.stamp,
                    terminal.outcome,
                    sink,
                    token,
                    supervisor,
                    events,
                    retirement_changed,
                ).await;
            }
            result = active_listener.accept_candidate(
                &cancellation,
                spec.timeouts.session(),
            ) => match result {
                Ok(candidate) => candidate,
                Err(error) => {
                    let terminal = Terminal::from_error(AttemptError::Session(error));
                    let listener = listener.take().expect("listener is present");
                    let _ = listener.close(spec.timeouts.session()).await;
                    drop(paths);
                    return finish_released_attempt(
                        plan.stamp,
                        terminal.outcome,
                        sink,
                        token,
                        supervisor,
                        events,
                        retirement_changed,
                    ).await;
                }
            }
        };
        let mut candidate = candidate;
        match authenticate_receiver(
            &mut candidate,
            &token,
            &clock,
            spec.timeouts,
            &mut stop,
            &mut entropy,
        )
        .await
        {
            Ok(()) => {
                let listener = listener.take().expect("listener is present");
                break Box::new(listener.promote(candidate)) as Box<dyn SessionLink>;
            }
            Err(terminal)
                if matches!(
                    terminal.outcome,
                    OutcomeCode::Cancelled | OutcomeCode::Paused
                ) =>
            {
                let _ = candidate
                    .close(CloseOrdering::Active, spec.timeouts.session())
                    .await;
                let listener = listener.take().expect("listener is present");
                let _ = listener.close(spec.timeouts.session()).await;
                drop(paths);
                return finish_released_attempt(
                    plan.stamp,
                    terminal.outcome,
                    sink,
                    token,
                    supervisor,
                    events,
                    retirement_changed,
                )
                .await;
            }
            Err(_) => {
                let _ = candidate
                    .close(CloseOrdering::Active, spec.timeouts.session())
                    .await;
                if !auth_failures.record_failure() {
                    let listener = listener.take().expect("listener is present");
                    let _ = listener.close(spec.timeouts.session()).await;
                    drop(paths);
                    return finish_released_attempt(
                        plan.stamp,
                        OutcomeCode::Unauthenticated,
                        sink,
                        token,
                        supervisor,
                        events,
                        retirement_changed,
                    )
                    .await;
                }
            }
        }
    };

    let mut observations = link.take_path_observations();
    let path_forwarder = tokio::spawn(async move {
        while let Some(observation) = observations.recv().await {
            if paths.send(observation).is_err() {
                break;
            }
        }
    });
    emit(
        &events,
        plan.stamp,
        AttemptEventKind::Phase(Phase::Transferring),
    );
    let terminal = transfer_receiver(
        plan,
        &spec,
        &mut sink,
        &mut *link,
        &clock,
        &supervisor,
        &events,
        &mut stop,
    )
    .await;
    finish_attempt(
        plan.stamp,
        terminal,
        link,
        sink,
        token,
        supervisor,
        events,
        retirement_changed,
        spec.timeouts.session(),
        Some(path_forwarder),
    )
    .await
}

async fn authenticate_sender(
    link: &mut dyn SessionLink,
    token: &DataPlaneToken,
    clock: &AttemptClock,
    timeouts: AttemptTimeouts,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
    entropy: &mut impl EntropySource,
) -> Result<(), Terminal> {
    let binding = channel_binding(link).map_err(Terminal::from_error)?;
    let deadline = auth_deadline(clock, timeouts.authentication());
    let (await_response, start) = envoix_auth::sender_start(token, binding, deadline, entropy)
        .map_err(|error| Terminal::from_error(AttemptError::Authentication(error)))?;
    if let Err(exit) =
        send_interruptible(link, &start, stop, clock.remaining(deadline.instant().0)).await
    {
        return Err(auth_wait_terminal(await_response.cancel(), exit));
    }

    let response = match receive_interruptible(
        link,
        MAX_AUTH_PAYLOAD,
        stop,
        clock.remaining(deadline.instant().0),
    )
    .await
    {
        Ok(packet) => packet,
        Err(WaitExit::Stop(intent)) => {
            let _ = await_response.cancel();
            return Err(Terminal::retired(intent));
        }
        Err(WaitExit::Timeout) => {
            let error = expired_auth(await_response.deadline_exceeded(AuthMillis(u64::MAX)));
            return Err(Terminal::from_auth(error));
        }
        Err(WaitExit::Session(SessionError::PeerClosed)) => {
            return Err(Terminal::from_auth(await_response.peer_closed()));
        }
        Err(WaitExit::Session(error)) => {
            return Err(Terminal::from_error(AttemptError::Session(error)));
        }
        Err(WaitExit::Protocol) => {
            return Err(Terminal::from_error(AttemptError::ProtocolEnvelope));
        }
    };
    let (await_confirm, confirmation) = await_response
        .receive_response(&response, clock.auth_now())
        .map_err(Terminal::from_auth)?;
    if let Err(exit) = send_interruptible(
        link,
        &confirmation,
        stop,
        clock.remaining(deadline.instant().0),
    )
    .await
    {
        return Err(auth_wait_terminal(await_confirm.cancel(), exit));
    }

    let confirmation = match receive_interruptible(
        link,
        MAX_AUTH_PAYLOAD,
        stop,
        clock.remaining(deadline.instant().0),
    )
    .await
    {
        Ok(packet) => packet,
        Err(WaitExit::Stop(intent)) => {
            let _ = await_confirm.cancel();
            return Err(Terminal::retired(intent));
        }
        Err(WaitExit::Timeout) => {
            let error = expired_auth(await_confirm.deadline_exceeded(AuthMillis(u64::MAX)));
            return Err(Terminal::from_auth(error));
        }
        Err(WaitExit::Session(SessionError::PeerClosed)) => {
            return Err(Terminal::from_auth(await_confirm.peer_closed()));
        }
        Err(WaitExit::Session(error)) => {
            return Err(Terminal::from_error(AttemptError::Session(error)));
        }
        Err(WaitExit::Protocol) => {
            return Err(Terminal::from_error(AttemptError::ProtocolEnvelope));
        }
    };
    await_confirm
        .receive_confirmation(&confirmation, clock.auth_now())
        .map(|_| ())
        .map_err(Terminal::from_auth)
}

async fn authenticate_receiver(
    link: &mut dyn SessionLink,
    token: &DataPlaneToken,
    clock: &AttemptClock,
    timeouts: AttemptTimeouts,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
    entropy: &mut impl EntropySource,
) -> Result<(), Terminal> {
    let binding = channel_binding(link).map_err(Terminal::from_error)?;
    let deadline = auth_deadline(clock, timeouts.authentication());
    let await_start = envoix_auth::receiver_wait(binding, deadline);
    let start = match receive_interruptible(
        link,
        MAX_AUTH_PAYLOAD,
        stop,
        clock.remaining(deadline.instant().0),
    )
    .await
    {
        Ok(packet) => packet,
        Err(WaitExit::Stop(intent)) => {
            let _ = await_start.cancel();
            return Err(Terminal::retired(intent));
        }
        Err(WaitExit::Timeout) => {
            let error = expired_auth(await_start.deadline_exceeded(AuthMillis(u64::MAX)));
            return Err(Terminal::from_auth(error));
        }
        Err(WaitExit::Session(SessionError::PeerClosed)) => {
            return Err(Terminal::from_auth(await_start.peer_closed()));
        }
        Err(WaitExit::Session(error)) => {
            return Err(Terminal::from_error(AttemptError::Session(error)));
        }
        Err(WaitExit::Protocol) => {
            return Err(Terminal::from_error(AttemptError::ProtocolEnvelope));
        }
    };
    let (await_confirm, response) = await_start
        .receive_start(&start, clock.auth_now(), token, entropy)
        .map_err(Terminal::from_auth)?;
    if let Err(exit) =
        send_interruptible(link, &response, stop, clock.remaining(deadline.instant().0)).await
    {
        return Err(auth_wait_terminal(await_confirm.cancel(), exit));
    }

    let confirmation = match receive_interruptible(
        link,
        MAX_AUTH_PAYLOAD,
        stop,
        clock.remaining(deadline.instant().0),
    )
    .await
    {
        Ok(packet) => packet,
        Err(WaitExit::Stop(intent)) => {
            let _ = await_confirm.cancel();
            return Err(Terminal::retired(intent));
        }
        Err(WaitExit::Timeout) => {
            let error = expired_auth(await_confirm.deadline_exceeded(AuthMillis(u64::MAX)));
            return Err(Terminal::from_auth(error));
        }
        Err(WaitExit::Session(SessionError::PeerClosed)) => {
            return Err(Terminal::from_auth(await_confirm.peer_closed()));
        }
        Err(WaitExit::Session(error)) => {
            return Err(Terminal::from_error(AttemptError::Session(error)));
        }
        Err(WaitExit::Protocol) => {
            return Err(Terminal::from_error(AttemptError::ProtocolEnvelope));
        }
    };
    let (_, response) = await_confirm
        .receive_confirmation(&confirmation, clock.auth_now())
        .map_err(Terminal::from_auth)?;
    send_interruptible(link, &response, stop, clock.remaining(deadline.instant().0))
        .await
        .map_err(|exit| auth_wait_terminal(AuthError::Cancelled, exit))
}

#[allow(clippy::too_many_arguments)]
async fn transfer_sender(
    plan: AttemptPlan,
    spec: &AttemptTransferSpec,
    source: &mut impl SourceReader,
    link: &mut dyn SessionLink,
    clock: &AttemptClock,
    supervisor: &SharedAttemptSupervisor,
    events: &mpsc::UnboundedSender<AttemptEvent>,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
) -> Terminal {
    // No staged digest, no send. The card cannot reach an attempt without a
    // `Ready` source, which is unconstructible without one, so this is the
    // composition root failing to carry it rather than a state the authority can
    // produce — and sending anyway would be sending bytes nobody can vouch for.
    let Some(content_hash) = spec.content_hash else {
        return Terminal::from_transfer_error(TransferError::IntegrityMismatch);
    };
    let request = match SenderRequest::new(
        plan.transfer,
        spec.offered_name.clone(),
        spec.file_size,
        spec.chunk_size,
        resume_mode(plan),
        content_hash,
    ) {
        Ok(request) => request,
        Err(error) => return Terminal::from_transfer_error(error),
    };
    let ready_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
    let (await_ready, hello) = envoix_transfer::sender_start(request, ready_deadline);
    if let Err(exit) = send_frame_interruptible(
        link,
        &hello,
        stop,
        clock.remaining(ready_deadline.instant().0),
    )
    .await
    {
        return sender_wait_exit(await_ready, exit, clock);
    }
    let ready = match receive_interruptible(
        link,
        MAX_FRAME_SIZE,
        stop,
        clock.remaining(ready_deadline.instant().0),
    )
    .await
    {
        Ok(packet) => match decode_frame(&packet, await_ready.ingress_state()) {
            Ok(frame) => frame,
            Err(_) => return Terminal::protocol_violation(plan),
        },
        Err(exit) => return sender_wait_exit(await_ready, exit, clock),
    };
    let resume_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
    let (await_resume, header) =
        match await_ready.receive_ready(ready, clock.transfer_now(), resume_deadline) {
            Ok(next) => next,
            Err(failure) => return Terminal::from_machine(failure),
        };
    if let Err(exit) = send_frame_interruptible(
        link,
        &header,
        stop,
        clock.remaining(resume_deadline.instant().0),
    )
    .await
    {
        return sender_wait_exit(await_resume, exit, clock);
    }
    let resume = match receive_interruptible(
        link,
        MAX_FRAME_SIZE,
        stop,
        clock.remaining(resume_deadline.instant().0),
    )
    .await
    {
        Ok(packet) => match decode_frame(&packet, await_resume.ingress_state()) {
            Ok(frame) => frame,
            Err(_) => return Terminal::protocol_violation(plan),
        },
        Err(exit) => return sender_wait_exit(await_resume, exit, clock),
    };
    if !resume_matches_plan(&resume, plan) {
        return Terminal::protocol_violation(plan);
    }
    let ack_deadline = transfer_deadline(clock, spec.timeouts.completion_ack());
    let mut sending =
        match await_resume.receive_resume(resume, clock.transfer_now(), ack_deadline, source) {
            Ok(sending) => sending,
            Err(failure) => return Terminal::from_machine(failure),
        };

    loop {
        match sending.next_frame(source) {
            Ok(SenderStep::Chunk {
                state,
                frame,
                progress,
            }) => {
                sending = state;
                let deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
                if let Err(exit) = send_frame_interruptible(
                    link,
                    &frame,
                    stop,
                    clock.remaining(deadline.instant().0),
                )
                .await
                {
                    return sender_sending_exit(sending, exit);
                }
                emit(
                    events,
                    plan.stamp,
                    AttemptEventKind::Progress {
                        transferred: progress.bytes_sent,
                    },
                );
            }
            Ok(SenderStep::Complete { state, frame }) => {
                if let Err(exit) = send_frame_interruptible(
                    link,
                    &frame,
                    stop,
                    clock.remaining(ack_deadline.instant().0),
                )
                .await
                {
                    return sender_wait_exit(state, exit, clock);
                }
                // Complete is sent but not acknowledged: the confirm window is
                // open, so the product's confirm-timeout/mailbox fallback (P6)
                // must be reachable from here.
                emit(
                    events,
                    plan.stamp,
                    AttemptEventKind::Phase(Phase::Confirming),
                );
                let ack = match receive_interruptible(
                    link,
                    MAX_FRAME_SIZE,
                    stop,
                    clock.remaining(ack_deadline.instant().0),
                )
                .await
                {
                    Ok(packet) => match decode_frame(&packet, state.ingress_state()) {
                        Ok(frame) => frame,
                        Err(_) => return Terminal::protocol_violation(plan),
                    },
                    Err(exit) => return sender_wait_exit(state, exit, clock),
                };
                let mut state = Some(state);
                let mut ack = Some(ack);
                let decision = {
                    let mut supervisor = match supervisor.lock() {
                        Ok(supervisor) => supervisor,
                        Err(_) => {
                            return Terminal::from_error(AttemptError::SupervisorPoisoned);
                        }
                    };
                    supervisor.cross_commit_point_with(plan.stamp, || {
                        state
                            .take()
                            .expect("commit operation runs at most once")
                            .receive_ack(
                                ack.take().expect("commit operation runs at most once"),
                                clock.transfer_now(),
                            )
                    })
                };
                return match decision {
                    CommitOperationResult::Crossed(_) => Terminal::completed(CloseOrdering::Active),
                    CommitOperationResult::OperationFailed(failure) => {
                        Terminal::from_machine(failure)
                    }
                    CommitOperationResult::RetirementWon => {
                        retirement_won_wait(
                            state.expect("retirement leaves the operation untouched"),
                            stop,
                        )
                        .await
                    }
                    CommitOperationResult::AlreadyCrossed
                    | CommitOperationResult::Stale
                    | CommitOperationResult::Retired
                    | CommitOperationResult::Unknown => {
                        Terminal::from_error(AttemptError::RetirementHandshake)
                    }
                };
            }
            Err(failure) => return Terminal::from_machine(failure),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn transfer_receiver(
    plan: AttemptPlan,
    spec: &AttemptTransferSpec,
    sink: &mut impl StagingSink,
    link: &mut dyn SessionLink,
    clock: &AttemptClock,
    supervisor: &SharedAttemptSupervisor,
    events: &mpsc::UnboundedSender<AttemptEvent>,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
) -> Terminal {
    let hello_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
    let await_hello = match envoix_transfer::receiver_start(spec.chunk_size, hello_deadline) {
        Ok(state) => state,
        Err(error) => return Terminal::from_transfer_error(error),
    };
    let hello = match receive_interruptible(
        link,
        MAX_FRAME_SIZE,
        stop,
        clock.remaining(hello_deadline.instant().0),
    )
    .await
    {
        Ok(packet) => match decode_frame(&packet, await_hello.ingress_state()) {
            Ok(frame) => frame,
            Err(_) => return Terminal::protocol_violation(plan),
        },
        Err(exit) => return receiver_wait_exit(await_hello, exit, clock),
    };
    let header_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
    let (await_header, ready) =
        match await_hello.receive_hello(hello, clock.transfer_now(), header_deadline) {
            Ok(next) => next,
            Err(failure) => return Terminal::from_machine(failure),
        };
    if let Err(exit) = send_frame_interruptible(
        link,
        &ready,
        stop,
        clock.remaining(header_deadline.instant().0),
    )
    .await
    {
        return receiver_wait_exit(await_header, exit, clock);
    }
    let header = match receive_interruptible(
        link,
        MAX_FRAME_SIZE,
        stop,
        clock.remaining(header_deadline.instant().0),
    )
    .await
    {
        Ok(packet) => match decode_frame(&packet, await_header.ingress_state()) {
            Ok(frame) => frame,
            Err(_) => return Terminal::protocol_violation(plan),
        },
        Err(exit) => return receiver_wait_exit(await_header, exit, clock),
    };
    let data_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
    let (mut receiving, resume) = match await_header.receive_header(
        header,
        clock.transfer_now(),
        data_deadline,
        spec.claimed_complete,
        sink,
    ) {
        Ok(next) => next,
        Err(failure) => return Terminal::from_machine(failure),
    };
    if let Err(exit) = send_frame_interruptible(
        link,
        &resume,
        stop,
        clock.remaining(data_deadline.instant().0),
    )
    .await
    {
        return receiver_receiving_exit(receiving, exit, clock, sink);
    }

    loop {
        let packet = match receive_interruptible(
            link,
            MAX_FRAME_SIZE,
            stop,
            clock.remaining(receiving.deadline().instant().0),
        )
        .await
        {
            Ok(packet) => packet,
            Err(exit) => return receiver_receiving_exit(receiving, exit, clock, sink),
        };
        let frame = match decode_frame(&packet, receiving.ingress_state()) {
            Ok(frame) => frame,
            Err(_) => return Terminal::protocol_violation(plan),
        };
        let next_deadline = transfer_deadline(clock, spec.timeouts.transfer_idle());
        match receiving.receive(frame, clock.transfer_now(), next_deadline, sink) {
            Ok(ReceiverStep::Continue { state, progress }) => {
                receiving = state;
                emit(
                    events,
                    plan.stamp,
                    AttemptEventKind::Progress {
                        transferred: progress.bytes_staged,
                    },
                );
            }
            Ok(ReceiverStep::ReadyToCommit(ready)) => {
                let mut ready = Some(ready);
                let decision = {
                    let mut supervisor = match supervisor.lock() {
                        Ok(supervisor) => supervisor,
                        Err(_) => {
                            return Terminal::from_error(AttemptError::SupervisorPoisoned);
                        }
                    };
                    supervisor.cross_commit_point_with(plan.stamp, || {
                        ready
                            .take()
                            .expect("commit operation runs at most once")
                            .commit(sink)
                    })
                };
                let completed = match decision {
                    CommitOperationResult::Crossed(completed) => completed,
                    CommitOperationResult::OperationFailed(failure) => {
                        return Terminal::from_machine(failure);
                    }
                    CommitOperationResult::RetirementWon => {
                        return retirement_won_ready(
                            ready.expect("retirement leaves the operation untouched"),
                            stop,
                        )
                        .await;
                    }
                    CommitOperationResult::AlreadyCrossed
                    | CommitOperationResult::Stale
                    | CommitOperationResult::Retired
                    | CommitOperationResult::Unknown => {
                        return Terminal::from_error(AttemptError::RetirementHandshake);
                    }
                };
                let ack = completed.acknowledgement();
                let encoded = match encode_frame(&ack) {
                    Ok(encoded) => encoded,
                    Err(_) => return Terminal::from_error(AttemptError::ProtocolEnvelope),
                };
                // Commit has crossed. Pause/cancel now resolves as Completed and
                // must not interrupt delivery of the final acknowledgement.
                let sent =
                    timeout(spec.timeouts.completion_ack(), link.send_packet(&encoded)).await;
                return match sent {
                    Ok(Ok(())) => Terminal::completed(CloseOrdering::AwaitPeer),
                    Ok(Err(_)) | Err(_) => Terminal::completed(CloseOrdering::Active),
                };
            }
            Err(failure) => return Terminal::from_machine(failure),
        }
    }
}

#[derive(Debug)]
struct Terminal {
    outcome: OutcomeCode,
    close: CloseOrdering,
    outbound: Option<Frame>,
}

impl Terminal {
    const fn completed(close: CloseOrdering) -> Self {
        Self {
            outcome: OutcomeCode::Completed,
            close,
            outbound: None,
        }
    }

    fn from_auth(error: AuthError) -> Self {
        Self::from_error(AttemptError::Authentication(error))
    }

    const fn retired(intent: RetirementIntent) -> Self {
        Self {
            outcome: match intent {
                RetirementIntent::Pause => OutcomeCode::Paused,
                RetirementIntent::Cancel | RetirementIntent::Finalize => OutcomeCode::Cancelled,
            },
            close: CloseOrdering::Active,
            outbound: None,
        }
    }

    fn from_transfer_error(error: TransferError) -> Self {
        Self {
            outcome: error.outcome_code(),
            close: CloseOrdering::Active,
            outbound: None,
        }
    }

    fn from_machine(failure: MachineFailure) -> Self {
        Self {
            outcome: failure.error().outcome_code(),
            close: CloseOrdering::Active,
            outbound: failure.outbound(),
        }
    }

    fn from_error(error: AttemptError) -> Self {
        Self {
            outcome: error.outcome_code(),
            close: CloseOrdering::Active,
            outbound: None,
        }
    }

    fn protocol_violation(plan: AttemptPlan) -> Self {
        Self {
            outcome: OutcomeCode::Internal,
            close: CloseOrdering::Active,
            outbound: Some(Frame::Abort(Abort {
                transfer_id: Some(plan.transfer),
                reason: ProtocolReason::ProtocolViolation,
            })),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_attempt<R>(
    stamp: AttemptStamp,
    terminal: Terminal,
    mut link: Box<dyn SessionLink>,
    resource: R,
    token: DataPlaneToken,
    supervisor: SharedAttemptSupervisor,
    events: mpsc::UnboundedSender<AttemptEvent>,
    retirement_changed: Arc<Notify>,
    session_timeouts: SessionTimeouts,
    path_forwarder: Option<JoinHandle<()>>,
) -> Result<RetirementAck, AttemptError> {
    let outcome = terminal.outcome;
    if let Some(frame) = terminal.outbound
        && let Ok(encoded) = encode_frame(&frame)
    {
        let _ = timeout(session_timeouts.stream(), link.send_packet(&encoded)).await;
    }
    let _ = link.close(terminal.close, session_timeouts).await;
    drop(link);
    if let Some(path_forwarder) = path_forwarder {
        path_forwarder.abort();
        let _ = path_forwarder.await;
    }
    finish_released_attempt(
        stamp,
        outcome,
        resource,
        token,
        supervisor,
        events,
        retirement_changed,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finish_released_attempt<R>(
    stamp: AttemptStamp,
    outcome: OutcomeCode,
    resource: R,
    token: DataPlaneToken,
    supervisor: SharedAttemptSupervisor,
    events: mpsc::UnboundedSender<AttemptEvent>,
    retirement_changed: Arc<Notify>,
) -> Result<RetirementAck, AttemptError> {
    if outcome != OutcomeCode::Completed {
        let result = supervisor
            .lock()
            .map_err(|_| AttemptError::SupervisorPoisoned)?
            .resolve_terminal(stamp, outcome);
        if !matches!(
            result,
            TerminalResolutionResult::Recorded | TerminalResolutionResult::AlreadyRecorded
        ) {
            return Err(AttemptError::RetirementHandshake);
        }
    }
    emit(&events, stamp, AttemptEventKind::Terminal(outcome));
    drop(resource);
    drop(token);
    drop(events);

    loop {
        // Register with the notifier BEFORE consulting the supervisor:
        // notify_waiters() wakes only already-registered waiters, so an
        // unregistered gap between the acknowledge check and the await would
        // silently miss a retirement request and hang this loop forever.
        let mut notified = std::pin::pin!(retirement_changed.notified());
        notified.as_mut().enable();
        let result = supervisor
            .lock()
            .map_err(|_| AttemptError::SupervisorPoisoned)?
            .acknowledge_retirement(stamp);
        match result {
            RetirementAckResult::Acknowledged(ack) => return Ok(ack),
            RetirementAckResult::NotRequested | RetirementAckResult::NotReady => notified.await,
            _ => return Err(AttemptError::RetirementHandshake),
        }
    }
}

fn channel_binding(link: &dyn SessionLink) -> Result<ExportedKeyingMaterial, AttemptError> {
    let exported = link
        .export_keying_material(
            ExportedKeyingMaterial::label(),
            ExportedKeyingMaterial::context(),
        )
        .map_err(AttemptError::Session)?;
    Ok(ExportedKeyingMaterial::new(exported.into_bytes()))
}

fn emit(events: &mpsc::UnboundedSender<AttemptEvent>, stamp: AttemptStamp, kind: AttemptEventKind) {
    let _ = events.send(AttemptEvent { stamp, kind });
}

fn resume_mode(plan: AttemptPlan) -> ResumeMode {
    match plan.resume {
        envoix_attempt_api::ResumeIntent::Fresh => ResumeMode::Disabled,
        envoix_attempt_api::ResumeIntent::ResumeFrom { .. } => ResumeMode::Allowed,
    }
}

fn resume_matches_plan(frame: &Frame, plan: AttemptPlan) -> bool {
    let Frame::ResumeStatus(status) = frame else {
        return true;
    };
    match plan.resume {
        envoix_attempt_api::ResumeIntent::Fresh => status.bytes_received.get() == 0,
        envoix_attempt_api::ResumeIntent::ResumeFrom { offset } => status.bytes_received == offset,
    }
}

async fn send_frame_interruptible(
    link: &mut dyn SessionLink,
    frame: &Frame,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
    wait: Duration,
) -> Result<(), WaitExit> {
    let encoded = encode_frame(frame).map_err(|_| WaitExit::Protocol)?;
    send_interruptible(link, &encoded, stop, wait).await
}

async fn send_interruptible(
    link: &mut dyn SessionLink,
    packet: &[u8],
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
    wait: Duration,
) -> Result<(), WaitExit> {
    tokio::select! {
        biased;
        intent = stop.recv() => match intent {
            Some(intent) => Err(WaitExit::Stop(intent)),
            None => Err(WaitExit::Stop(RetirementIntent::Cancel)),
        },
        result = timeout(wait, link.send_packet(packet)) => {
            result.map_err(|_| WaitExit::Timeout)?.map_err(WaitExit::Session)
        }
    }
}

async fn receive_interruptible(
    link: &mut dyn SessionLink,
    maximum_payload: usize,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
    wait: Duration,
) -> Result<Vec<u8>, WaitExit> {
    tokio::select! {
        biased;
        intent = stop.recv() => match intent {
            Some(intent) => Err(WaitExit::Stop(intent)),
            None => Err(WaitExit::Stop(RetirementIntent::Cancel)),
        },
        result = timeout(wait, link.receive_packet(maximum_payload)) => {
            result.map_err(|_| WaitExit::Timeout)?.map_err(WaitExit::Session)
        }
    }
}

#[derive(Debug)]
enum WaitExit {
    Stop(RetirementIntent),
    Timeout,
    Session(SessionError),
    Protocol,
}

fn auth_wait_terminal(_cancelled: AuthError, exit: WaitExit) -> Terminal {
    match exit {
        WaitExit::Stop(intent) => Terminal::retired(intent),
        WaitExit::Timeout => Terminal::from_auth(AuthError::Timeout),
        WaitExit::Session(error) => Terminal::from_error(AttemptError::Session(error)),
        WaitExit::Protocol => Terminal::from_error(AttemptError::ProtocolEnvelope),
    }
}

trait SenderWaitState {
    fn deadline_exceeded(self, now: TransferMillis) -> Result<Self, MachineFailure>
    where
        Self: Sized;
    fn peer_closed(self) -> MachineFailure;
    fn cancelled(self) -> MachineFailure;
    fn paused(self) -> MachineFailure;
}

macro_rules! sender_wait_state {
    ($type:ty) => {
        impl SenderWaitState for $type {
            fn deadline_exceeded(self, now: TransferMillis) -> Result<Self, MachineFailure> {
                self.deadline_exceeded(now)
            }

            fn peer_closed(self) -> MachineFailure {
                self.peer_closed()
            }

            fn cancelled(self) -> MachineFailure {
                self.cancelled()
            }

            fn paused(self) -> MachineFailure {
                self.paused()
            }
        }
    };
}

sender_wait_state!(envoix_transfer::SenderAwaitReady);
sender_wait_state!(envoix_transfer::SenderAwaitResume);
sender_wait_state!(envoix_transfer::SenderAwaitAck);

fn sender_wait_exit(
    state: impl SenderWaitState,
    exit: WaitExit,
    _clock: &AttemptClock,
) -> Terminal {
    match exit {
        WaitExit::Stop(RetirementIntent::Pause) => Terminal::from_machine(state.paused()),
        WaitExit::Stop(_) => Terminal::from_machine(state.cancelled()),
        WaitExit::Timeout => {
            let failure = expired_transfer(state.deadline_exceeded(TransferMillis(u64::MAX)));
            Terminal::from_machine(failure)
        }
        WaitExit::Session(SessionError::PeerClosed) => Terminal::from_machine(state.peer_closed()),
        WaitExit::Session(error) => Terminal::from_error(AttemptError::Session(error)),
        WaitExit::Protocol => Terminal::from_error(AttemptError::ProtocolEnvelope),
    }
}

trait ReceiverWaitState {
    fn deadline_exceeded(self, now: TransferMillis) -> Result<Self, MachineFailure>
    where
        Self: Sized;
    fn peer_closed(self) -> MachineFailure;
    fn cancelled(self) -> MachineFailure;
    fn paused(self) -> MachineFailure;
}

macro_rules! receiver_wait_state {
    ($type:ty) => {
        impl ReceiverWaitState for $type {
            fn deadline_exceeded(self, now: TransferMillis) -> Result<Self, MachineFailure> {
                self.deadline_exceeded(now)
            }

            fn peer_closed(self) -> MachineFailure {
                self.peer_closed()
            }

            fn cancelled(self) -> MachineFailure {
                self.cancelled()
            }

            fn paused(self) -> MachineFailure {
                self.paused()
            }
        }
    };
}

receiver_wait_state!(envoix_transfer::ReceiverAwaitHello);
receiver_wait_state!(envoix_transfer::ReceiverAwaitHeader);

fn receiver_wait_exit(
    state: impl ReceiverWaitState,
    exit: WaitExit,
    _clock: &AttemptClock,
) -> Terminal {
    match exit {
        WaitExit::Stop(RetirementIntent::Pause) => Terminal::from_machine(state.paused()),
        WaitExit::Stop(_) => Terminal::from_machine(state.cancelled()),
        WaitExit::Timeout => {
            let failure = expired_transfer(state.deadline_exceeded(TransferMillis(u64::MAX)));
            Terminal::from_machine(failure)
        }
        WaitExit::Session(SessionError::PeerClosed) => Terminal::from_machine(state.peer_closed()),
        WaitExit::Session(error) => Terminal::from_error(AttemptError::Session(error)),
        WaitExit::Protocol => Terminal::from_error(AttemptError::ProtocolEnvelope),
    }
}

fn sender_sending_exit(state: envoix_transfer::SenderSending, exit: WaitExit) -> Terminal {
    match exit {
        WaitExit::Stop(RetirementIntent::Pause) => Terminal::from_machine(state.paused()),
        WaitExit::Stop(_) => Terminal::from_machine(state.cancelled()),
        WaitExit::Timeout => Terminal::from_transfer_error(TransferError::Timeout),
        WaitExit::Session(SessionError::PeerClosed) => Terminal::from_machine(state.peer_closed()),
        WaitExit::Session(error) => Terminal::from_error(AttemptError::Session(error)),
        WaitExit::Protocol => Terminal::from_error(AttemptError::ProtocolEnvelope),
    }
}

fn receiver_receiving_exit(
    state: envoix_transfer::ReceiverReceiving,
    exit: WaitExit,
    _clock: &AttemptClock,
    sink: &mut impl StagingSink,
) -> Terminal {
    match exit {
        WaitExit::Stop(RetirementIntent::Pause) => Terminal::from_machine(state.paused(sink)),
        WaitExit::Stop(_) => Terminal::from_machine(state.cancelled(sink)),
        WaitExit::Timeout => {
            let failure = expired_transfer(state.deadline_exceeded(TransferMillis(u64::MAX), sink));
            Terminal::from_machine(failure)
        }
        WaitExit::Session(SessionError::PeerClosed) => {
            Terminal::from_machine(state.peer_closed(sink))
        }
        WaitExit::Session(error) => Terminal::from_error(AttemptError::Session(error)),
        WaitExit::Protocol => Terminal::from_error(AttemptError::ProtocolEnvelope),
    }
}

async fn retirement_won_wait(
    state: envoix_transfer::SenderAwaitAck,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
) -> Terminal {
    match stop.recv().await {
        Some(RetirementIntent::Pause) => Terminal::from_machine(state.paused()),
        _ => Terminal::from_machine(state.cancelled()),
    }
}

async fn retirement_won_ready(
    state: envoix_transfer::ReceiverReadyToCommit,
    stop: &mut mpsc::UnboundedReceiver<RetirementIntent>,
) -> Terminal {
    match stop.recv().await {
        Some(RetirementIntent::Pause) => Terminal::from_machine(state.paused()),
        _ => Terminal::from_machine(state.cancelled()),
    }
}

fn auth_deadline(clock: &AttemptClock, duration: Duration) -> envoix_auth::Deadline {
    envoix_auth::Deadline::at(AuthMillis(clock.deadline_after(duration)))
}

fn expired_auth<T>(result: Result<T, AuthError>) -> AuthError {
    match result {
        Err(error) => error,
        Ok(_) => AuthError::Timeout,
    }
}

fn expired_transfer<T>(result: Result<T, MachineFailure>) -> MachineFailure {
    match result {
        Err(failure) => failure,
        Ok(_) => unreachable!("maximum monotonic time must exceed a finite deadline"),
    }
}

fn transfer_deadline(clock: &AttemptClock, duration: Duration) -> envoix_transfer::Deadline {
    envoix_transfer::Deadline::at(TransferMillis(clock.deadline_after(duration)))
}

struct AttemptClock {
    origin: Instant,
}

impl AttemptClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }

    fn deadline_after(&self, duration: Duration) -> u64 {
        self.now_ms()
            .saturating_add(duration.as_millis().min(u128::from(u64::MAX)) as u64)
    }

    fn remaining(&self, deadline_ms: u64) -> Duration {
        Duration::from_millis(deadline_ms.saturating_sub(self.now_ms()))
    }

    fn auth_now(&self) -> AuthMillis {
        AuthMillis(self.now_ms())
    }

    fn transfer_now(&self) -> TransferMillis {
        TransferMillis(self.now_ms())
    }
}
