//! Durable driver for one Manifest transfer activity.
//!
//! This actor is additive to the compatible single-file driver. It reuses the
//! same pure aggregate state machine and commit-before-effects rule, while
//! persisting the accepted Manifest plan and receiver-authoritative entry
//! results as one activity.

use std::time::Duration;

use envoix_protocol::{
    ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryResultV1, ManifestEntryV1,
    ManifestHashAlgorithm, ManifestId, ManifestV1,
};
use envoix_session::{
    IdentityConfig, MemoryIdentity, SessionTransferSummary, TransferDirection, TransferSummary,
    discard_manifest_resume_state,
};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::driver::{failure_code_of, transfer_failure_for_session};
use super::error::{ErrorKind, Phase, TransferError, TransferFailure};
use super::event::SessionFailureCode;
use super::machine::{AttemptEvent, Effect, Input, PauseOrigin, State};
use super::manifest_activity::{
    ManifestActivity, ManifestOperation, ManifestRecordStore, ManifestSessionContext,
    ManifestTransferRecord, new_manifest_record,
};
use super::record::unix_now_ms;
use super::{
    Client, ManifestTransferRequest, PeerSource, StampedEvent, TransferEvent, TransferRequest,
    TransferSet,
};

const COMMIT_RETRY: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
const PROGRESS_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);

/// One observable Manifest activity state.
#[derive(Clone, Debug, Serialize)]
pub struct ManifestSessionSnapshot {
    /// Monotonic; native clients drop out-of-order snapshots.
    pub seq: u64,
    pub speed_bps: f64,
    pub avg_bps: f64,
    #[serde(flatten)]
    pub activity: ManifestActivity,
}

/// What the Manifest driver tells a native client.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ManifestSessionNotice {
    Snapshot(ManifestSessionSnapshot),
    /// Raw event for diagnostics and pairing presentation. UI lifecycle state
    /// comes only from snapshots.
    Event(StampedEvent),
}

enum Cmd {
    Pause,
    Cancel,
    Resume,
    Published(String),
    Discard,
    SetExtras(serde_json::Value),
}

