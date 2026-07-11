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

use std::path::PathBuf;
use std::time::Duration;

use envoix_session::TransferDirection;
use envoix_storage::LocalFileStorage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::error::TransferError;
use super::event::FailureCode;
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
    /// Receipt-mailbox endpoint (e.g. `https://rdz.example:8460`), frozen at
    /// session creation. The courier gets it from the driver's notices, so a
    /// transfer keeps confirming against the mailbox it was created with even
    /// if the (diagnostics) setting is later cleared or edited. `None` = the
    /// frontend's current setting (pre-field records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_server: Option<String>,
}

impl ClientContext {
    pub fn client(&self) -> Result<Client, TransferError> {
        Client::from_config_fields(
            self.chunk_size.as_deref(),
            &self.candidates_allow,
            &self.candidates_deny,
        )
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

/// What the driver tells the frontend.
#[derive(Clone, Debug)]
pub enum SessionNotice {
    Snapshot(SessionSnapshot),
    /// GET `<server>/receipts/<key>` and call
    /// [`TransferSession::receipt_response`] with the body (or None on 404).
    FetchReceipt {
        key: String,
        /// The durable endpoint from the session's context; `None` means the
        /// frontend falls back to its current setting (pre-field records).
        server: Option<String>,
    },
    /// POST the sealed blob to `<server>/receipts/<key>` (retry on failure).
    PostReceipt {
        key: String,
        blob: Vec<u8>,
        /// See [`Self::FetchReceipt::server`].
        server: Option<String>,
    },
}

enum Cmd {
    Pause,
    Cancel,
    Resume,
    /// The courier's answer, stamped with the mailbox key it answers so a
    /// late response from a superseded attempt can be dropped.
    ReceiptResponse {
        key: String,
        blob: Option<Vec<u8>>,
    },
    /// Courier ack-back: the receipt POST got its 2xx.
    ReceiptPosted,
    /// D2 (Remove): delete the partial, resume state, and receipt sidecars.
    Discard,
    /// Serve the peer's re-verify (courier tier: one-shot, bounded, never
    /// touches the machine, the record, or the card).
    ServeReverify,
    /// Replace the frontend-owned card context persisted with the record.
    SetExtras(serde_json::Value),
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
        context: SessionContext,
        record: Option<(RecordStore, u64)>,
        extras: Option<serde_json::Value>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionNotice>), TransferError> {
        let client = context.client.client()?;
        let direction = context.params.direction;
        Ok(Self::spawn(
            client,
            context,
            Session::new(direction),
            record,
            extras,
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
        store: Option<(RecordStore, u64)>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionNotice>), TransferError> {
        let client = record.context.client.client()?;
        let mut session = record.session;
        use super::machine::{PauseOrigin, State};
        if matches!(
            session.state,
            State::Waiting
                | State::Connecting
                | State::Verifying
                | State::Transferring
                | State::Confirming
        ) {
            // `sent_hash` is recorded exactly at Confirming: every byte and
            // the Complete frame were sent. That durable fact makes the
            // honest restored state Unconfirmed (mailbox poll resumes below),
            // not Paused(Lost) - only the ACK died with the process.
            if session.state == State::Confirming && session.sent_hash.is_some() {
                session.state = State::Unconfirmed;
                session.reason = Some("app restarted while awaiting confirmation".into());
            } else {
                session.state = State::Paused(PauseOrigin::Lost);
                session.reason = Some("interrupted by an app restart".into());
            }
        }
        Ok(Self::spawn(
            client,
            record.context,
            session,
            store,
            record.platform_extras,
            false,
        ))
    }

    fn spawn(
        client: Client,
        context: SessionContext,
        mut session: Session,
        record: Option<(RecordStore, u64)>,
        platform_extras: Option<serde_json::Value>,
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
            pending: None,
            seq: 0,
            confirm_deadline: None,
            polls: Vec::new(),
            poll_key: None,
            rate: RateTracker::default(),
            last_progress_snapshot: None,
            record,
            platform_extras,
            launch,
        };
        tokio::spawn(actor.run());
        (Self { cmds: cmd_tx }, notice_rx)
    }

    pub fn pause(&self) {
        let _ = self.cmds.send(Cmd::Pause);
    }

    pub fn cancel(&self) {
        let _ = self.cmds.send(Cmd::Cancel);
    }

    pub fn resume(&self) {
        let _ = self.cmds.send(Cmd::Resume);
    }

    /// The courier's answer to a [`SessionNotice::FetchReceipt`] — the raw
    /// mailbox blob, or `None` when the slot was empty (404). `key` echoes
    /// the fetched slot, so an answer from a superseded attempt is dropped.
    pub fn receipt_response(&self, key: String, blob: Option<Vec<u8>>) {
        let _ = self.cmds.send(Cmd::ReceiptResponse { key, blob });
    }

    /// Courier ack-back: the receipt POST was acknowledged - the receiver's
    /// confirmation duty is discharged (drives ↻ retirement + stops re-posts).
    pub fn receipt_posted(&self) {
        let _ = self.cmds.send(Cmd::ReceiptPosted);
    }

    /// D2 (Remove, the one true abandon): delete this transfer's partial,
    /// resume state, and receipt sidecars. Call before dropping the handle.
    pub fn discard(&self) {
        let _ = self.cmds.send(Cmd::Discard);
    }

    /// Serve the peer's re-verify from a Completed card: the mailbox-
    /// unreachable fallback. A service, not a resurrection - the card and the
    /// machine are untouched.
    pub fn serve_reverify(&self) {
        let _ = self.cmds.send(Cmd::ServeReverify);
    }

    /// Replace the frontend-owned card context (QR payload, saved URI, ...)
    /// persisted with the record. Opaque to the core; survives restarts.
    pub fn set_extras(&self, extras: serde_json::Value) {
        let _ = self.cmds.send(Cmd::SetExtras(extras));
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
    /// Frontend-owned card context, persisted verbatim with the record.
    platform_extras: Option<serde_json::Value>,
    /// Input produced while running effects (a sync launch failure inside
    /// [`Effect::StartAttempt`]); drained by the [`Self::apply`] loop so it
    /// takes the same reduce->effects->persist path as every other input.
    pending: Option<Input>,
    seq: u64,
    /// (attempt, deadline) of the armed confirm timer.
    confirm_deadline: Option<(u32, Instant)>,
    /// Pending mailbox poll instants (drained front to back).
    polls: Vec<Instant>,
    poll_key: Option<String>,
    rate: RateTracker,
    last_progress_snapshot: Option<Instant>,
    record: Option<(RecordStore, u64)>,
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
            if let Some(input) = self.pending.take() {
                self.apply(input).await;
            }
        } else if self.session.state == super::machine::State::Unconfirmed {
            self.run_effect(Effect::StartMailboxPoll).await;
        } else if self.session.state == super::machine::State::Completed
            && self.session.direction == TransferDirection::Receive
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
                    // All handles dropped: an infrastructure fact, not a user
                    // intent. Say nothing on the wire - the peer's connection-
                    // lost handling (partial kept, durable facts) is the
                    // designed story for silence, and a graceful teardown must
                    // be indistinguishable from a crash. A cancel here used to
                    // make the peer discard its partial (lifecycle-issue #4).
                    None => {
                        if let Some(t) = self.current.take() {
                            t.detach();
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
                        let _ = self.notices.send(SessionNotice::FetchReceipt {
                            key,
                            server: self.context.client.receipt_server.clone(),
                        });
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
            Cmd::ReceiptResponse {
                key,
                blob: Some(blob),
            } => {
                if self.poll_key.as_deref() != Some(&key) {
                    return; // a late answer for a superseded attempt's slot
                }
                let (Some(tid), Some(code)) = (self.session.transfer_id.clone(), self.code())
                else {
                    return;
                };
                let verified = match &self.session.sent_hash {
                    // The committed fact (what this attempt actually sent):
                    // never re-read the source path - it is mutable and may
                    // have changed or vanished since the send.
                    Some(sent_hash) => receipt::verify_receipt_against_fact(
                        &tid,
                        &code,
                        &blob,
                        sent_hash,
                        self.session.total,
                    ),
                    // Sessions persisted before the fact existed: fall back
                    // to hashing the source file (params.path IS the file).
                    None => {
                        receipt::verify_receipt_against_file(
                            &tid,
                            &code,
                            &blob,
                            &self.context.params.path,
                        )
                        .await
                    }
                };
                match verified {
                    Ok(_) => self.apply(Input::ReceiptVerified).await,
                    Err(error) => {
                        tracing::warn!(%error, "mailbox receipt failed verification");
                        // A machine fact, not a driver decision: recorded and
                        // persisted; polling continues (the receiver
                        // overwrites the slot if it re-completes our offer).
                        self.apply(Input::ReceiptMismatch).await;
                    }
                }
            }
            Cmd::ReceiptPosted => self.apply(Input::ReceiptPosted).await,
            Cmd::ReceiptResponse { blob: None, .. } => {} // empty slot; later polls may hit
            Cmd::ServeReverify => {
                let mut options = self.context.params.options.clone();
                options.resume = true;
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
            Cmd::SetExtras(extras) => {
                self.platform_extras = Some(extras);
                self.persist().await;
            }
            Cmd::Discard => {
                // Stop the attempt BEFORE deleting anything: the engine
                // checkpoints resume state even on its cancel path, so a live
                // attempt would resurrect the files removed below. Remove is
                // an explicit user intent - the peer hears a cancel.
                if let Some(t) = self.current.take() {
                    t.cancel_and_join().await;
                }
                super::record::discard_artifacts(
                    &self.context.params.path,
                    self.session.file_name.as_deref(),
                    self.session.transfer_id.as_deref(),
                )
                .await;
                // Remove is the one true abandon: the record goes too.
                if let Some((store, id)) = &self.record {
                    store.delete(*id).await;
                }
            }
        }
    }

    async fn on_transfer_event(&mut self, event: super::StampedEvent) {
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
            TransferEvent::Verifying {
                transfer_id,
                file_name,
                ..
            } => Some(AttemptEvent::Verifying {
                transfer_id: transfer_id.to_string(),
                file_name,
            }),
            TransferEvent::Verified { .. } => Some(AttemptEvent::Verified),
            TransferEvent::Confirming { file_hash, .. } => {
                Some(AttemptEvent::Confirming { file_hash })
            }
            TransferEvent::Completed {
                transfer_id,
                file_name,
                bytes_transferred,
            } => Some(AttemptEvent::Completed {
                transfer_id: transfer_id.to_string(),
                file_name,
                bytes: bytes_transferred,
            }),
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
        let failure = match self.current.take() {
            Some(transfer) => match transfer.wait().await {
                Ok(_) => None,
                Err(error) => Some((failure_code_of(&error), error.message)),
            },
            None => None,
        };
        let attempt = self.session.attempt;
        self.apply(Input::Event {
            attempt,
            event: AttemptEvent::RunEnded { failure },
        })
        .await;
    }

    /// Feed one input to the machine, execute its effects, emit a snapshot.
    async fn apply(&mut self, input: Input) {
        let mut progress_only = true;
        let before = self.session.clone();
        let mut next = Some(input);
        while let Some(input) = next.take() {
            progress_only &= matches!(
                input,
                Input::Event {
                    event: AttemptEvent::Progress { .. },
                    ..
                }
            );
            let effects = self.session.reduce(input);
            for effect in effects {
                self.run_effect(effect).await;
            }
            next = self.pending.take();
        }
        if self.session != before {
            // The record commits BEFORE the snapshot goes out: frontends act
            // on snapshots (publish, delete staging), and a side effect must
            // never run for a state the durable authority has not committed -
            // a crash in between would restore a world that never knew.
            // (Progress ticks skip persistence: the receiver's on-disk resume
            // state is the real resume anyway.)
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
                version: super::record::RECORD_VERSION,
                id: *id,
                updated_ms: unix_now_ms(),
                context: self.context.clone(),
                session: self.session.clone(),
                platform_extras: self.platform_extras.clone(),
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
        }
    }

    fn launch_attempt(&mut self, resume: bool) {
        let mut options = self.context.params.options.clone();
        options.resume = resume;
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
                // Queued (never fed to reduce() inline): the apply loop gives
                // it the same effects+persist+snapshot path as every input -
                // the old inline shortcut dropped effects and skipped persist,
                // so the failure vanished on restore.
                let attempt = self.session.attempt;
                let failure = Some((failure_code_of(&error), error.message));
                self.pending = Some(Input::Event {
                    attempt,
                    event: AttemptEvent::RunEnded { failure },
                });
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
                let _ = self.notices.send(SessionNotice::PostReceipt {
                    key,
                    blob,
                    server: self.context.client.receipt_server.clone(),
                });
            }
            Err(error) => tracing::warn!(%error, "sealing receipt failed"),
        }
    }

    /// D1: the peer explicitly cancelled — drop the partial + resume state.
    async fn discard_partial(&self) {
        super::record::discard_partial_files(
            &self.context.params.path,
            self.session.file_name.as_deref(),
            self.session.transfer_id.as_deref(),
        )
        .await;
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
fn failure_code_of(error: &TransferError) -> FailureCode {
    use super::ErrorKind;
    match error.kind {
        ErrorKind::Cancelled => FailureCode::Cancelled,
        ErrorKind::Paused => FailureCode::Paused,
        _ => FailureCode::classify(&error.message),
    }
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
    use super::super::machine::{AttemptEvent, Input, State};
    use super::*;
    use envoix_storage::{LocalFileStorage, TransferReceipt};

    fn failing_params(direction: TransferDirection) -> SessionParams {
        SessionParams {
            direction,
            path: "nonexistent.bin".into(),
            sources: vec![PeerSource::Invite {
                invite: "not-an-invite".into(),
            }],
            options: TransferOptions::default(),
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
            version: super::super::record::RECORD_VERSION,
            id: 7,
            updated_ms: 1,
            context: failing_context(direction),
            session,
            platform_extras: None,
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
                pending: None,
                seq: 0,
                confirm_deadline: None,
                polls: Vec::new(),
                poll_key: None,
                rate: RateTracker::default(),
                last_progress_snapshot: None,
                record: None,
                platform_extras: None,
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
            TransferSession::start(failing_context(TransferDirection::Send), None, None).unwrap();
        let snapshot = wait_for_state(&mut notices, State::Failed).await;
        assert_eq!(snapshot.session.attempt, 1);
        assert!(snapshot.session.reason.is_some());
    }

    #[tokio::test]
    async fn resume_launches_attempt_two() {
        let (session, mut notices) =
            TransferSession::start(failing_context(TransferDirection::Send), None, None).unwrap();
        wait_for_state(&mut notices, State::Failed).await;
        session.resume();
        let snapshot = wait_for_state(&mut notices, State::Connecting).await;
        assert_eq!(snapshot.session.attempt, 2);
        // …and the second attempt fails the same way.
        let snapshot = wait_for_state(&mut notices, State::Failed).await;
        assert_eq!(snapshot.session.attempt, 2);
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
    async fn restored_confirming_with_sent_hash_is_unconfirmed() {
        // sent_hash is recorded exactly at Confirming: every byte + the
        // Complete frame went out, only the ack died with the process. The
        // honest restored state is Unconfirmed (poll resumes), not Paused.
        let mut session = Session::new(TransferDirection::Send);
        session.state = State::Confirming;
        session.transfer_id = Some("transfer-confirming".into());
        session.sent_hash = Some("committed-hash".into());
        let (_handle, mut notices) =
            TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
        let snapshot = wait_for_state(&mut notices, State::Unconfirmed).await;
        assert_eq!(snapshot.session.state, State::Unconfirmed);

        // Without the committed fact (a pre-fact record), Paused(Lost) stands.
        let mut session = Session::new(TransferDirection::Send);
        session.state = State::Confirming;
        let (_handle, mut notices) =
            TransferSession::restore(record(TransferDirection::Send, session), None).unwrap();
        wait_for_state(
            &mut notices,
            State::Paused(super::super::machine::PauseOrigin::Lost),
        )
        .await;
    }

    #[tokio::test]
    async fn sync_launch_failure_is_persisted() {
        // Client::run fails synchronously on empty sources; the failure must
        // take the same apply path as any input - the old inline shortcut
        // skipped persist(), so the Failed state vanished on restore.
        let dir = std::env::temp_dir().join(format!("envoix-launchfail-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = RecordStore::new(&dir);
        let mut context = failing_context(TransferDirection::Send);
        context.params.sources = Vec::new();
        let (_handle, mut notices) =
            TransferSession::start(context, Some((store.clone(), 3)), None).unwrap();
        wait_for_state(&mut notices, State::Failed).await;

        // Persist happens-before emit: by the time any snapshot is observed,
        // the record already holds that state. No polling.
        let persisted = store.load(3).await.expect("record exists");
        assert_eq!(
            persisted.session.state,
            State::Failed,
            "the record commits before the snapshot goes out"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
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
            if let SessionNotice::FetchReceipt { key, .. } = notice {
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
                },
            })
            .await;

        match notices.recv().await.expect("receipt posted") {
            SessionNotice::PostReceipt { key, blob, .. } => {
                assert_eq!(key.len(), 64);
                assert!(!blob.is_empty());
            }
            notice => panic!("expected receipt post, got {notice:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn stale_receipt_responses_are_dropped_by_key() {
        let mut context = failing_context(TransferDirection::Send);
        context.params.sources = vec![PeerSource::Room {
            code: "123456-kelp-coral".into(),
            broker: "id@1.2.3.4:5".into(),
        }];
        let (mut actor, _notices) = actor_for_context(context);
        actor.session.state = State::Unconfirmed;
        actor.session.transfer_id = Some("transfer-current".into());
        actor.session.sent_hash = Some("sent".into());
        let current_key = receipt::receipt_mailbox_key("transfer-current");
        actor.poll_key = Some(current_key.clone());
        actor.polls = vec![Instant::now() + Duration::from_secs(60)];

        // A late answer for a superseded attempt's slot changes nothing.
        actor
            .on_cmd(Cmd::ReceiptResponse {
                key: receipt::receipt_mailbox_key("transfer-previous"),
                blob: Some(vec![1, 2, 3]),
            })
            .await;
        assert_eq!(actor.polls.len(), 1, "stale response must not clear polls");

        // The current slot with a mismatching blob records the fact and keeps
        // the bounded polls alive: the receiver overwrites the slot if it
        // re-completes our offer, so a later poll can still verify.
        actor
            .on_cmd(Cmd::ReceiptResponse {
                key: current_key,
                blob: Some(vec![1, 2, 3]),
            })
            .await;
        assert!(
            actor.session.facts.receipt_mismatch,
            "the mismatch is a recorded machine fact"
        );
        assert_eq!(actor.polls.len(), 1, "polling continues after a mismatch");
    }

    #[tokio::test]
    async fn cancel_wins_over_the_attempt() {
        let (session, mut notices) =
            TransferSession::start(failing_context(TransferDirection::Receive), None, None)
                .unwrap();
        session.cancel();
        let snapshot = wait_for_state(&mut notices, State::Cancelled).await;
        // Whatever the racing attempt reported, the user's cancel is final.
        assert_eq!(snapshot.session.state, State::Cancelled);
    }
}
