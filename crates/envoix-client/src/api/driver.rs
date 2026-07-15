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
use super::event::SessionFailureCode as FailureCode;
use super::machine::{AttemptEvent, Effect, Input, Session};
use super::receipt;
use super::record::{RecordStore, TransferRecord, unix_now_ms};
use super::{Client, PeerSource, TransferEvent, TransferOptions, TransferRequest};

/// How long a send waits in Confirming before escalating to the mailbox.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(20);
/// Commit-barrier retry backoff; after the last entry the session escalates
/// to [`Input::StorageFailed`] - a visible failure, never a silent stall.
const COMMIT_RETRY: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// World-facing effects wait for the commit barrier; in-memory bookkeeping
/// (timers, polls) and token signals do not - stopping an attempt is
/// idempotent and must never wait on a disk.
fn is_post_commit(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::StartAttempt { .. }
            | Effect::PostReceipt
            | Effect::DiscardPartial
            | Effect::DiscardStagedFile
    )
}
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
    /// Per-transfer endpoint identity used by durable listener sessions.
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
            receipt_server: None,
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
    /// Static listener coordinates are handed to the peer and must survive a
    /// durable resume. Dialers and room peers stay ephemeral.
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
    /// The receive lands in app-private staging and needs a native publication
    /// step before it is user-visible.
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
// Boxing `Snapshot` would break the established public match surface merely to
// optimize this infrequent UI notification enum.
#[allow(clippy::large_enum_variant)]
pub enum SessionNotice {
    Snapshot(SessionSnapshot),
    /// Raw event for diagnostics and invite/token presentation. Lifecycle UI
    /// must render snapshots rather than fold this stream independently.
    Event(super::StampedEvent),
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
    Published(String),
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
        mut context: SessionContext,
        record: Option<(RecordStore, u64)>,
        extras: Option<serde_json::Value>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionNotice>), TransferError> {
        stabilize_listening_tokens(&mut context)?;
        if context.requires_stable_listener_identity()
            && context.client.identity_file.is_none()
            && let Some((store, id)) = &record
        {
            context.client.identity_file = Some(store.identity_path(*id));
        }
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
        let mut context = record.context;
        stabilize_listening_tokens(&mut context)?;
        if context.requires_stable_listener_identity()
            && context.client.identity_file.is_none()
            && let Some((store, id)) = &store
        {
            context.client.identity_file = Some(store.identity_path(*id));
        }
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
            context,
            session,
            created_ms,
            store,
            record.platform_extras,
            false,
        ))
    }

    fn spawn(
        client: Client,
        context: SessionContext,
        mut session: Session,
        created_ms: u64,
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
            pending_failure: None,
            seq: 0,
            confirm_deadline: None,
            polls: Vec::new(),
            poll_key: None,
            rate: RateTracker::default(),
            last_progress_snapshot: None,
            created_ms,
            record,
            platform_extras,
            staged: Vec::new(),
            commit_failures: 0,
            commit_retry_at: None,
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
    /// mailbox blob, or `None` when the slot was empty (404). `key` echoes
    /// the fetched slot, so an answer from a superseded attempt is dropped.
    pub fn receipt_response(&self, key: String, blob: Option<Vec<u8>>) -> bool {
        self.cmds.send(Cmd::ReceiptResponse { key, blob }).is_ok()
    }

    /// Courier ack-back: the receipt POST was acknowledged - the receiver's
    /// confirmation duty is discharged (drives ↻ retirement + stops re-posts).
    pub fn receipt_posted(&self) -> bool {
        self.cmds.send(Cmd::ReceiptPosted).is_ok()
    }

    /// Native publication finished; advances a staged receive to user-visible
    /// Completed and durably stores its final path or URI.
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

    /// Replace the frontend-owned card context (QR payload, saved URI, ...)
    /// persisted with the record. Opaque to the core; survives restarts.
    pub fn set_extras(&self, extras: serde_json::Value) -> bool {
        self.cmds.send(Cmd::SetExtras(extras)).is_ok()
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
    /// World-facing effects staged behind the commit barrier: they run only
    /// after the record write succeeds. In-memory bookkeeping and token
    /// signals never wait here.
    staged: Vec<Effect>,
    /// Consecutive failed commits (drives the retry backoff + escalation).
    commit_failures: u32,
    /// When to retry a failed commit.
    commit_retry_at: Option<Instant>,
    /// Input produced while running effects (a sync launch failure inside
    /// [`Effect::StartAttempt`]); drained by the [`Self::apply`] loop so it
    /// takes the same reduce->effects->persist path as every other input.
    pending: Option<Input>,
    /// Structured detail paired with `pending` when an attempt fails before it
    /// can produce an event stream.
    pending_failure: Option<TransferFailure>,
    seq: u64,
    /// (attempt, deadline) of the armed confirm timer.
    confirm_deadline: Option<(u32, Instant)>,
    /// Pending mailbox poll instants (drained front to back).
    polls: Vec<Instant>,
    poll_key: Option<String>,
    rate: RateTracker,
    last_progress_snapshot: Option<Instant>,
    created_ms: u64,
    record: Option<(RecordStore, u64)>,
    /// False when restoring: no attempt is launched; standing effects are
    /// re-derived from the restored state instead.
    launch: bool,
}

impl Actor {
    async fn run(mut self) {
        if self.launch {
            // Attempt 1 waits behind the commit barrier like every other
            // world-facing effect: nothing contacts a peer for a session the
            // record has not committed.
            let resume = self.context.params.options.resume;
            self.staged.push(Effect::StartAttempt { resume });
        } else if self.session.state == super::machine::State::Unconfirmed {
            self.run_effect(Effect::StartMailboxPoll).await;
        } else if matches!(
            self.session.state,
            super::machine::State::Completed | super::machine::State::AwaitingPublication
        ) && self.session.direction == TransferDirection::Receive
            && !self.session.facts.proof_delivered
        {
            // Restored with the confirmation duty undischarged: re-post.
            self.staged.push(Effect::PostReceipt);
        }
        self.try_commit().await;
        if let Some(input) = self.pending.take() {
            let failure = self.pending_failure.take();
            self.apply_with_failure(input, failure).await;
        }

        loop {
            let confirm_at = self.confirm_deadline.map(|(_, at)| at);
            let poll_at = self.polls.first().copied();
            let commit_at = self.commit_retry_at;
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
                _ = sleep_until(commit_at), if commit_at.is_some() => {
                    self.commit_retry_at = None;
                    self.try_commit().await;
                    if let Some(input) = self.pending.take() {
                        let failure = self.pending_failure.take();
                        self.apply_with_failure(input, failure).await;
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
            Cmd::Published(path) => self.apply(Input::Published { path }).await,
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
                self.try_commit().await;
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

    /// A remote pause/loss must park a fresh listening or dialing attempt so
    /// the side that later resumes can reconnect without requiring a second
    /// user action on the peer device. Local pause still waits for Resume.
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
        let before = self.session.clone();
        let mut progress_only = true;
        let mut next = Some((input, structured_failure));
        loop {
            while let Some((input, structured_failure)) = next.take() {
                progress_only &= matches!(
                    input,
                    Input::Event {
                        event: AttemptEvent::Progress { .. },
                        ..
                    }
                );
                let before_input = self.session.clone();
                for effect in self.session.reduce(input) {
                    if is_post_commit(&effect) {
                        self.staged.push(effect);
                    } else {
                        self.run_effect(effect).await;
                    }
                }
                if self.session != before_input
                    && let Some(failure) = structured_failure
                {
                    self.session.failure = Some(failure);
                }
                next = self
                    .pending
                    .take()
                    .map(|input| (input, self.pending_failure.take()));
            }
            if self.session == before && self.staged.is_empty() {
                return; // no legal edge, nothing to commit
            }
            if progress_only {
                // Progress is UI-only and never persisted - but while a
                // commit is pending, the full-session snapshot would leak
                // uncommitted state, so it is withheld with the rest.
                if self.commit_retry_at.is_none() {
                    self.emit_snapshot(true);
                }
                return;
            }
            // The commit barrier: the record commits BEFORE the snapshot and
            // BEFORE any world-facing effect - a crash in between would
            // restore a world that never knew. Ordering alone is not enough:
            // the write's SUCCESS gates them (a swallowed failure makes the
            // barrier fake - PR #48 review, P1).
            self.try_commit().await;
            match self.pending.take() {
                // A drained StartAttempt failed synchronously: reduce it
                // through the same loop.
                Some(input) => {
                    next = Some((input, self.pending_failure.take()));
                }
                None => return,
            }
        }
    }

    /// Run the commit barrier: persist, then release the staged world-facing
    /// effects and the snapshot. On failure, withhold both and retry on a
    /// bounded backoff; when the store stays unwritable, escalate to a
    /// VISIBLE failure - never a silent stall.
    async fn try_commit(&mut self) {
        match self.persist().await {
            Ok(()) => {
                self.commit_failures = 0;
                self.commit_retry_at = None;
                for effect in std::mem::take(&mut self.staged) {
                    self.run_effect(effect).await;
                }
                self.emit_snapshot(false);
            }
            Err(error) => {
                let failures = self.commit_failures as usize;
                if failures < COMMIT_RETRY.len() {
                    self.commit_failures += 1;
                    self.commit_retry_at = Some(Instant::now() + COMMIT_RETRY[failures]);
                    tracing::warn!(
                        %error,
                        attempt = self.commit_failures,
                        "record commit failed; snapshot and effects withheld"
                    );
                } else {
                    tracing::error!(%error, "record store unwritable; failing the session");
                    self.storage_failed().await;
                }
            }
        }
    }

    /// Terminal escalation: the machine records the storage failure and the
    /// snapshot goes out even though the record could not - the store is
    /// gone, and a truthful UI is what remains. Staged effects for the
    /// never-committed states are dropped: they were never world-visible,
    /// and conservative loses nothing (a kept partial, an unposted receipt).
    async fn storage_failed(&mut self) {
        self.staged.clear();
        self.commit_retry_at = None;
        for effect in self.session.reduce(Input::StorageFailed) {
            if !is_post_commit(&effect) {
                self.run_effect(effect).await;
            }
        }
        let _ = self.persist().await; // best-effort last attempt
        self.emit_snapshot(false);
    }

    /// Write the durable record, when recording is on. `Ok` when no store is
    /// attached: durability is not required, so the barrier is vacuous.
    async fn persist(&self) -> Result<(), std::io::Error> {
        let Some((store, id)) = &self.record else {
            return Ok(());
        };
        let record = TransferRecord {
            version: super::record::RECORD_VERSION,
            id: *id,
            created_ms: self.created_ms,
            updated_ms: unix_now_ms(),
            context: self.context.clone(),
            session: self.session.clone(),
            platform_extras: self.platform_extras.clone(),
        };
        store.save(&record).await
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
                // Queued (never fed to reduce() inline): the apply loop gives
                // it the same effects+persist+snapshot path as every input -
                // the old inline shortcut dropped effects and skipped persist,
                // so the failure vanished on restore.
                let attempt = self.session.attempt;
                self.pending_failure = Some(transfer_failure_for_session(
                    &error,
                    &self.session,
                    self.context.params.direction,
                ));
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

    /// Delete only the final staging artifact that belongs to this transfer.
    /// The matching receipt is the ownership proof; otherwise preserve it.
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
                    "discard: staged file has no matching receipt; preserving it"
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
pub(super) fn failure_code_of(error: &TransferError) -> FailureCode {
    use super::ErrorKind;
    match error.kind {
        ErrorKind::Cancelled => match FailureCode::classify(&error.message) {
            FailureCode::PeerCancelled => FailureCode::PeerCancelled,
            _ => FailureCode::Cancelled,
        },
        ErrorKind::Paused => match FailureCode::classify(&error.message) {
            FailureCode::PeerPaused => FailureCode::PeerPaused,
            _ => FailureCode::Paused,
        },
        _ => FailureCode::classify(&error.message),
    }
}

pub(super) fn transfer_failure_for_session(
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
            version: super::super::record::RECORD_VERSION,
            id: 7,
            created_ms: 1,
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
                pending_failure: None,
                seq: 0,
                confirm_deadline: None,
                polls: Vec::new(),
                poll_key: None,
                rate: RateTracker::default(),
                last_progress_snapshot: None,
                created_ms: 1,
                record: None,
                platform_extras: None,
                staged: Vec::new(),
                commit_failures: 0,
                commit_retry_at: None,
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

    /// The commit barrier under a dead store: no snapshot may leak an
    /// uncommitted state, the attempt never launches, and after the bounded
    /// retries the session fails VISIBLY (never a silent stall).
    #[tokio::test(start_paused = true)]
    async fn unwritable_store_escalates_to_a_visible_failure() {
        let root = std::env::temp_dir().join(format!("envoix-deadstore-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&root).await;
        tokio::fs::create_dir_all(&root).await.unwrap();
        // A FILE where the store wants a directory: every save fails.
        tokio::fs::write(root.join("blocker"), b"x").await.unwrap();
        let store = RecordStore::new(root.join("blocker").join("records"));

        let (_handle, mut notices) = TransferSession::start(
            failing_context(TransferDirection::Send),
            Some((store, 5)),
            None,
        )
        .unwrap();

        // The paused clock drives the retry backoff instantly; the FIRST
        // snapshot ever observed must already be the terminal escalation -
        // anything earlier would be a view of uncommitted state.
        let snapshot = loop {
            match notices.recv().await.expect("stream open") {
                SessionNotice::Snapshot(s) => break s,
                _ => continue,
            }
        };
        assert_eq!(snapshot.session.state, State::Failed);
        assert!(
            snapshot
                .session
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("record store"),
            "the failure names the store, not the transfer"
        );
        let _ = tokio::fs::remove_dir_all(&root).await;
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
                transfer_id: envoix_types::TransferId::new("transfer-fast"),
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