/// Handle to one durable Manifest activity.
pub struct ManifestTransferSession {
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl ManifestTransferSession {
    /// Creates an activity and launches attempt 1 only after its first record
    /// has committed successfully.
    pub fn start(
        mut context: ManifestSessionContext,
        record: Option<(ManifestRecordStore, u64)>,
        extras: Option<serde_json::Value>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ManifestSessionNotice>), TransferError> {
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
        let activity = ManifestActivity::new(&context)?;
        Ok(Self::spawn(
            client,
            context,
            activity,
            unix_now_ms(),
            record,
            extras,
            true,
        ))
    }

    /// Restores a record without contacting a peer. A process-lost active
    /// attempt becomes Paused(Lost); the user explicitly resumes it.
    pub fn restore(
        record: ManifestTransferRecord,
        store: Option<(ManifestRecordStore, u64)>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ManifestSessionNotice>), TransferError> {
        record.validate()?;
        if let Some((_, id)) = &store
            && *id != record.id
        {
            return Err(TransferError::input(
                "Manifest record store id does not match the record",
            ));
        }
        let created_ms = record.created_ms;
        let extras = record.platform_extras;
        let mut context = record.context;
        let mut activity = record.activity;
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
        if state_is_active(activity.session.state) {
            activity.session.state = State::Paused(PauseOrigin::Lost);
            activity.session.reason = Some("interrupted by an app restart".into());
        }
        context.validate()?;
        Ok(Self::spawn(
            client, context, activity, created_ms, store, extras, false,
        ))
    }

    fn spawn(
        client: Client,
        context: ManifestSessionContext,
        activity: ManifestActivity,
        created_ms: u64,
        record: Option<(ManifestRecordStore, u64)>,
        platform_extras: Option<serde_json::Value>,
        launch: bool,
    ) -> (Self, mpsc::UnboundedReceiver<ManifestSessionNotice>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (notice_tx, notice_rx) = mpsc::unbounded_channel();
        tokio::spawn(
            Actor {
                client,
                context,
                activity,
                cmds: cmd_rx,
                notices: notice_tx,
                current: None,
                pending_run_end: None,
                seq: 0,
                rate: RateTracker::default(),
                last_progress_snapshot: None,
                created_ms,
                record,
                platform_extras,
                staged: Vec::new(),
                commit_failures: 0,
                commit_retry_at: None,
                launch,
            }
            .run(),
        );
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

    /// Native publication finished and supplied its user-visible path/URI.
    pub fn published(&self, path: String) -> bool {
        self.cmds.send(Cmd::Published(path)).is_ok()
    }

    /// Explicitly abandons the activity and its private resume state.
    pub fn discard(&self) -> bool {
        self.cmds.send(Cmd::Discard).is_ok()
    }

    /// Replaces frontend-owned context persisted with the card.
    pub fn set_extras(&self, extras: serde_json::Value) -> bool {
        self.cmds.send(Cmd::SetExtras(extras)).is_ok()
    }
}

fn stabilize_listening_tokens(context: &mut ManifestSessionContext) -> Result<(), TransferError> {
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

fn state_is_active(state: State) -> bool {
    matches!(
        state,
        State::Waiting
            | State::Connecting
            | State::Verifying
            | State::Transferring
            | State::Confirming
    )
}

fn is_post_commit(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::StartAttempt { .. } | Effect::DiscardPartial | Effect::DiscardStagedFile
    )
}

#[derive(Default)]
struct RateTracker {
    started: Option<(Instant, u64)>,
    window: Option<(Instant, u64)>,
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
        if let Some((at, previous)) = self.window {
            let elapsed = now.duration_since(at);
            if elapsed >= Duration::from_millis(250) {
                self.speed_bps = bytes.saturating_sub(previous) as f64 / elapsed.as_secs_f64();
                self.window = Some((now, bytes));
            }
        }
    }

    fn avg_bps(&self, bytes: u64) -> f64 {
        self.started.map_or(0.0, |(at, initial)| {
            let seconds = at.elapsed().as_secs_f64();
            if seconds > 0.0 {
                bytes.saturating_sub(initial) as f64 / seconds
            } else {
                0.0
            }
        })
    }
}

struct PendingRunEnd {
    failure: Option<(SessionFailureCode, String)>,
    structured: Option<TransferFailure>,
}

struct Actor {
    client: Client,
    context: ManifestSessionContext,
    activity: ManifestActivity,
    cmds: mpsc::UnboundedReceiver<Cmd>,
    notices: mpsc::UnboundedSender<ManifestSessionNotice>,
    current: Option<TransferSet>,
    pending_run_end: Option<PendingRunEnd>,
    seq: u64,
    rate: RateTracker,
    last_progress_snapshot: Option<Instant>,
    created_ms: u64,
    record: Option<(ManifestRecordStore, u64)>,
    platform_extras: Option<serde_json::Value>,
    staged: Vec<Effect>,
    commit_failures: u32,
    commit_retry_at: Option<Instant>,
    launch: bool,
}

impl Actor {
    async fn run(mut self) {
        if self.launch {
            self.staged.push(Effect::StartAttempt {
                resume: self.context.params.options.resume,
            });
        }
        self.try_commit().await;
        self.drain_pending_run_end().await;

        loop {
            let commit_at = self.commit_retry_at;
            tokio::select! {
                cmd = self.cmds.recv() => match cmd {
                    Some(cmd) => {
                        if !self.on_cmd(cmd).await {
                            return;
                        }
                    }
                    None => {
                        if let Some(transfer) = self.current.take() {
                            transfer.detach();
                        }
                        return;
                    }
                },
                event = next_event(&mut self.current), if self.current.is_some() => {
                    match event {
                        Some(event) => self.on_transfer_event(event).await,
                        None => self.on_run_ended().await,
                    }
                },
                _ = sleep_until(commit_at), if commit_at.is_some() => {
                    self.commit_retry_at = None;
                    self.try_commit().await;
                    self.drain_pending_run_end().await;
                }
            }
        }
    }

