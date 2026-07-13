//! The transfer-session driver: owns the pure [`machine`](super::machine) plus
//! the things the machine must not touch — attempts (`Client::run`), cancel
//! tokens, timers, and the mailbox schedule. Frontends talk to a
//! [`TransferSession`]: they call intents, render the snapshot stream, and act
//! as a dumb HTTP courier for the mailbox (the driver seals, verifies, and
//! decides; the app only moves bytes to/from the rdz).
//!
//! D4 (design doc): mid-run `Failed` events are session-level retry reports,
//! not attempt outcomes — the driver drops them and derives the attempt's
//! terminal from the run RESULT (`RunEnded`), which the "every failed run ends
//! its stream with a typed Failed" contract keeps equivalent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use envoix_session::{IdentityConfig, MemoryIdentity, TransferDirection};
use envoix_storage::LocalFileStorage;
use envoix_types::TransferId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::error::{Phase, TransferError, TransferFailure};
use super::event::SessionFailureCode;
use super::machine::{AttemptEvent, Effect, Input, Session};
use super::receipt;
use super::record::{RecordStore, TransferRecord, unix_now_ms};
use super::{Client, PeerSource, TransferEvent, TransferOptions, TransferRequest};

/// How long a send waits in Confirming before escalating to the mailbox.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(20);
/// The state-scoped mailbox poll schedule (design: bounded). Runs in PARALLEL
/// with the Confirming ack wait - whichever proof lands first wins - and is
/// restarted on entering Unconfirmed.
const POLL_SCHEDULE: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(40),
];
/// Minimum interval between progress-only snapshots.
const PROGRESS_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);

/// Client-side runtime policy needed to recreate a transfer after process death.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ClientContext {
    /// Human-readable chunk size as supplied by the frontend. `None` means default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<String>,
    /// CIDR allow-list for candidate addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates_allow: Vec<String>,
    /// CIDR deny-list for candidate addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates_deny: Vec<String>,
    /// Per-transfer endpoint identity used by durable mobile sessions. Keeping
    /// it stable preserves already-issued invites across retries/restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<PathBuf>,
}

impl ClientContext {
    pub fn from_config_path(path: Option<&Path>) -> Result<Self, TransferError> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let config = crate::RuntimeConfig::read(path)
            .map_err(|error| TransferError::from_core(error, Phase::Setup))?;
        let candidates = config.candidates.unwrap_or_default();
        Ok(Self {
            chunk_size: config.chunk_size,
            candidates_allow: candidates.allow,
            candidates_deny: candidates.deny,
            identity_file: None,
        })
    }

    pub fn client(&self) -> Result<Client, TransferError> {
        let mut client = Client::from_config_fields(
            self.chunk_size.as_deref(),
            &self.candidates_allow,
            &self.candidates_deny,
        )?;
        if let Some(path) = &self.identity_file {
            client.identity = IdentityConfig::Persistent(path.clone());
        }
        Ok(client)
    }
}

/// Everything needed to relaunch attempts of one transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionContext {
    /// Runtime client policy that affects transfer wire behavior and addressing.
    #[serde(default)]
    pub client: ClientContext,
    /// Direction, path, rendezvous sources, and per-transfer options.
    pub params: SessionParams,
}

impl SessionContext {
    /// Static listener coordinates appear in a QR/manual handoff and therefore
    /// must survive a resumed attempt. Dialers and Room peers should remain
    /// ephemeral so overlapping QUIC shutdown cannot collide at the relay.
    pub fn requires_stable_listener_identity(&self) -> bool {
        self.params.direction == TransferDirection::Receive
            && self.params.sources.iter().any(|source| {
                matches!(
                    source,
                    PeerSource::ShowManual { .. }
                        | PeerSource::ShowInvite { .. }
                        | PeerSource::Mdns { .. }
                )
            })
    }
}

/// Direction, paths, rendezvous sources, and attempt options for one transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionParams {
    pub direction: TransferDirection,
    /// The file to send, or the directory to receive into.
    pub path: PathBuf,
    pub sources: Vec<PeerSource>,
    /// `options.resume` applies to attempt 1 (a user-initiated NEW transfer is
    /// fresh); resumed attempts follow the machine's `StartAttempt` effect.
    pub options: TransferOptions,
    /// Receive completed into app-private staging and still needs a native
    /// Files/MediaStore publication step before user-visible completion.
    #[serde(default)]
    pub publication_required: bool,
}

/// One emission of the session's observable state.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSnapshot {
    /// Monotonic; frontends drop out-of-order snapshots.
    pub seq: u64,
    /// Instantaneous rate over the last sampling window (bytes/s).
    pub speed_bps: f64,
    /// Average rate since this attempt started moving bytes (bytes/s).
    pub avg_bps: f64,
    #[serde(flatten)]
    pub session: Session,
}

fn stabilize_listening_tokens(context: &mut SessionContext) -> Result<(), TransferError> {
    for source in &mut context.params.sources {
        let token = match source {
            PeerSource::ShowManual { token }
            | PeerSource::ShowInvite { token, .. }
            | PeerSource::Mdns { token } => token,
            _ => continue,
        };
        if token.is_none() {
            *token = Some(super::new_token()?);
        }
    }
    Ok(())
}

/// What the driver tells the frontend.
#[derive(Clone, Debug)]
pub enum SessionNotice {
    Snapshot(SessionSnapshot),
    /// Raw attempt event for diagnostics and invite/token presentation. Native
    /// clients must render lifecycle state from snapshots, never fold this.
    Event(super::StampedEvent),
    /// GET `<server>/receipts/<key>` and call
    /// [`TransferSession::receipt_response`] with the body (or None on 404).
    FetchReceipt {
        key: String,
    },
    /// POST the sealed blob to `<server>/receipts/<key>` (retry on failure).
    PostReceipt {
        key: String,
        blob: Vec<u8>,
    },
}

enum Cmd {
    Pause,
    Cancel,
    Resume,
    ReceiptResponse(Option<Vec<u8>>),
    /// Courier ack-back: the receipt POST got its 2xx.
    ReceiptPosted,
    Published(String),
    /// D2 (Remove): delete the partial, resume state, and receipt sidecars.
    Discard,
    /// Serve the peer's re-verify (courier tier: one-shot, bounded, never
    /// touches the machine, the record, or the card).
    ServeReverify,
}

/// Handle to a running transfer session (one card).
pub struct TransferSession {
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl TransferSession {
    /// Start a session: launches attempt 1 immediately. Returns the handle and
    /// the notice stream (snapshots + courier requests). With `record`, every
    /// state change is persisted for restore-across-restart.
    pub fn start(
        mut context: SessionContext,
        record: Option<(RecordStore, String)>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionNotice>), TransferError> {
        stabilize_listening_tokens(&mut context)?;
        let mut client = context.client.client()?;
        if context.requires_stable_listener_identity() && context.client.identity_file.is_none() {
            client.identity = IdentityConfig::Memory(MemoryIdentity::generate());
        }
        let direction = context.params.direction;
        let mut session = Session::new(direction);
        session.publication_required = context.params.publication_required;
        Ok(Self::spawn(
            client,
            context,
            session,
            unix_now_ms(),
            record,
            true,
        ))
    }

    /// Rehydrate a persisted session WITHOUT launching an attempt. A record
    /// that died mid-flight (process killed while active) restores as
    /// Paused(Lost) — the attempt died with the process. Standing effects are
    /// re-derived from the state: a restored Unconfirmed session resumes its
    /// mailbox poll, so receipt confirmation survives restarts.
    pub fn restore(
        record: TransferRecord,
        store: Option<(RecordStore, String)>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionNotice>), TransferError> {
        let mut context = record.context;
        stabilize_listening_tokens(&mut context)?;
        let mut client = context.client.client()?;
        if context.requires_stable_listener_identity() && context.client.identity_file.is_none() {
            client.identity = IdentityConfig::Memory(MemoryIdentity::generate());
        }
        let created_ms = record.created_ms;
        let mut session = record.session;
        session.publication_required = context.params.publication_required;
        use super::machine::{PauseOrigin, State};
        if matches!(
            session.state,
            State::Waiting
                | State::Connecting
                | State::Verifying
                | State::Transferring
                | State::Confirming
        ) {
            session.state = State::Paused(PauseOrigin::Lost);
            session.reason = Some("interrupted by an app restart".into());
        }
        Ok(Self::spawn(
            client, context, session, created_ms, store, false,
        ))
    }

    fn spawn(
        client: Client,
        context: SessionContext,
        mut session: Session,
        created_ms: u64,
        record: Option<(RecordStore, String)>,
        launch: bool,
    ) -> (Self, mpsc::UnboundedReceiver<SessionNotice>) {
        // A sender always knows its file: seed the name from the source path
        // instead of waiting for Started (pairing-stage cards showed "file").
        if session.file_name.is_none() && session.direction == TransferDirection::Send {
            session.file_name = context
                .params
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
        }
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (notice_tx, notice_rx) = mpsc::unbounded_channel();
        let actor = Actor {
            client,
            session,
            context,
            cmds: cmd_rx,
            notices: notice_tx,
            current: None,
            seq: 0,
            confirm_deadline: None,
            polls: Vec::new(),
            poll_key: None,
            rate: RateTracker::default(),
            last_progress_snapshot: None,
            created_ms,
            record,
            launch,
        };
        tokio::spawn(actor.run());
        (Self { cmds: cmd_tx }, notice_rx)
    }

    pub fn pause(&self) -> bool {
        self.cmds.send(Cmd::Pause).is_ok()
    }

    pub fn cancel(&self) -> bool {
        self.cmds.send(Cmd::Cancel).is_ok()
    }

    pub fn resume(&self) -> bool {
        self.cmds.send(Cmd::Resume).is_ok()
    }

    /// The courier's answer to a [`SessionNotice::FetchReceipt`] — the raw
    /// mailbox blob, or `None` when the slot was empty (404).
    pub fn receipt_response(&self, blob: Option<Vec<u8>>) -> bool {
        self.cmds.send(Cmd::ReceiptResponse(blob)).is_ok()
    }

    /// Courier ack-back: the receipt POST was acknowledged - the receiver's
    /// confirmation duty is discharged (drives ↻ retirement + stops re-posts).
    pub fn receipt_posted(&self) -> bool {
        self.cmds.send(Cmd::ReceiptPosted).is_ok()
    }

    /// Native publication finished; advances a staged receive to user-visible
    /// Completed and durably stores its final path/URI.
    pub fn published(&self, path: String) -> bool {
        self.cmds.send(Cmd::Published(path)).is_ok()
    }

    /// D2 (Remove, the one true abandon): delete this transfer's partial,
    /// resume state, and receipt sidecars. Call before dropping the handle.
    pub fn discard(&self) -> bool {
        self.cmds.send(Cmd::Discard).is_ok()
    }

    /// Serve the peer's re-verify from a Completed card: the mailbox-
    /// unreachable fallback. A service, not a resurrection - the card and the
    /// machine are untouched.
    pub fn serve_reverify(&self) -> bool {
        self.cmds.send(Cmd::ServeReverify).is_ok()
    }
}

/// Instantaneous/average rate accounting for snapshots.
#[derive(Default)]
struct RateTracker {
    started: Option<(Instant, u64)>, // (when bytes began moving, bytes_resumed)
    window: Option<(Instant, u64)>,  // last >=250ms sample
    speed_bps: f64,
}

impl RateTracker {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn on_progress(&mut self, bytes: u64) {
        let now = Instant::now();
        if self.started.is_none() {
            self.started = Some((now, bytes));
            self.window = Some((now, bytes));
        }
        if let Some((t0, b0)) = self.window {
            let dt = now.duration_since(t0);
            if dt >= Duration::from_millis(250) {
                self.speed_bps = (bytes.saturating_sub(b0)) as f64 / dt.as_secs_f64();
                self.window = Some((now, bytes));
            }
        }
    }