    async fn on_cmd(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Pause => self.apply_input(Input::Pause, None, false).await,
            Cmd::Cancel => self.apply_input(Input::Cancel, None, false).await,
            Cmd::Resume => {
                if matches!(
                    self.activity.session.state,
                    State::Paused(_) | State::Unconfirmed | State::Failed | State::Cancelled
                ) {
                    let fresh = self.activity.session.state == State::Cancelled;
                    self.activity.prepare_resume(fresh);
                }
                self.apply_input(Input::Resume, None, false).await;
            }
            Cmd::Published(path) => {
                self.apply_input(Input::Published { path }, None, false)
                    .await;
            }
            Cmd::SetExtras(extras) => {
                self.platform_extras = Some(extras);
                self.try_commit().await;
                self.drain_pending_run_end().await;
            }
            Cmd::Discard => {
                if let Some(transfer) = self.current.take() {
                    transfer.cancel_and_join().await;
                }
                if self.context.params.publication_required {
                    self.discard_staged_entries().await;
                }
                self.discard_resume_state().await;
                if let Some((store, id)) = self.record.take() {
                    store.delete(id).await;
                }
                return false;
            }
        }
        true
    }

    async fn on_transfer_event(&mut self, event: StampedEvent) {
        let _ = self
            .notices
            .send(ManifestSessionNotice::Event(event.clone()));
        if !state_is_active(self.activity.session.state) {
            return;
        }

        let mut context_changed = false;
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
            context_changed = true;
        }

        let lifecycle = match &event.event {
            TransferEvent::Advertised { .. } => Some(AttemptEvent::Advertised),
            TransferEvent::Pairing { .. } => Some(AttemptEvent::Pairing),
            TransferEvent::Connecting => Some(AttemptEvent::Connecting),
            TransferEvent::Connected { path } => Some(AttemptEvent::Connected(path.clone())),
            TransferEvent::PathChanged { path } => Some(AttemptEvent::PathChanged(path.clone())),
            _ => None,
        };
        if let Some(lifecycle) = lifecycle {
            let attempt = self.activity.session.attempt;
            self.apply_input(
                Input::Event {
                    attempt,
                    event: lifecycle,
                },
                None,
                context_changed,
            )
            .await;
            return;
        }