    fn avg_bps(&self, bytes: u64) -> f64 {
        match self.started {
            Some((t0, b0)) => {
                let secs = t0.elapsed().as_secs_f64();
                if secs > 0.0 {
                    (bytes.saturating_sub(b0)) as f64 / secs
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }
}

struct Actor {
    client: Client,
    session: Session,
    context: SessionContext,
    cmds: mpsc::UnboundedReceiver<Cmd>,
    notices: mpsc::UnboundedSender<SessionNotice>,
    current: Option<super::Transfer>,
    seq: u64,
    /// (attempt, deadline) of the armed confirm timer.
    confirm_deadline: Option<(u32, Instant)>,
    /// Pending mailbox poll instants (drained front to back).
    polls: Vec<Instant>,
    poll_key: Option<String>,
    rate: RateTracker,
    last_progress_snapshot: Option<Instant>,
    created_ms: u64,
    record: Option<(RecordStore, String)>,
    /// False when restoring: no attempt is launched; standing effects are
    /// re-derived from the restored state instead.
    launch: bool,
}

impl Actor {
    async fn run(mut self) {
        if self.launch {
            // Attempt 1: a user-initiated new transfer, resume per the params.
            let resume = self.context.params.options.resume;
            self.launch_attempt(resume);
        } else if self.session.state == super::machine::State::Unconfirmed {
            self.run_effect(Effect::StartMailboxPoll).await;
        } else if matches!(
            self.session.state,
            super::machine::State::Completed | super::machine::State::AwaitingPublication
        ) && self.session.direction == TransferDirection::Receive
            && !self.session.facts.proof_delivered
        {
            // Restored with the confirmation duty undischarged: re-post.
            self.run_effect(Effect::PostReceipt).await;
        }
        self.persist().await;
        self.emit_snapshot(false);

        loop {
            let confirm_at = self.confirm_deadline.map(|(_, at)| at);
            let poll_at = self.polls.first().copied();
            tokio::select! {
                cmd = self.cmds.recv() => match cmd {
                    Some(cmd) => self.on_cmd(cmd).await,
                    // All handles dropped: stop the attempt and end the actor.
                    None => {
                        if let Some(t) = self.current.take() {
                            t.cancel();
                        }
                        return;
                    }
                },
                event = next_event(&mut self.current), if self.current.is_some() => {
                    match event {
                        Some(event) => self.on_transfer_event(event).await,
                        None => self.on_run_ended().await,
                    }
                }
                _ = sleep_until(confirm_at), if confirm_at.is_some() => {
                    let attempt = self.confirm_deadline.take().map(|(a, _)| a).unwrap_or_default();
                    self.apply(Input::ConfirmTimeout { attempt }).await;
                }
                _ = sleep_until(poll_at), if poll_at.is_some() => {
                    self.polls.remove(0);
                    if let Some(key) = self.poll_key.clone() {
                        let _ = self.notices.send(SessionNotice::FetchReceipt { key });
                    }
                }
            }
        }
    }

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Pause => self.apply(Input::Pause).await,
            Cmd::Cancel => self.apply(Input::Cancel).await,
            Cmd::Resume => self.apply(Input::Resume).await,
            Cmd::ReceiptResponse(Some(blob)) => {
                let (Some(tid), Some(code)) = (self.session.transfer_id.clone(), self.code())
                else {
                    return;
                };
                // Send side: params.path IS the source file.
                match receipt::verify_receipt_against_file(
                    &tid,
                    &code,
                    &blob,
                    &self.context.params.path,
                )
                .await
                {
                    Ok(_) => self.apply(Input::ReceiptVerified).await,
                    Err(error) => {
                        tracing::warn!(%error, "mailbox receipt failed verification");
                        // An authenticated mismatch will not fix itself: stop.
                        self.polls.clear();
                    }
                }
            }
            Cmd::ReceiptPosted => self.apply(Input::ReceiptPosted).await,
            Cmd::Published(path) => self.apply(Input::Published { path }).await,
            Cmd::ReceiptResponse(None) => {} // empty slot; later polls may hit
            Cmd::ServeReverify => {
                let mut options = self.context.params.options.clone();
                options.resume = true;
                options.continuation = true;
                let request = TransferRequest {
                    direction: self.context.params.direction,
                    path: self.context.params.path.clone(),
                    sources: self.context.params.sources.clone(),
                    options,
                };
                if let Ok(transfer) = self.client.run(request) {
                    tracing::info!("serving re-verify (courier tier; card untouched)");
                    tokio::spawn(async move {
                        // Bounded: one shot; outcome only logged.
                        match tokio::time::timeout(Duration::from_secs(120), transfer.wait()).await
                        {
                            Ok(Ok(_)) => tracing::info!("re-verify served"),
                            other => tracing::info!(?other, "re-verify ended without serving"),
                        }
                    });
                }
            }
            Cmd::Discard => {
                self.discard_partial().await;
                self.discard_staged_file().await;
                if let (Some(name), Some(tid)) =
                    (&self.session.file_name, &self.session.transfer_id)
                    && let Err(error) = LocalFileStorage::delete_receipt_for_transfer(
                        &self.context.params.path,
                        name,
                        &TransferId::new(tid.clone()),
                    )
                    .await
                {
                    tracing::debug!(%error, "discard: receipt");
                }
                // Remove is the one true abandon: the record goes too.
                if let Some((store, id)) = &self.record {
                    store.delete(id).await;
                }
            }
        }
    }

    async fn on_transfer_event(&mut self, event: super::StampedEvent) {
        if let TransferEvent::Advertised { peer, .. } = &event.event
            && let Some(rebind) = self
                .context
                .params
                .options
                .listen_addrs
                .as_ref()
                .and_then(|addrs| addrs.rebind_from_advertised(&peer.direct_addrs))
        {
            self.context.params.options.listen_addrs = Some(rebind);
        }
        let _ = self.notices.send(SessionNotice::Event(event.clone()));
        let mapped = match event.event {
            TransferEvent::Advertised { .. } => Some(AttemptEvent::Advertised),
            TransferEvent::Pairing { .. } => Some(AttemptEvent::Pairing),
            TransferEvent::Connecting => Some(AttemptEvent::Connecting),
            TransferEvent::Connected { path } => Some(AttemptEvent::Connected(path)),
            TransferEvent::PathChanged { path } => Some(AttemptEvent::PathChanged(path)),
            TransferEvent::Started {
                transfer_id,
                file_name,
                total_bytes,
                bytes_resumed,
                ..
            } => {
                self.rate.reset();
                Some(AttemptEvent::Started {
                    transfer_id: transfer_id.to_string(),
                    file_name,
                    total: total_bytes,
                    bytes_resumed,
                })
            }
            TransferEvent::Progress {
                bytes_transferred, ..
            } => {
                self.rate.on_progress(bytes_transferred);
                Some(AttemptEvent::Progress {
                    bytes: bytes_transferred,
                })
            }
            TransferEvent::Verifying { .. } => Some(AttemptEvent::Verifying),
            TransferEvent::Verified { .. } => Some(AttemptEvent::Verified),
            TransferEvent::Confirming { .. } => Some(AttemptEvent::Confirming),
            TransferEvent::Completed {
                transfer_id,
                file_name,
                bytes_transferred,
            } => {
                let completed_file_path =
                    (self.context.params.direction == TransferDirection::Receive).then(|| {
                        self.context
                            .params
                            .path
                            .join(&file_name)
                            .to_string_lossy()
                            .into_owned()
                    });
                Some(AttemptEvent::Completed {
                    transfer_id: transfer_id.to_string(),
                    file_name,
                    bytes: bytes_transferred,
                    completed_file_path,
                })
            }
            // D4: mid-run Failed events are retry reports, not attempt
            // outcomes; the terminal comes from RunEnded.
            TransferEvent::Failed { .. } => None,
            _ => None,
        };
        if let Some(event) = mapped {
            let attempt = self.session.attempt;
            self.apply(Input::Event { attempt, event }).await;
        }
    }

    async fn on_run_ended(&mut self) {
        let (failure, structured_failure) = match self.current.take() {
            Some(transfer) => match transfer.wait().await {
                Ok(_) => (None, None),
                Err(error) => {
                    let structured = transfer_failure_for_session(
                        &error,
                        &self.session,
                        self.context.params.direction,
                    );
                    (
                        Some((failure_code_of(&error), error.message)),
                        Some(structured),
                    )
                }
            },
            None => (None, None),
        };
        let attempt = self.session.attempt;
        self.apply_with_failure(
            Input::Event {
                attempt,
                event: AttemptEvent::RunEnded { failure },
            },
            structured_failure,
        )
        .await;
        self.park_after_remote_stop().await;
    }

    /// Park a fresh resumable attempt after a remote pause or a mid-transfer
    /// network loss. A pause control frame can itself be lost behind queued
    /// data, so `Paused(Lost)` must rendezvous again just like `Paused(Peer)`;
    /// only `Paused(Local)` waits for an explicit user Resume.
    async fn park_after_remote_stop(&mut self) {
        if matches!(
            self.session.state,
            super::machine::State::Paused(super::machine::PauseOrigin::Peer)
                | super::machine::State::Paused(super::machine::PauseOrigin::Lost)
        ) {
            self.apply(Input::Resume).await;
        }
    }

    /// Feed one input to the machine, execute its effects, emit a snapshot.
    async fn apply(&mut self, input: Input) {
        self.apply_with_failure(input, None).await;
    }

    async fn apply_with_failure(
        &mut self,
        input: Input,
        structured_failure: Option<TransferFailure>,
    ) {
        let progress_only = matches!(
            input,
            Input::Event {
                event: AttemptEvent::Progress { .. },
                ..
            }
        );
        let before = self.session.clone();
        let effects = self.session.reduce(input);
        for effect in effects {
            self.run_effect(effect).await;
        }
        if self.session != before {
            if let Some(failure) = structured_failure {
                self.session.failure = Some(failure);
            }
            // Persist state changes (not progress ticks: on a crash the
            // receiver's on-disk resume state is the real resume anyway).
            if !progress_only {
                self.persist().await;
            }
            self.emit_snapshot(progress_only);
        }
    }

    /// Write the durable record, when recording is on.
    async fn persist(&self) {
        if let Some((store, id)) = &self.record {
            let record = TransferRecord {
                id: id.clone(),
                created_ms: self.created_ms,
                updated_ms: unix_now_ms(),
                context: self.context.clone(),
                session: self.session.clone(),
            };
            if let Err(error) = store.save(&record).await {
                tracing::warn!(%error, "persisting transfer record failed");
            }
        }
    }

    async fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::StartAttempt { resume } => self.launch_attempt(resume),
            Effect::PauseToken => {
                if let Some(t) = &self.current {
                    t.pause();
                }
            }
            Effect::CancelToken => {
                if let Some(t) = &self.current {
                    t.cancel();
                }
            }
            Effect::StartConfirmTimer => {
                self.confirm_deadline =
                    Some((self.session.attempt, Instant::now() + CONFIRM_TIMEOUT));
            }
            Effect::StopConfirmTimer => self.confirm_deadline = None,
            Effect::StartMailboxPoll => {
                if let Some(tid) = &self.session.transfer_id {
                    self.poll_key = Some(receipt::receipt_mailbox_key(tid));
                    let now = Instant::now();
                    self.polls = POLL_SCHEDULE.iter().map(|d| now + *d).collect();
                }
            }
            Effect::StopMailboxPoll => {
                self.polls.clear();
                self.poll_key = None;
            }
            Effect::PostReceipt => self.post_receipt().await,
            Effect::DiscardPartial => self.discard_partial().await,
            Effect::DiscardStagedFile => self.discard_staged_file().await,
        }
    }

    fn launch_attempt(&mut self, resume: bool) {
        let mut options = self.context.params.options.clone();
        options.resume = resume;
        options.continuation = self.session.attempt > 1;
        let request = TransferRequest {
            direction: self.context.params.direction,
            path: self.context.params.path.clone(),
            sources: self.context.params.sources.clone(),
            options,
        };
        self.rate.reset();
        match self.client.run(request) {
            Ok(transfer) => self.current = Some(transfer),
            Err(error) => {
                // Synchronous validation failure: the attempt never launched.
                let attempt = self.session.attempt;
                let structured = transfer_failure_for_session(
                    &error,
                    &self.session,
                    self.context.params.direction,
                );
                let failure = Some((failure_code_of(&error), error.message));
                // Feed inline (no await points needed for pure classify).
                let before = self.session.clone();
                let effects = self.session.reduce(Input::Event {
                    attempt,
                    event: AttemptEvent::RunEnded { failure },
                });
                debug_assert!(effects.is_empty(), "classification only");
                if self.session != before {
                    self.session.failure = Some(structured);
                }
            }
        }
    }

    /// Receive completed: seal the local receipt and hand it to the courier.
    async fn post_receipt(&mut self) {
        let (Some(name), Some(tid), Some(code)) = (
            self.session.file_name.clone(),
            self.session.transfer_id.clone(),
            self.code(),
        ) else {
            return;
        };
        let receipt_data =
            match LocalFileStorage::read_receipt(&self.context.params.path, &name).await {
                Ok(Some(r)) => r,
                _ => return,
            };
        match receipt::seal_receipt(&tid, &code, &receipt_data) {
            Ok(blob) => {
                let key = receipt::receipt_mailbox_key(&tid);
                let _ = self.notices.send(SessionNotice::PostReceipt { key, blob });
            }
            Err(error) => tracing::warn!(%error, "sealing receipt failed"),
        }
    }