        let before = self.activity.clone();
        let processed = self.process_manifest_event(event.event);
        match processed {
            Ok((effects, progress_only)) => {
                self.queue_effects(effects).await;
                if self.activity == before && !context_changed {
                    return;
                }
                if progress_only {
                    if self.commit_retry_at.is_none() {
                        self.emit_snapshot(true);
                    }
                } else {
                    self.try_commit().await;
                    self.drain_pending_run_end().await;
                }
            }
            Err(error) => self.fail_protocol_event(error).await,
        }
    }

    fn process_manifest_event(
        &mut self,
        event: TransferEvent,
    ) -> Result<(Vec<Effect>, bool), TransferError> {
        match event {
            TransferEvent::ManifestPlanned {
                direction,
                manifest,
            } => {
                self.activity.accept_plan(direction, manifest)?;
                Ok((Vec::new(), false))
            }
            TransferEvent::ManifestPreparingEntry {
                manifest_id,
                entry_id,
                relative_path,
                size,
            } => {
                self.require_manifest(&manifest_id)?;
                self.activity
                    .preparing_entry(entry_id, relative_path, size)?;
                Ok((Vec::new(), false))
            }
            TransferEvent::ManifestStarted {
                manifest_id,
                direction,
                file_count,
                directory_count,
                total_bytes,
            } => {
                let manifest = self.require_manifest(&manifest_id)?;
                if direction != self.activity.session.direction
                    || file_count != manifest.file_count
                    || directory_count != manifest.directory_count
                    || total_bytes != manifest.total_bytes
                {
                    return Err(protocol_event_error(
                        "Manifest started event contradicts the accepted plan",
                    ));
                }
                self.rate.reset();
                Ok((self.activity.started()?, false))
            }
            TransferEvent::ManifestEntryStarted {
                manifest_id,
                entry_id,
                transfer_id,
                relative_path,
                total_bytes,
                bytes_resumed,
            } => {
                self.require_manifest(&manifest_id)?;
                self.activity.entry_started(
                    entry_id,
                    transfer_id.to_string(),
                    relative_path,
                    total_bytes,
                    bytes_resumed,
                )?;
                Ok((Vec::new(), false))
            }
            TransferEvent::ManifestProgress {
                manifest_id,
                entry_id,
                entry_bytes,
                entry_total_bytes,
                completed_bytes,
                total_bytes,
            } => {
                let manifest = self.require_manifest(&manifest_id)?;
                let entry = manifest
                    .entries
                    .iter()
                    .find(|entry| entry.entry_id == entry_id)
                    .ok_or_else(|| {
                        protocol_event_error("Manifest progress names an unknown entry")
                    })?;
                if entry.size != entry_total_bytes
                    || entry_bytes > entry_total_bytes
                    || total_bytes != manifest.total_bytes
                    || completed_bytes > total_bytes
                {
                    return Err(protocol_event_error(
                        "Manifest progress contradicts the accepted plan",
                    ));
                }
                self.rate.on_progress(completed_bytes);
                self.activity
                    .progress(entry_id, entry_bytes, completed_bytes);
                Ok((Vec::new(), true))
            }
            TransferEvent::ManifestEntryCompleted {
                manifest_id,
                result,
            } => {
                self.require_manifest(&manifest_id)?;
                self.activity.entry_completed(result)?;
                Ok((Vec::new(), false))
            }
            TransferEvent::ManifestCompleted {
                manifest_id,
                file_count,
                directory_count,
                total_bytes,
                entries,
            } => {
                self.require_manifest(&manifest_id)?;
                let summary = envoix_session::ManifestTransferSummary {
                    manifest_id,
                    file_count,
                    directory_count,
                    total_bytes,
                    entries,
                };
                let completed_root = self.completed_root();
                Ok((self.activity.completed(summary, completed_root)?, false))
            }
            TransferEvent::Started {
                transfer_id,
                direction,
                file_name,
                total_bytes,
                bytes_resumed,
            } if self.activity.manifest.is_none()
                && self.context.params.direction() == TransferDirection::Receive =>
            {
                if direction != TransferDirection::Receive || bytes_resumed > total_bytes {
                    return Err(protocol_event_error(
                        "compatible single-file start contradicts the receive activity",
                    ));
                }
                self.rate.reset();
                let effects = self.activity.session.reduce(Input::Event {
                    attempt: self.activity.session.attempt,
                    event: AttemptEvent::Started {
                        transfer_id: transfer_id.to_string(),
                        file_name,
                        total: total_bytes,
                        bytes_resumed,
                    },
                });
                Ok((effects, false))
            }
            TransferEvent::Progress {
                transfer_id,
                bytes_transferred,
                total_bytes,
            } if self.activity.manifest.is_none()
                && self.context.params.direction() == TransferDirection::Receive =>
            {
                if self.activity.session.transfer_id.as_deref() != Some(transfer_id.0.as_str())
                    || self.activity.session.total != total_bytes
                    || bytes_transferred > total_bytes
                {
                    return Err(protocol_event_error(
                        "compatible single-file progress contradicts its start",
                    ));
                }
                self.rate.on_progress(bytes_transferred);
                let effects = self.activity.session.reduce(Input::Event {
                    attempt: self.activity.session.attempt,
                    event: AttemptEvent::Progress {
                        bytes: bytes_transferred,
                    },
                });
                Ok((effects, true))
            }
            TransferEvent::Verifying {
                transfer_id,
                direction,
                file_name,
                ..
            } if self.activity.manifest.is_none()
                && self.context.params.direction() == TransferDirection::Receive =>
            {
                if direction != TransferDirection::Receive {
                    return Err(protocol_event_error(
                        "compatible single-file verification has the wrong direction",
                    ));
                }
                let effects = self.activity.session.reduce(Input::Event {
                    attempt: self.activity.session.attempt,
                    event: AttemptEvent::Verifying {
                        transfer_id: transfer_id.to_string(),
                        file_name,
                    },
                });
                Ok((effects, false))
            }
            TransferEvent::Verified { direction, .. }
                if self.activity.manifest.is_none()
                    && self.context.params.direction() == TransferDirection::Receive =>
            {
                if direction != TransferDirection::Receive {
                    return Err(protocol_event_error(
                        "compatible single-file verification has the wrong direction",
                    ));
                }
                let effects = self.activity.session.reduce(Input::Event {
                    attempt: self.activity.session.attempt,
                    event: AttemptEvent::Verified,
                });
                Ok((effects, false))
            }
            // The negotiated result is classified when the run ends. Sender-
            // only confirmation and the raw completion landmark do not own
            // canonical lifecycle state here.
            TransferEvent::Failed { .. }
            | TransferEvent::Binding { .. }
            | TransferEvent::Diagnostic { .. }
            | TransferEvent::Started { .. }
            | TransferEvent::Progress { .. }
            | TransferEvent::Verifying { .. }
            | TransferEvent::Verified { .. }
            | TransferEvent::Confirming { .. }
            | TransferEvent::Completed { .. }
            | TransferEvent::Advertised { .. }
            | TransferEvent::Pairing { .. }
            | TransferEvent::Connecting
            | TransferEvent::Connected { .. }
            | TransferEvent::PathChanged { .. } => Ok((Vec::new(), false)),
        }
    }

    fn require_manifest(
        &self,
        manifest_id: &ManifestId,
    ) -> Result<&envoix_protocol::ManifestV1, TransferError> {
        self.activity
            .manifest
            .as_ref()
            .filter(|manifest| &manifest.manifest_id == manifest_id)
            .ok_or_else(|| protocol_event_error("Manifest event has an unknown transfer-set id"))
    }

    async fn fail_protocol_event(&mut self, error: TransferError) {
        if let Some(transfer) = &self.current {
            transfer.cancel();
        }
        let structured = transfer_failure_for_session(
            &error,
            &self.activity.session,
            self.context.params.direction(),
        );
        self.finish_attempt(
            Some((SessionFailureCode::Other, error.message)),
            Some(structured),
        )
        .await;
    }

    async fn on_run_ended(&mut self) {
        let result = match self.current.take() {
            Some(transfer) => transfer.wait().await,
            None => return,
        };
        match result {
            Ok(SessionTransferSummary::Manifest(summary)) => {
                if state_is_active(self.activity.session.state) {
                    let before = self.activity.clone();
                    let completed_root = self.completed_root();
                    match self.activity.completed(summary, completed_root) {
                        Ok(effects) => {
                            self.queue_effects(effects).await;
                            if self.activity != before {
                                self.try_commit().await;
                            }
                        }
                        Err(error) => {
                            self.fail_protocol_event(error).await;
                            return;
                        }
                    }
                }
                self.finish_attempt(None, None).await;
            }
            Ok(SessionTransferSummary::SingleFile(summary)) => {
                match self.adopt_compatible_single_file(summary).await {
                    Ok(effects) => {
                        self.queue_effects(effects).await;
                        self.try_commit().await;
                        self.finish_attempt(None, None).await;
                    }
                    Err(error) => {
                        let structured = transfer_failure_for_session(
                            &error,
                            &self.activity.session,
                            self.context.params.direction(),
                        );
                        self.finish_attempt(
                            Some((failure_code_of(&error), error.message)),
                            Some(structured),
                        )
                        .await;
                    }
                }
            }
            Err(error) => {
                let structured = transfer_failure_for_session(
                    &error,
                    &self.activity.session,
                    self.context.params.direction(),
                );
                self.finish_attempt(
                    Some((failure_code_of(&error), error.message)),
                    Some(structured),
                )
                .await;
            }
        }
    }

    async fn adopt_compatible_single_file(
        &mut self,
        summary: TransferSummary,
    ) -> Result<Vec<Effect>, TransferError> {
        self.context.params.operation.output_dir().ok_or_else(|| {
            protocol_event_error("Manifest send unexpectedly negotiated single-file receive")
        })?;
        let manifest_id = ManifestId::new(summary.transfer_id.to_string());
        let hash = blake3::Hash::from_hex(&summary.file_hash).map_err(|_| {
            protocol_event_error("compatible single-file result has an invalid BLAKE3 hash")
        })?;
        let manifest = ManifestV1 {
            manifest_id: manifest_id.clone(),
            entries: vec![ManifestEntryV1 {
                entry_id: 0,
                relative_path: summary.file_name.clone(),
                kind: ManifestEntryKind::RegularFile,
                size: summary.bytes_transferred,
                hash: Some(*hash.as_bytes()),
                modified_at_unix_ms: None,
            }],
            file_count: 1,
            directory_count: 0,
            root_count: 1,
            total_bytes: summary.bytes_transferred,
            hash_algorithm: ManifestHashAlgorithm::Blake3_256,
        };
        manifest.validate_structure().map_err(|error| {
            protocol_event_error(format!(
                "compatible single-file result cannot form a Manifest: {error}"
            ))
        })?;

        self.activity.session.file_name = Some(manifest_id.to_string());
        self.activity
            .accept_plan(TransferDirection::Receive, manifest.clone())?;
        let result = ManifestEntryResultV1 {
            entry_id: 0,
            status: ManifestEntryResultStatus::Completed,
            offered_relative_path: summary.file_name.clone(),
            final_relative_path: Some(summary.file_name),
            failure_code: None,
        };
        self.activity.entry_completed(result.clone())?;
        let effects = self.activity.completed(
            envoix_session::ManifestTransferSummary {
                manifest_id: manifest.manifest_id,
                file_count: manifest.file_count,
                directory_count: manifest.directory_count,
                total_bytes: manifest.total_bytes,
                entries: vec![result],
            },
            self.completed_root(),
        )?;
        Ok(effects)
    }

    async fn apply_input(
        &mut self,
        input: Input,
        structured_failure: Option<TransferFailure>,
        force_commit: bool,
    ) {
        let before = self.activity.clone();
        let effects = if input == Input::Cancel {
            self.activity.cancel_unfinished()
        } else {
            self.activity.session.reduce(input)
        };
        if self.activity != before
            && let Some(failure) = structured_failure
        {
            self.activity.session.failure = Some(failure);
        }
        self.queue_effects(effects).await;
        if self.activity == before && !force_commit && self.staged.is_empty() {
            return;
        }
        self.try_commit().await;
        self.drain_pending_run_end().await;
    }

    async fn finish_attempt(
        &mut self,
        failure: Option<(SessionFailureCode, String)>,
        structured: Option<TransferFailure>,
    ) {
        let before = self.activity.clone();
        let attempt = self.activity.session.attempt;
        let failure_code = failure.as_ref().map(|(code, _)| *code);
        let effects = self.activity.session.reduce(Input::Event {
            attempt,
            event: AttemptEvent::RunEnded { failure },
        });
        if self.activity != before
            && let Some(structured) = structured
        {
            self.activity.session.failure = Some(structured);
        }
        match self.activity.session.state {
            State::Cancelled if self.activity.session.state != before.session.state => {
                self.activity.mark_unfinished_cancelled();
            }
            State::Failed if self.activity.session.state != before.session.state => {
                let code = failure_code.unwrap_or(SessionFailureCode::Other);
                if let Err(error) = self
                    .activity
                    .fail_current(format!("manifest.{}", code.as_str()))
                {
                    tracing::error!(%error, "cannot record failed Manifest entry");
                }
            }
            _ => {}
        }
        self.queue_effects(effects).await;
        if self.activity != before || !self.staged.is_empty() {
            self.try_commit().await;
        }
    }

    async fn drain_pending_run_end(&mut self) {
        if let Some(pending) = self.pending_run_end.take() {
            self.finish_attempt(pending.failure, pending.structured)
                .await;
        }
    }

    async fn queue_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if is_post_commit(&effect) {
                self.staged.push(effect);
            } else {
                self.run_effect(effect).await;
            }
        }
    }

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
                        "Manifest record commit failed; snapshot and effects withheld"
                    );
                } else {
                    tracing::error!(%error, "Manifest record store is unwritable");
                    self.storage_failed().await;
                }
            }
        }
    }

    async fn storage_failed(&mut self) {
        self.staged.clear();
        self.commit_retry_at = None;
        let effects = self.activity.session.reduce(Input::StorageFailed);
        for effect in effects {
            if !is_post_commit(&effect) {
                self.run_effect(effect).await;
            }
        }
        let _ = self.persist().await;
        self.emit_snapshot(false);
    }

    async fn persist(&self) -> Result<(), std::io::Error> {
        let Some((store, id)) = &self.record else {
            return Ok(());
        };
        let record = new_manifest_record(
            *id,
            self.context.clone(),
            self.activity.clone(),
            self.platform_extras.clone(),
        );
        let mut record = record;
        record.created_ms = self.created_ms;
        record.updated_ms = unix_now_ms();
        store.save(&record).await
    }

    async fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::StartAttempt { resume } => self.launch_attempt(resume),
            Effect::PauseToken => {
                if let Some(transfer) = &self.current {
                    transfer.pause();
                }
            }
            Effect::CancelToken => {
                if let Some(transfer) = &self.current {
                    transfer.cancel();
                }
            }
            Effect::DiscardPartial => self.discard_resume_state().await,
            Effect::DiscardStagedFile => {
                self.discard_staged_entries().await;
                self.discard_resume_state().await;
            }
            Effect::StartConfirmTimer
            | Effect::StopConfirmTimer
            | Effect::StartMailboxPoll
            | Effect::StopMailboxPoll
            | Effect::PostReceipt => {}
        }
    }

    fn launch_attempt(&mut self, resume: bool) {
        let mut options = self.context.params.options.clone();
        options.resume = resume;
        options.continuation = self.activity.session.attempt > 1;
        self.rate.reset();
        let result = match &self.context.params.operation {
            ManifestOperation::Send { request } => {
                self.client.run_manifest(ManifestTransferRequest {
                    request: request.clone(),
                    sources: self.context.params.sources.clone(),
                    options,
                })
            }
            ManifestOperation::Receive { output_dir } => {
                self.client.run_receive_transfer(TransferRequest {
                    direction: TransferDirection::Receive,
                    path: output_dir.clone(),
                    sources: self.context.params.sources.clone(),
                    options,
                })
            }
        };
        match result {
            Ok(transfer) => self.current = Some(transfer),
            Err(error) => {
                let structured = transfer_failure_for_session(
                    &error,
                    &self.activity.session,
                    self.context.params.direction(),
                );
                self.pending_run_end = Some(PendingRunEnd {
                    failure: Some((failure_code_of(&error), error.message)),
                    structured: Some(structured),
                });
            }
        }
    }

    fn completed_root(&self) -> Option<String> {
        self.context
            .params
            .operation
            .output_dir()
            .map(|path| path.to_string_lossy().into_owned())
    }

    async fn discard_resume_state(&self) {
        let (Some(output_dir), Some(manifest)) = (
            self.context.params.operation.output_dir(),
            self.activity.manifest.as_ref(),
        ) else {
            return;
        };
        if let Err(error) = discard_manifest_resume_state(output_dir, &manifest.manifest_id).await {
            tracing::warn!(%error, "cannot discard private Manifest resume state");
        }
    }

    async fn discard_staged_entries(&self) {
        if self.context.params.direction() != TransferDirection::Receive
            || !self.context.params.publication_required
        {
            return;
        }
        let (Some(output_dir), Some(manifest)) = (
            self.context.params.operation.output_dir(),
            self.activity.manifest.as_ref(),
        ) else {
            return;
        };
        for kind in [ManifestEntryKind::RegularFile, ManifestEntryKind::Directory] {
            let mut entries = manifest
                .entries
                .iter()
                .filter(|entry| entry.kind == kind)
                .filter_map(|entry| {
                    self.activity
                        .entry_results
                        .iter()
                        .find(|result| {
                            result.entry_id == entry.entry_id
                                && matches!(
                                    result.status,
                                    ManifestEntryResultStatus::Completed
                                        | ManifestEntryResultStatus::Renamed
                                )
                        })
                        .and_then(|result| result.final_relative_path.as_deref())
                })
                .collect::<Vec<_>>();
            entries.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
            for relative in entries {
                let path = output_dir.join(relative);
                let result = if kind == ManifestEntryKind::RegularFile {
                    tokio::fs::remove_file(&path).await
                } else {
                    tokio::fs::remove_dir(&path).await
                };
                if let Err(error) = result
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(%error, path = %path.display(), "cannot discard staged Manifest entry");
                }
            }
        }
    }

    fn emit_snapshot(&mut self, progress_only: bool) {
        if progress_only {
            let now = Instant::now();
            if self
                .last_progress_snapshot
                .is_some_and(|last| now.duration_since(last) < PROGRESS_SNAPSHOT_INTERVAL)
            {
                return;
            }
            self.last_progress_snapshot = Some(now);
        }
        self.seq += 1;
        let snapshot = ManifestSessionSnapshot {
            seq: self.seq,
            speed_bps: self.rate.speed_bps,
            avg_bps: self.rate.avg_bps(self.activity.session.bytes),
            activity: self.activity.clone(),
        };
        let _ = self.notices.send(ManifestSessionNotice::Snapshot(snapshot));
    }
}

fn protocol_event_error(message: impl Into<String>) -> TransferError {
    TransferError {
        phase: Phase::Transfer,
        kind: ErrorKind::Protocol,
        message: message.into(),
        failure_code: None,
    }
}

async fn next_event(current: &mut Option<TransferSet>) -> Option<StampedEvent> {
    match current {
        Some(transfer) => transfer.next_event().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "manifest_driver_tests.rs"]
mod tests;