    /// D1: the peer explicitly cancelled — drop the partial + resume state.
    async fn discard_partial(&self) {
        let (Some(name), Some(tid)) = (&self.session.file_name, &self.session.transfer_id) else {
            return;
        };
        let tid = TransferId::new(tid.clone());
        let dir = &self.context.params.path;
        if let Err(error) = LocalFileStorage::delete_resume_temp(dir, name, &tid).await {
            tracing::debug!(%error, "discard: temp");
        }
        if let Err(error) = LocalFileStorage::delete_resume_state(dir, name, &tid).await {
            tracing::debug!(%error, "discard: state");
        }
    }

    async fn discard_staged_file(&self) {
        if !self.context.params.publication_required
            || self.context.params.direction != TransferDirection::Receive
        {
            return;
        }
        let (Some(name), Some(tid)) = (&self.session.file_name, &self.session.transfer_id) else {
            return;
        };
        let transfer_id = TransferId::new(tid.clone());
        match LocalFileStorage::read_receipt(&self.context.params.path, name).await {
            Ok(Some(receipt)) if receipt.transfer_id == transfer_id => {}
            Ok(Some(receipt)) => {
                tracing::warn!(
                    expected_transfer_id = %transfer_id,
                    actual_transfer_id = %receipt.transfer_id,
                    file_name = %name,
                    "discard: staged file belongs to another transfer; preserving it"
                );
                return;
            }
            Ok(None) => {
                tracing::warn!(
                    transfer_id = %transfer_id,
                    file_name = %name,
                    "discard: staged file has no matching completion receipt; preserving it"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "discard: cannot validate staged receipt; preserving file");
                return;
            }
        }
        let final_path = self.context.params.path.join(name);
        if let Err(error) = tokio::fs::remove_file(&final_path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(%error, path = %final_path.display(), "discard: staged final");
        }
        if let Err(error) = LocalFileStorage::delete_receipt_for_transfer(
            &self.context.params.path,
            name,
            &transfer_id,
        )
        .await
        {
            tracing::debug!(%error, "discard: staged receipt");
        }
    }

    /// The pairing code, for sealing/verifying mailbox blobs.
    fn code(&self) -> Option<String> {
        self.context.params.sources.iter().find_map(|s| match s {
            PeerSource::Room { code, .. } => Some(code.clone()),
            PeerSource::Mdns { token: Some(t) } => Some(t.clone()),
            _ => None,
        })
    }

    fn emit_snapshot(&mut self, progress_only: bool) {
        if progress_only {
            let now = Instant::now();
            if let Some(last) = self.last_progress_snapshot
                && now.duration_since(last) < PROGRESS_SNAPSHOT_INTERVAL
            {
                return;
            }
            self.last_progress_snapshot = Some(now);
        }
        self.seq += 1;
        let snapshot = SessionSnapshot {
            seq: self.seq,
            speed_bps: self.rate.speed_bps,
            avg_bps: self.rate.avg_bps(self.session.bytes),
            session: self.session.clone(),
        };
        let _ = self.notices.send(SessionNotice::Snapshot(snapshot));
    }
}

/// The machine's failure code for an attempt result.
fn failure_code_of(error: &TransferError) -> SessionFailureCode {
    use super::ErrorKind;
    match error.kind {
        ErrorKind::Cancelled => match SessionFailureCode::classify(&error.message) {
            SessionFailureCode::PeerCancelled => SessionFailureCode::PeerCancelled,
            _ => SessionFailureCode::Cancelled,
        },
        ErrorKind::Paused => match SessionFailureCode::classify(&error.message) {
            SessionFailureCode::PeerPaused => SessionFailureCode::PeerPaused,
            _ => SessionFailureCode::Paused,
        },
        _ => SessionFailureCode::classify(&error.message),
    }
}

fn transfer_failure_for_session(
    error: &TransferError,
    session: &Session,
    direction: TransferDirection,
) -> TransferFailure {
    let mut failure = error.to_failure(Some(direction));
    failure.transfer_id = session.transfer_id.clone();
    failure.attempt_id = Some(format!("attempt-{}", session.attempt));
    failure
}

/// Await the current attempt's next event (guarded by `if current.is_some()`).
async fn next_event(current: &mut Option<super::Transfer>) -> Option<super::StampedEvent> {
    match current {
        Some(transfer) => transfer.next_event().await,
        None => std::future::pending().await,
    }
}

/// Sleep until `at` (guarded by `if at.is_some()` in the select).
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::super::ErrorKind;
    use super::super::machine::{AttemptEvent, Input, State};
    use super::*;
    use envoix_storage::{LocalFileStorage, TransferReceipt};

    #[test]
    fn listening_pairing_tokens_are_stable_within_a_session() {
        let mut context = SessionContext {
            client: ClientContext::default(),
            params: SessionParams {
                direction: TransferDirection::Receive,
                path: "/tmp/envoix".into(),
                sources: vec![
                    PeerSource::ShowManual { token: None },
                    PeerSource::ShowInvite {
                        ttl_secs: 300,
                        token: None,
                    },
                    PeerSource::Mdns { token: None },
                ],
                options: TransferOptions::default(),
                publication_required: false,
            },
        };

        stabilize_listening_tokens(&mut context).unwrap();
        let first = context.params.sources.clone();
        stabilize_listening_tokens(&mut context).unwrap();

        assert_eq!(context.params.sources, first);
        assert!(context.params.sources.iter().all(|source| match source {
            PeerSource::ShowManual { token }
            | PeerSource::ShowInvite { token, .. }
            | PeerSource::Mdns { token } => token.is_some(),
            _ => false,
        }));
    }

    #[test]
    fn only_static_receivers_need_a_stable_endpoint_identity() {
        let mut context = failing_context(TransferDirection::Receive);
        context.params.sources = vec![PeerSource::ShowInvite {
            ttl_secs: 300,
            token: None,
        }];
        assert!(context.requires_stable_listener_identity());

        context.params.direction = TransferDirection::Send;
        context.params.sources = vec![PeerSource::Invite {
            invite: "already-scanned".to_string(),
        }];
        assert!(!context.requires_stable_listener_identity());

        context.params.direction = TransferDirection::Receive;
        context.params.sources = vec![PeerSource::Room {
            code: "123456-amber-comet".to_string(),
            broker: "id@127.0.0.1:8445".to_string(),
        }];
        assert!(!context.requires_stable_listener_identity());
    }

    fn failing_params(direction: TransferDirection) -> SessionParams {
        SessionParams {
            direction,
            path: "nonexistent.bin".into(),
            sources: vec![PeerSource::Invite {
                invite: "not-an-invite".into(),
            }],
            options: TransferOptions::default(),
            publication_required: false,
        }
    }

    fn failing_context(direction: TransferDirection) -> SessionContext {
        SessionContext {
            client: ClientContext::default(),
            params: failing_params(direction),
        }
    }

    fn record(direction: TransferDirection, session: Session) -> TransferRecord {
        TransferRecord {
            id: "7".to_string(),
            created_ms: 1,
            updated_ms: 1,
            context: failing_context(direction),
            session,
        }
    }

    fn actor_for_context(
        context: SessionContext,
    ) -> (Actor, mpsc::UnboundedReceiver<SessionNotice>) {
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        drop(cmds);
        let (notice_tx, notice_rx) = mpsc::unbounded_channel();
        (
            Actor {
                client: Client::new(),
                session: Session::new(context.params.direction),
                context,
                cmds: cmd_rx,
                notices: notice_tx,
                current: None,
                seq: 0,
                confirm_deadline: None,
                polls: Vec::new(),
                poll_key: None,
                rate: RateTracker::default(),
                last_progress_snapshot: None,
                created_ms: 1,
                record: None,
                launch: false,
            },
            notice_rx,
        )
    }

    /// Drain notices until a snapshot in the wanted state arrives.
    async fn wait_for_state(
        notices: &mut mpsc::UnboundedReceiver<SessionNotice>,
        wanted: State,
    ) -> SessionSnapshot {
        let mut last_seq = 0;
        loop {
            let notice = tokio::time::timeout(Duration::from_secs(10), notices.recv())
                .await
                .expect("notice within timeout")
                .expect("stream open");
            if let SessionNotice::Snapshot(snapshot) = notice {
                assert!(snapshot.seq > last_seq, "snapshot seq must be monotonic");
                last_seq = snapshot.seq;
                if snapshot.session.state == wanted {
                    return snapshot;
                }
            }
        }
    }

    #[tokio::test]
    async fn failing_attempt_reaches_failed_via_run_ended() {
        let (_session, mut notices) =
            TransferSession::start(failing_context(TransferDirection::Send), None).unwrap();
        let snapshot = wait_for_state(&mut notices, State::Failed).await;
        assert_eq!(snapshot.session.attempt, 1);
        assert!(snapshot.session.reason.is_some());
    }

    #[tokio::test]
    async fn resume_launches_attempt_two() {
        let (session, mut notices) =
            TransferSession::start(failing_context(TransferDirection::Send), None).unwrap();
        wait_for_state(&mut notices, State::Failed).await;
        session.resume();
        let snapshot = wait_for_state(&mut notices, State::Connecting).await;
        assert_eq!(snapshot.session.attempt, 2);
        // …and the second attempt fails the same way.
        let snapshot = wait_for_state(&mut notices, State::Failed).await;
        assert_eq!(snapshot.session.attempt, 2);
    }

    #[tokio::test]
    async fn remote_loss_parks_a_resume_but_local_pause_waits_for_user() {
        let (mut actor, mut notices) =
            actor_for_context(failing_context(TransferDirection::Receive));
        actor.session.state = State::Transferring;
        actor.session.bytes = 4;
        actor.session.total = 8;
        actor
            .apply(Input::Event {
                attempt: 1,
                event: AttemptEvent::RunEnded {
                    failure: Some((SessionFailureCode::ConnectionLost, "connection lost".into())),
                },
            })
            .await;
        actor.park_after_remote_stop().await;

        let lost = wait_for_state(
            &mut notices,
            State::Paused(super::super::machine::PauseOrigin::Lost),
        )
        .await;
        assert_eq!(lost.session.attempt, 1);
        let parked = wait_for_state(&mut notices, State::Connecting).await;
        assert_eq!(parked.session.attempt, 2);

        actor.session.state = State::Paused(super::super::machine::PauseOrigin::Local);
        actor.park_after_remote_stop().await;
        assert_eq!(
            actor.session.state,
            State::Paused(super::super::machine::PauseOrigin::Local)
        );
        assert_eq!(actor.session.attempt, 2);
    }

    #[tokio::test]
    async fn restore_coerces_active_to_paused_lost() {
        let mut session = Session::new(TransferDirection::Receive);
        session.bytes = 40;
        assert_eq!(session.state, State::Connecting); // "active" when persisted
        let (_handle, mut notices) =
            TransferSession::restore(record(TransferDirection::Receive, session), None).unwrap();
        let snapshot = wait_for_state(
            &mut notices,
            State::Paused(super::super::machine::PauseOrigin::Lost),
        )
        .await;
        assert_eq!(snapshot.session.bytes, 40, "progress display survives");
        assert_eq!(snapshot.session.attempt, 1, "no attempt was launched");
    }

    #[tokio::test]
    async fn restore_unconfirmed_resumes_the_mailbox_poll() {
        let mut session = Session::new(TransferDirection::Send);
        session.state = State::Unconfirmed;
        session.transfer_id = Some("transfer-restored".into());
        let (_handle, mut notices) =
            TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
        // Receipt confirmation must survive restarts: the restored session
        // re-derives its standing effect and asks the courier to fetch.
        loop {
            let notice = tokio::time::timeout(Duration::from_secs(20), notices.recv())
                .await
                .expect("courier request within the poll schedule")
                .expect("stream open");
            if let SessionNotice::FetchReceipt { key } = notice {
                assert_eq!(key.len(), 64);
                break;
            }
        }
    }

    #[test]
    fn restore_validates_persisted_client_context() {
        let mut record = record(
            TransferDirection::Receive,
            Session::new(TransferDirection::Receive),
        );
        record.context.client.chunk_size = Some("not-a-size".into());

        assert!(TransferSession::restore(record, None).is_err());
    }

    #[tokio::test]
    async fn completed_without_started_still_posts_receipt() {
        let dir =
            std::env::temp_dir().join(format!("envoix-driver-receipt-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        LocalFileStorage::write_receipt(
            &dir,
            &TransferReceipt {
                transfer_id: TransferId::new("transfer-fast"),
                file_name: "done.bin".into(),
                file_size: 77,
                file_hash: "hash".into(),
            },
        )
        .await
        .unwrap();

        let mut context = failing_context(TransferDirection::Receive);
        context.params.path = dir.clone();
        context.params.sources = vec![PeerSource::Room {
            code: "123456-kelp-coral".into(),
            broker: "id@1.2.3.4:5".into(),
        }];
        let (mut actor, mut notices) = actor_for_context(context);

        actor
            .apply(Input::Event {
                attempt: 1,
                event: AttemptEvent::Completed {
                    transfer_id: "transfer-fast".into(),
                    file_name: "done.bin".into(),
                    bytes: 77,
                    completed_file_path: Some(dir.join("done.bin").to_string_lossy().into_owned()),
                },
            })
            .await;

        match notices.recv().await.expect("receipt posted") {
            SessionNotice::PostReceipt { key, blob } => {
                assert_eq!(key.len(), 64);
                assert!(!blob.is_empty());
            }
            notice => panic!("expected receipt post, got {notice:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn cancel_wins_over_the_attempt() {
        let (session, mut notices) =
            TransferSession::start(failing_context(TransferDirection::Receive), None).unwrap();
        session.cancel();
        let snapshot = wait_for_state(&mut notices, State::Cancelled).await;
        // Whatever the racing attempt reported, the user's cancel is final.
        assert_eq!(snapshot.session.state, State::Cancelled);
    }

    #[tokio::test]
    async fn raw_events_are_forwarded_for_diagnostics_without_driving_frontend_state() {
        let (mut actor, mut notices) =
            actor_for_context(failing_context(TransferDirection::Receive));
        let event = super::super::StampedEvent {
            ts_ms: 7,
            event: TransferEvent::Diagnostic {
                message: "endpoint ready".into(),
            },
        };

        actor.on_transfer_event(event.clone()).await;

        match notices.recv().await.expect("raw event notice") {
            SessionNotice::Event(forwarded) => assert_eq!(forwarded, event),
            notice => panic!("expected raw event, got {notice:?}"),
        }
        assert!(
            notices.try_recv().is_err(),
            "diagnostic events must not fabricate lifecycle snapshots"
        );
    }

    #[test]
    fn attempt_outcome_preserves_peer_pause_and_cancel_origin() {
        let peer_pause = TransferError {
            phase: Phase::Transfer,
            kind: ErrorKind::Paused,
            message: envoix_session::PEER_PAUSE_MESSAGE.to_string(),
        };
        let peer_cancel = TransferError {
            phase: Phase::Transfer,
            kind: ErrorKind::Cancelled,
            message: envoix_session::PEER_INTERRUPT_MESSAGE.to_string(),
        };
        let local_pause = TransferError {
            phase: Phase::Transfer,
            kind: ErrorKind::Paused,
            message: envoix_session::USER_PAUSE_MESSAGE.to_string(),
        };

        assert_eq!(failure_code_of(&peer_pause), SessionFailureCode::PeerPaused);
        assert_eq!(
            failure_code_of(&peer_cancel),
            SessionFailureCode::PeerCancelled
        );
        assert_eq!(failure_code_of(&local_pause), SessionFailureCode::Paused);
    }

    #[tokio::test]
    async fn canonical_snapshot_retains_full_structured_failure() {
        let context = failing_context(TransferDirection::Receive);
        let (mut actor, mut notices) = actor_for_context(context);
        let error = TransferError {
            phase: Phase::Transfer,
            kind: ErrorKind::Storage,
            message: "No space left on device".to_string(),
        };
        let structured =
            transfer_failure_for_session(&error, &actor.session, TransferDirection::Receive);
        actor
            .apply_with_failure(
                Input::Event {
                    attempt: 1,
                    event: AttemptEvent::RunEnded {
                        failure: Some((SessionFailureCode::Other, error.message)),
                    },
                },
                Some(structured),
            )
            .await;

        let snapshot = match notices.recv().await.expect("failed snapshot") {
            SessionNotice::Snapshot(snapshot) => snapshot,
            notice => panic!("expected snapshot, got {notice:?}"),
        };
        let failure = snapshot.session.failure.expect("structured failure");
        assert_eq!(failure.code, super::super::FailureCode::DiskFull);
        assert_eq!(failure.category, super::super::FailureCategory::Storage);
        assert_eq!(failure.phase, super::super::FailurePhase::Transferring);
        assert_eq!(failure.attempt_id.as_deref(), Some("attempt-1"));
        assert!(failure.diagnostic_message.contains("No space left"));
    }

    #[tokio::test]
    async fn peer_cancel_effect_deletes_only_the_matching_partial() {
        let dir = tempfile::tempdir().unwrap();
        let transfer_id = TransferId::new("peer-cancel-target");
        let other_id = TransferId::new("peer-cancel-other");
        let state = envoix_storage::TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "cancel.bin".to_string(),
            file_size: 8,
            chunk_size: 4,
            bytes_received: 4,
            next_chunk_index: 1,
            hash_bytes: 4,
            hash_checkpoint: None,
        };
        let other_state = envoix_storage::TransferResumeState {
            transfer_id: other_id.clone(),
            ..state.clone()
        };
        LocalFileStorage::write_resume_state(dir.path(), &state)
            .await
            .unwrap();
        LocalFileStorage::write_resume_state(dir.path(), &other_state)
            .await
            .unwrap();
        let partial =
            LocalFileStorage::resumable_temp_path(dir.path(), "cancel.bin", &transfer_id).unwrap();
        let other_partial =
            LocalFileStorage::resumable_temp_path(dir.path(), "cancel.bin", &other_id).unwrap();
        tokio::fs::write(&partial, b"abcd").await.unwrap();
        tokio::fs::write(&other_partial, b"wxyz").await.unwrap();

        let mut context = failing_context(TransferDirection::Receive);
        context.params.path = dir.path().to_path_buf();
        let (mut actor, mut notices) = actor_for_context(context);
        actor.session.state = State::Transferring;
        actor.session.transfer_id = Some(transfer_id.to_string());
        actor.session.file_name = Some("cancel.bin".to_string());
        actor.session.bytes = 4;
        actor.session.total = 8;
        actor
            .apply(Input::Event {
                attempt: 1,
                event: AttemptEvent::RunEnded {
                    failure: Some((
                        SessionFailureCode::PeerCancelled,
                        envoix_session::PEER_INTERRUPT_MESSAGE.to_string(),
                    )),
                },
            })
            .await;

        let snapshot = match notices.recv().await.expect("cancelled snapshot") {
            SessionNotice::Snapshot(snapshot) => snapshot,
            notice => panic!("expected snapshot, got {notice:?}"),
        };
        assert_eq!(snapshot.session.state, State::Cancelled);
        assert!(!partial.exists());
        assert!(
            LocalFileStorage::read_resume_state(dir.path(), "cancel.bin", &transfer_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(other_partial.exists());
        assert!(
            LocalFileStorage::read_resume_state(dir.path(), "cancel.bin", &other_id)
                .await
                .unwrap()
                .is_some()
        );
    }
}
