use super::*;

impl FfiTransferRequest {
    pub(crate) fn send(file_path: String, mode: FfiTransferMode) -> Self {
        Self {
            activity_id: next_activity_id(),
            direction: FfiTransferDirection::Send,
            mode,
            file_path,
            output_dir: String::new(),
            peer_descriptor: String::new(),
            invite: String::new(),
            code: String::new(),
            token: String::new(),
            broker: String::new(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            resume: true,
            publication_required: false,
            limits: FfiTransferLimits::default(),
            rendezvous: FfiRendezvousPlan::for_mode(mode),
        }
    }

    pub(crate) fn receive(output_dir: String, mode: FfiTransferMode) -> Self {
        Self {
            activity_id: next_activity_id(),
            direction: FfiTransferDirection::Receive,
            mode,
            file_path: String::new(),
            output_dir,
            peer_descriptor: String::new(),
            invite: String::new(),
            code: String::new(),
            token: String::new(),
            broker: String::new(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            resume: true,
            publication_required: false,
            limits: FfiTransferLimits::default(),
            rendezvous: FfiRendezvousPlan::for_mode(mode),
        }
    }
}

impl FfiPairingInvite {
    pub(crate) fn from_invite(invite: &Invite) -> Self {
        Self {
            code: invite.code().to_string(),
            payload: invite.payload(),
            broker: invite.broker().unwrap_or_default().to_string(),
            relay: invite.relay().unwrap_or_default().to_string(),
            role: ffi_invite_role(invite.role()),
        }
    }
}

impl FfiTransferActivityRecord {
    pub(crate) fn from_request(request: &FfiTransferRequest, now_ms: u64) -> Self {
        Self {
            activity_id: request.activity_id.clone(),
            sequence: 0,
            attempt_id: String::new(),
            state: FfiTransferActivityState::Queued,
            direction: request.direction,
            mode: request.mode,
            transfer_id: String::new(),
            file_name: request_file_name(request),
            total_bytes: 0,
            bytes_transferred: 0,
            bytes_resumed: 0,
            speed_bps: 0,
            average_speed_bps: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: 0,
            completed_at_ms: 0,
            completed_file_path: String::new(),
            data_path_kind: FfiDataPathKind::None,
            data_path_detail: String::new(),
            invite: request.invite.clone(),
            token: request.token.clone(),
            peer_descriptor: request.peer_descriptor.clone(),
            diagnostic_message: String::new(),
            failure_code: FfiFailureCode::Unknown,
            failure_category: FfiFailureCategory::Unknown,
            failure_phase: FfiFailurePhase::Setup,
            failure_origin: FfiFailureOrigin::Unknown,
            user_message_key: String::new(),
            retryable: false,
            recovery_action: FfiRecoveryAction::None,
            limits: request.limits.clone(),
        }
    }

    pub(crate) fn apply_event(&mut self, event: &FfiTransferEvent) {
        self.apply_observation(event);

        self.state = match event.kind {
            FfiTransferEventKind::Binding => FfiTransferActivityState::Binding,
            FfiTransferEventKind::Advertised => FfiTransferActivityState::WaitingForPeer,
            FfiTransferEventKind::Pairing => FfiTransferActivityState::Pairing,
            FfiTransferEventKind::Connecting | FfiTransferEventKind::Connected => {
                if self.started_at_ms == 0 {
                    FfiTransferActivityState::Connecting
                } else {
                    self.state
                }
            }
            FfiTransferEventKind::PathChanged => self.state,
            FfiTransferEventKind::Started | FfiTransferEventKind::Progress => {
                if self.started_at_ms == 0 {
                    self.started_at_ms = event.ts_ms;
                }
                FfiTransferActivityState::Transferring
            }
            FfiTransferEventKind::Verifying | FfiTransferEventKind::Verified => {
                FfiTransferActivityState::Verifying
            }
            // The protocol-level event is emitted before `report_terminal` attaches the
            // receiver's committed file path. Keep the activity non-terminal until then so
            // native clients cannot publish or announce a file that is not addressable yet.
            FfiTransferEventKind::Completed => FfiTransferActivityState::Verifying,
            FfiTransferEventKind::Failed => {
                self.completed_at_ms = event.ts_ms;
                FfiTransferActivityState::Failed
            }
            FfiTransferEventKind::Unknown => self.state,
        };
    }

    /// Copy diagnostic and presentation fields from a raw core event without
    /// interpreting lifecycle state. Durable sessions render state solely from
    /// canonical snapshots.
    pub(crate) fn apply_observation(&mut self, event: &FfiTransferEvent) {
        if !event.activity_id.is_empty() {
            self.activity_id = event.activity_id.clone();
        }
        self.updated_at_ms = event.ts_ms;
        if event.direction != FfiTransferDirection::Unknown {
            self.direction = event.direction;
        }
        if event.mode != FfiTransferMode::Unknown {
            self.mode = event.mode;
        }
        if !event.transfer_id.is_empty() {
            self.transfer_id = event.transfer_id.clone();
        }
        if !event.file_name.is_empty() {
            self.file_name = event.file_name.clone();
        }
        if event.total_bytes > 0 {
            self.total_bytes = event.total_bytes;
        }
        if event.bytes_transferred > 0 || event.kind == FfiTransferEventKind::Progress {
            self.bytes_transferred = event.bytes_transferred;
        }
        if event.bytes_resumed > 0 {
            self.bytes_resumed = event.bytes_resumed;
            self.bytes_transferred = self.bytes_transferred.max(event.bytes_resumed);
        }
        if event.data_path_kind != FfiDataPathKind::None {
            self.data_path_kind = event.data_path_kind;
            self.data_path_detail = event.data_path_detail.clone();
        }
        if !event.invite.is_empty() {
            self.invite = event.invite.clone();
        }
        if !event.token.is_empty() {
            self.token = event.token.clone();
        }
        if !event.peer_descriptor.is_empty() {
            self.peer_descriptor = event.peer_descriptor.clone();
        }
        if !event.diagnostic_message.is_empty() {
            self.diagnostic_message = event.diagnostic_message.clone();
        }
    }

    pub(crate) fn apply_failure(&mut self, failure: &FfiTransferFailure, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Failed;
        self.apply_failure_metadata(failure);
    }

    pub(crate) fn apply_publication_failure(&mut self, failure: &FfiTransferFailure, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = 0;
        self.state = FfiTransferActivityState::Publishing;
        self.apply_failure_metadata(failure);
    }

    fn apply_failure_metadata(&mut self, failure: &FfiTransferFailure) {
        if failure.direction != FfiTransferDirection::Unknown {
            self.direction = failure.direction;
        }
        if !failure.transfer_id.is_empty() {
            self.transfer_id = failure.transfer_id.clone();
        }
        self.diagnostic_message = failure.diagnostic_message.clone();
        self.failure_code = failure.code;
        self.failure_category = failure.category;
        self.failure_phase = failure.phase;
        self.failure_origin = failure.origin;
        self.user_message_key = failure.user_message_key.clone();
        self.retryable = failure.retryable;
        self.recovery_action = failure.recovery_action;
    }

    pub(crate) fn clear_failure_metadata(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.diagnostic_message.clear();
        self.failure_code = FfiFailureCode::Unknown;
        self.failure_category = FfiFailureCategory::Unknown;
        self.failure_phase = FfiFailurePhase::Setup;
        self.failure_origin = FfiFailureOrigin::Unknown;
        self.user_message_key.clear();
        self.retryable = false;
        self.recovery_action = FfiRecoveryAction::None;
    }

    fn apply_completed(
        &mut self,
        summary: &TransferSummary,
        ts_ms: u64,
        completed_file_path: String,
    ) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Completed;
        self.bytes_transferred = summary.bytes_transferred;
        self.total_bytes = self.total_bytes.max(summary.bytes_transferred);
        self.completed_file_path = completed_file_path;
    }

    pub(crate) fn apply_canceled(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Canceled;
        self.diagnostic_message = "canceled".to_string();
        self.failure_code = FfiFailureCode::UserCanceled;
        self.failure_category = FfiFailureCategory::User;
        self.failure_phase = FfiFailurePhase::Transferring;
        self.failure_origin = FfiFailureOrigin::Local;
        self.user_message_key = "transfer.user_canceled".to_string();
        self.retryable = false;
        self.recovery_action = FfiRecoveryAction::None;
    }

    pub(crate) fn apply_paused(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Paused;
        self.diagnostic_message = "paused".to_string();
        self.failure_code = FfiFailureCode::UserCanceled;
        self.failure_category = FfiFailureCategory::User;
        self.failure_phase = FfiFailurePhase::Transferring;
        self.failure_origin = FfiFailureOrigin::Local;
        self.user_message_key = "transfer.paused".to_string();
        self.retryable = true;
        self.recovery_action = FfiRecoveryAction::Resume;
    }

    fn apply_peer_paused(&mut self, ts_ms: u64) {
        self.apply_paused(ts_ms);
        self.diagnostic_message = "peer paused; waiting for resume".to_string();
        self.failure_origin = FfiFailureOrigin::Peer;
    }

    pub(crate) fn apply_requeued(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = 0;
        self.attempt_id.clear();
        self.state = FfiTransferActivityState::Queued;
        self.diagnostic_message.clear();
        self.failure_code = FfiFailureCode::Unknown;
        self.failure_category = FfiFailureCategory::Unknown;
        self.failure_phase = FfiFailurePhase::Setup;
        self.failure_origin = FfiFailureOrigin::Unknown;
        self.user_message_key.clear();
        self.retryable = false;
        self.recovery_action = FfiRecoveryAction::None;
    }
}

fn request_file_name(request: &FfiTransferRequest) -> String {
    let path = match request.direction {
        FfiTransferDirection::Send => request.file_path.trim(),
        FfiTransferDirection::Receive | FfiTransferDirection::Unknown => "",
    };
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn next_activity_id() -> String {
    let id = NEXT_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    format!("ffi-{id}")
}

pub(crate) fn next_attempt_id() -> String {
    let id = NEXT_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed);
    format!("attempt-{id}")
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) async fn drive_durable_notices(
    activity_id: String,
    context: SessionContext,
    mut notices: mpsc::UnboundedReceiver<SessionNotice>,
    activity: Arc<Mutex<FfiTransferActivityRecord>>,
    observer: Arc<dyn TransferObserver>,
    mailbox: NativeMailboxObserver,
    pending_receipt_key: Arc<Mutex<Option<String>>>,
) {
    let mut previous_session = None;
    let mut last_progress_event_ms = 0;
    let mut last_progress_activity_ms = 0;

    while let Some(notice) = notices.recv().await {
        match notice {
            SessionNotice::Event(event) => {
                observe_durable_transfer_event(
                    &*observer,
                    &activity,
                    event,
                    &mut last_progress_event_ms,
                );
            }
            SessionNotice::Snapshot(snapshot) => {
                let timestamp = now_ms();
                let progress_only = previous_session
                    .as_ref()
                    .is_some_and(|previous| is_progress_only_snapshot(previous, &snapshot));
                let terminal_transition = previous_session
                    .as_ref()
                    .is_none_or(|previous| previous.state != snapshot.session.state);
                let emit_activity = !progress_only
                    || snapshot.session.total > 0
                        && snapshot.session.bytes >= snapshot.session.total
                    || last_progress_activity_ms == 0
                    || timestamp.saturating_sub(last_progress_activity_ms)
                        >= NATIVE_PROGRESS_INTERVAL_MS;

                let record = {
                    let mut record = activity.lock().unwrap();
                    apply_canonical_snapshot(&mut record, &snapshot, &context, timestamp);
                    record.clone()
                };
                if emit_activity {
                    if progress_only {
                        last_progress_activity_ms = timestamp;
                    }
                    observer.on_transfer_activity(record.clone());
                }
                if terminal_transition {
                    report_durable_state_transition(&*observer, &record);
                }
                previous_session = Some(snapshot.session);
            }
            SessionNotice::FetchReceipt { key, server } => {
                *pending_receipt_key.lock().unwrap() = Some(key.clone());
                mailbox.fetch(activity_id.clone(), key, server);
            }
            SessionNotice::PostReceipt { key, blob, server } => {
                mailbox.post(activity_id.clone(), key, blob, server);
            }
        }
    }
}

fn is_progress_only_snapshot(
    previous: &envoix_client::api::machine::Session,
    snapshot: &SessionSnapshot,
) -> bool {
    if previous.bytes == snapshot.session.bytes {
        return false;
    }
    let mut without_progress = snapshot.session.clone();
    without_progress.bytes = previous.bytes;
    &without_progress == previous
}

fn observe_durable_transfer_event(
    observer: &dyn TransferObserver,
    activity: &Arc<Mutex<FfiTransferActivityRecord>>,
    event: StampedEvent,
    last_native_progress_ms: &mut u64,
) {
    let ffi_event = {
        let mut activity = activity.lock().unwrap();
        let ffi_event = to_ffi_event(&event, &activity.activity_id);
        activity.apply_observation(&ffi_event);
        ffi_event
    };
    if !should_emit_native_event(&ffi_event, last_native_progress_ms) {
        return;
    }
    observer.on_transfer_event(ffi_event);
    emit_transfer_event_callbacks(observer, event.event);
}

fn report_durable_state_transition(
    observer: &dyn TransferObserver,
    activity: &FfiTransferActivityRecord,
) {
    match activity.state {
        FfiTransferActivityState::Completed => {
            observer.on_status("completed".to_string());
            observer.on_completed(activity.bytes_transferred);
        }
        FfiTransferActivityState::Failed | FfiTransferActivityState::Canceled => {
            let failure = failure_from_activity(activity);
            observer.on_status(activity.diagnostic_message.clone());
            observer.on_transfer_failed(failure);
            observer.on_failed(activity.diagnostic_message.clone());
        }
        FfiTransferActivityState::Paused => {
            observer.on_status(activity.diagnostic_message.clone());
        }
        FfiTransferActivityState::Unconfirmed => {
            observer.on_status("delivery unconfirmed; checking receipt".to_string());
        }
        FfiTransferActivityState::Publishing => {
            observer.on_status("publishing received file".to_string());
        }
        _ => {}
    }
}

fn failure_from_activity(activity: &FfiTransferActivityRecord) -> FfiTransferFailure {
    FfiTransferFailure {
        code: activity.failure_code,
        category: activity.failure_category,
        phase: activity.failure_phase,
        origin: activity.failure_origin,
        direction: activity.direction,
        transfer_id: activity.transfer_id.clone(),
        attempt_id: activity.attempt_id.clone(),
        retryable: activity.retryable,
        recovery_action: activity.recovery_action,
        user_message_key: activity.user_message_key.clone(),
        diagnostic_message: activity.diagnostic_message.clone(),
    }
}

pub(crate) fn canonical_context_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<SessionContext, EnvoixError> {
    let config_path = if request.config_path.trim().is_empty() {
        settings.config_path.trim()
    } else {
        request.config_path.trim()
    };
    let client =
        ClientContext::from_config_path((!config_path.is_empty()).then(|| Path::new(config_path)))
            .map_err(op_err)?;
    let mut sources = Vec::new();
    for attempt in peer_sources_for_request(settings, request)? {
        if !sources.contains(&attempt.source) {
            sources.push(attempt.source);
        }
    }
    let direction = match request.direction {
        FfiTransferDirection::Send => TransferDirection::Send,
        FfiTransferDirection::Receive => TransferDirection::Receive,
        FfiTransferDirection::Unknown => {
            return Err(EnvoixError::Operation {
                reason: "transfer direction must not be unknown".to_string(),
            });
        }
    };
    let path = match request.direction {
        FfiTransferDirection::Send => required_path(&request.file_path, "file_path")?,
        FfiTransferDirection::Receive => required_path(&request.output_dir, "output_dir")?,
        FfiTransferDirection::Unknown => unreachable!(),
    };
    Ok(SessionContext {
        client,
        params: SessionParams {
            direction,
            path: path.into(),
            sources,
            options: transfer_options_for_request(settings, request, None)?,
            publication_required: request.publication_required,
        },
    })
}

pub(crate) fn normalized_receipt_server(value: &str) -> Result<Option<String>, EnvoixError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(char::is_whitespace)
        || !["http://", "https://"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
    {
        return Err(EnvoixError::Operation {
            reason: "receipt_server must be an HTTP(S) endpoint".to_string(),
        });
    }
    Ok(Some(value.trim_end_matches('/').to_string()))
}

pub(crate) fn activity_from_canonical_record(record: &TransferRecord) -> FfiTransferActivityRecord {
    let activity_id = external_activity_id(record);
    let mut request = request_from_canonical_context(&activity_id, &record.context);
    request.activity_id = activity_id;
    let mut activity = FfiTransferActivityRecord::from_request(&request, record.created_ms);
    let mut session = record.session.clone();
    if matches!(
        session.state,
        CanonicalState::Waiting
            | CanonicalState::Connecting
            | CanonicalState::Verifying
            | CanonicalState::Transferring
            | CanonicalState::Confirming
    ) {
        session.state = CanonicalState::Paused(PauseOrigin::Lost);
        session.reason = Some("interrupted by an app restart".to_string());
    }
    apply_canonical_snapshot(
        &mut activity,
        &SessionSnapshot {
            seq: 0,
            speed_bps: 0.0,
            avg_bps: 0.0,
            session,
        },
        &record.context,
        record.updated_ms,
    );
    if activity.state == FfiTransferActivityState::Publishing
        && let Some(failure) =
            native_publication_metadata(record).and_then(|publication| publication.failure)
    {
        activity.apply_publication_failure(&failure, record.updated_ms);
    }
    activity
}

fn native_publication_metadata(record: &TransferRecord) -> Option<PersistedNativePublication> {
    record
        .platform_extras
        .as_ref()
        .and_then(native_publication_metadata_from_extras)
}

pub(crate) fn native_publication_metadata_from_extras(
    extras: &serde_json::Value,
) -> Option<PersistedNativePublication> {
    extras
        .as_object()
        .and_then(|extras| extras.get(NATIVE_PUBLICATION_EXTRAS_KEY))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub(crate) fn external_activity_id(record: &TransferRecord) -> String {
    record
        .platform_extras
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|extras| extras.get(EXTERNAL_RECORD_ID_KEY))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| record.id.to_string())
}

pub(crate) fn request_from_canonical_context(
    activity_id: &str,
    context: &SessionContext,
) -> FfiTransferRequest {
    let mode = context
        .params
        .sources
        .first()
        .map(|source| ffi_transfer_mode(source.mode()))
        .unwrap_or(FfiTransferMode::Unknown);
    let path = context.params.path.to_string_lossy().into_owned();
    let mut request = match context.params.direction {
        TransferDirection::Send => FfiTransferRequest::send(path, mode),
        TransferDirection::Receive => FfiTransferRequest::receive(path, mode),
    };
    request.activity_id = activity_id.to_string();
    request.resume = context.params.options.resume;
    request.publication_required = context.params.publication_required;
    request.relay = context.params.options.relay.clone().unwrap_or_default();
    request.path_policy = match context.params.options.path {
        PathPolicy::Auto => FfiPathPolicy::Auto,
        PathPolicy::RelayOnly => FfiPathPolicy::RelayOnly,
        PathPolicy::DirectOnly => FfiPathPolicy::DirectOnly,
    };
    if let Some(source) = context.params.sources.first() {
        match source {
            PeerSource::Manual { peer, token } => {
                request.peer_descriptor = peer.to_string();
                request.token = token.clone();
            }
            PeerSource::Invite { invite } => request.invite = invite.clone(),
            PeerSource::ShowManual { token } | PeerSource::Mdns { token } => {
                request.token = token.clone().unwrap_or_default();
            }
            PeerSource::ShowInvite { .. } => {}
            PeerSource::Room { code, broker } => {
                request.code = code.clone();
                request.broker = broker.clone();
            }
        }
    }
    request
}

pub(crate) fn apply_canonical_snapshot(
    activity: &mut FfiTransferActivityRecord,
    snapshot: &SessionSnapshot,
    context: &SessionContext,
    ts_ms: u64,
) {
    let session = &snapshot.session;
    let previous_state = activity.state;
    activity.updated_at_ms = ts_ms;
    activity.sequence = snapshot.seq;
    activity.attempt_id = format!("attempt-{}", session.attempt);
    activity.direction = ffi_direction(Some(session.direction));
    activity.transfer_id = session.transfer_id.clone().unwrap_or_default();
    activity.file_name = session.file_name.clone().unwrap_or_default();
    activity.bytes_transferred = session.bytes;
    activity.total_bytes = session.total;
    activity.bytes_resumed = session.bytes_resumed;
    activity.speed_bps = snapshot.speed_bps.max(0.0).round() as u64;
    activity.average_speed_bps = snapshot.avg_bps.max(0.0).round() as u64;
    if let Some(path) = &session.path {
        let (kind, detail) = ffi_data_path(path);
        activity.data_path_kind = kind;
        activity.data_path_detail = detail;
    }
    if activity.started_at_ms == 0
        && session.transfer_id.is_some()
        && matches!(
            session.state,
            CanonicalState::Verifying
                | CanonicalState::Transferring
                | CanonicalState::Confirming
                | CanonicalState::AwaitingPublication
                | CanonicalState::Completed
        )
    {
        activity.started_at_ms = ts_ms;
    }
    if matches!(
        session.state,
        CanonicalState::Waiting
            | CanonicalState::Connecting
            | CanonicalState::Verifying
            | CanonicalState::Transferring
            | CanonicalState::Confirming
            | CanonicalState::AwaitingPublication
            | CanonicalState::Completed
    ) {
        activity.completed_at_ms = 0;
        activity.completed_file_path.clear();
        activity.failure_code = FfiFailureCode::Unknown;
        activity.failure_category = FfiFailureCategory::Unknown;
        activity.failure_phase = FfiFailurePhase::Setup;
        activity.failure_origin = FfiFailureOrigin::Unknown;
        activity.user_message_key.clear();
        activity.retryable = false;
        activity.recovery_action = FfiRecoveryAction::None;
        if matches!(
            previous_state,
            FfiTransferActivityState::Paused
                | FfiTransferActivityState::Unconfirmed
                | FfiTransferActivityState::Publishing
                | FfiTransferActivityState::Failed
                | FfiTransferActivityState::Canceled
                | FfiTransferActivityState::Completed
        ) || session.state == CanonicalState::Completed
        {
            activity.diagnostic_message.clear();
        }
    }
    activity.state = match session.state {
        CanonicalState::Preparing => FfiTransferActivityState::Queued,
        CanonicalState::Waiting => FfiTransferActivityState::WaitingForPeer,
        CanonicalState::Connecting => FfiTransferActivityState::Connecting,
        CanonicalState::Verifying => FfiTransferActivityState::Verifying,
        CanonicalState::Transferring => FfiTransferActivityState::Transferring,
        CanonicalState::Confirming => {
            activity.diagnostic_message = "confirming".to_string();
            FfiTransferActivityState::Verifying
        }
        CanonicalState::Paused(origin) => {
            activity.completed_at_ms = 0;
            activity.completed_file_path.clear();
            activity.failure_phase = FfiFailurePhase::Transferring;
            activity.retryable = true;
            activity.recovery_action = FfiRecoveryAction::Resume;
            (
                activity.failure_code,
                activity.failure_category,
                activity.failure_origin,
            ) = match origin {
                PauseOrigin::Local => (
                    FfiFailureCode::UserCanceled,
                    FfiFailureCategory::User,
                    FfiFailureOrigin::Local,
                ),
                PauseOrigin::Peer => (
                    FfiFailureCode::UserCanceled,
                    FfiFailureCategory::User,
                    FfiFailureOrigin::Peer,
                ),
                PauseOrigin::Lost => (
                    FfiFailureCode::NetworkLost,
                    FfiFailureCategory::Network,
                    FfiFailureOrigin::Unknown,
                ),
            };
            activity.user_message_key = match origin {
                PauseOrigin::Local => "transfer.paused",
                PauseOrigin::Peer => "transfer.peer_paused",
                PauseOrigin::Lost => "transfer.network_lost",
            }
            .to_string();
            activity.diagnostic_message = session.reason.clone().unwrap_or_else(|| match origin {
                PauseOrigin::Local => "paused".to_string(),
                PauseOrigin::Peer => "paused by peer".to_string(),
                PauseOrigin::Lost => "connection lost; partial retained".to_string(),
            });
            FfiTransferActivityState::Paused
        }
        CanonicalState::Unconfirmed => {
            activity.completed_at_ms = 0;
            activity.completed_file_path.clear();
            activity.failure_code = FfiFailureCode::NetworkLost;
            activity.failure_category = FfiFailureCategory::Network;
            activity.failure_phase = FfiFailurePhase::Acknowledging;
            activity.failure_origin = FfiFailureOrigin::Unknown;
            activity.user_message_key = "transfer.delivery_unconfirmed".to_string();
            activity.retryable = true;
            activity.recovery_action = FfiRecoveryAction::Resume;
            activity.diagnostic_message =
                "delivery unconfirmed; awaiting completion receipt".to_string();
            FfiTransferActivityState::Unconfirmed
        }
        CanonicalState::AwaitingPublication => {
            activity.completed_file_path = canonical_completed_file_path(context, session);
            activity.diagnostic_message = "publishing".to_string();
            FfiTransferActivityState::Publishing
        }
        CanonicalState::Completed => {
            activity.completed_at_ms = ts_ms;
            activity.completed_file_path = canonical_completed_file_path(context, session);
            FfiTransferActivityState::Completed
        }
        CanonicalState::Failed => {
            activity.completed_at_ms = ts_ms;
            activity.completed_file_path.clear();
            apply_canonical_failure(activity, session);
            FfiTransferActivityState::Failed
        }
        CanonicalState::Cancelled => {
            activity.completed_at_ms = ts_ms;
            activity.apply_canceled(ts_ms);
            FfiTransferActivityState::Canceled
        }
    };
}

fn canonical_completed_file_path(
    context: &SessionContext,
    session: &envoix_client::api::machine::Session,
) -> String {
    if let Some(path) = &session.completed_file_path {
        return path.clone();
    }
    if context.params.direction != TransferDirection::Receive {
        return String::new();
    }
    let Some(file_name) = session.file_name.as_deref() else {
        return String::new();
    };
    context
        .params
        .path
        .join(file_name)
        .to_string_lossy()
        .into_owned()
}

fn apply_canonical_failure(
    activity: &mut FfiTransferActivityRecord,
    session: &envoix_client::api::machine::Session,
) {
    if let Some(failure) = &session.failure {
        activity.failure_code = ffi_failure_code(failure.code);
        activity.failure_category = ffi_failure_category(failure.category);
        activity.failure_phase = ffi_failure_phase(failure.phase);
        activity.failure_origin = ffi_failure_origin(failure.origin);
        if let Some(direction) = failure.direction {
            activity.direction = ffi_direction(Some(direction));
        }
        if let Some(transfer_id) = &failure.transfer_id {
            activity.transfer_id = transfer_id.clone();
        }
        if let Some(attempt_id) = &failure.attempt_id {
            activity.attempt_id = attempt_id.clone();
        }
        activity.retryable = failure.retryable;
        activity.recovery_action = ffi_recovery_action(failure.recovery_action);
        activity.user_message_key = failure.user_message_key.clone();
        activity.diagnostic_message = failure.diagnostic_message.clone();
        return;
    }

    let reason_code = session.reason_code;
    activity.diagnostic_message = session
        .reason
        .as_deref()
        .unwrap_or("transfer failed")
        .to_string();
    activity.retryable = false;
    activity.recovery_action = FfiRecoveryAction::None;
    match reason_code.unwrap_or(SessionFailureCode::Other) {
        SessionFailureCode::Cancelled => {
            activity.failure_code = FfiFailureCode::UserCanceled;
            activity.failure_category = FfiFailureCategory::User;
            activity.failure_origin = FfiFailureOrigin::Local;
        }
        SessionFailureCode::PeerCancelled => {
            activity.failure_code = FfiFailureCode::PeerCanceled;
            activity.failure_category = FfiFailureCategory::User;
            activity.failure_origin = FfiFailureOrigin::Peer;
        }
        SessionFailureCode::ConnectionLost => {
            activity.failure_code = FfiFailureCode::NetworkLost;
            activity.failure_category = FfiFailureCategory::Network;
            activity.failure_origin = FfiFailureOrigin::Unknown;
            activity.retryable = true;
            activity.recovery_action = FfiRecoveryAction::Resume;
        }
        SessionFailureCode::Paused | SessionFailureCode::PeerPaused => {
            activity.failure_code = FfiFailureCode::UserCanceled;
            activity.failure_category = FfiFailureCategory::User;
            activity.failure_origin = if reason_code == Some(SessionFailureCode::PeerPaused) {
                FfiFailureOrigin::Peer
            } else {
                FfiFailureOrigin::Local
            };
            activity.retryable = true;
            activity.recovery_action = FfiRecoveryAction::Resume;
        }
        SessionFailureCode::Other => {
            activity.failure_code = FfiFailureCode::Unknown;
            activity.failure_category = FfiFailureCategory::Unknown;
            activity.failure_origin = FfiFailureOrigin::Unknown;
        }
        _ => {
            activity.failure_code = FfiFailureCode::Unknown;
            activity.failure_category = FfiFailureCategory::Unknown;
            activity.failure_origin = FfiFailureOrigin::Unknown;
        }
    }
}

pub(crate) fn build_client_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Client, EnvoixError> {
    let config_path = request.config_path.trim();
    let config_path = if config_path.is_empty() {
        settings.config_path.trim()
    } else {
        config_path
    };
    let path = if config_path.is_empty() {
        None
    } else {
        Some(Path::new(config_path))
    };
    Client::from_runtime_sources(path).map_err(|error| EnvoixError::Operation {
        reason: error.to_string(),
    })
}

pub(crate) fn transfer_options_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    path_policy_override: Option<FfiPathPolicy>,
) -> Result<TransferOptions, EnvoixError> {
    let mut options = TransferOptions::default();
    options.relay = relay_url_for_request(settings, request);
    let effective_path_policy = path_policy_override.unwrap_or(request.path_policy);
    if effective_path_policy == FfiPathPolicy::RelayOnly && options.relay.is_none() {
        return Err(EnvoixError::Operation {
            reason: "relay-only transfers require a relay URL".to_string(),
        });
    }
    options.path = path_policy(effective_path_policy);
    options.resume = request.resume;
    options.listen_addrs = Some(receive_addrs());
    Ok(options)
}

fn receive_addrs() -> BindAddrs {
    BindAddrs::dual_stack(0)
}

pub(crate) fn peer_sources_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Vec<TransferAttemptSource>, EnvoixError> {
    let single = |mode, source| Ok(vec![TransferAttemptSource::new(mode, source)]);
    match request.mode {
        FfiTransferMode::Manual => single(
            FfiTransferMode::Manual,
            PeerSource::Manual {
                peer: required_peer_descriptor(&request.peer_descriptor)?,
                token: required_value(&request.token, "token")?,
            },
        ),
        FfiTransferMode::Invite => {
            let invite = required_value(&request.invite, "invite")?;
            let source = PeerSource::Invite {
                invite: invite.clone(),
            };
            let mut sources = vec![TransferAttemptSource::new(
                FfiTransferMode::Invite,
                source.clone(),
            )];
            if should_retry_invite_relay_only(settings, request, &invite) {
                sources.push(
                    TransferAttemptSource::new(FfiTransferMode::Invite, source)
                        .with_path_policy(FfiPathPolicy::RelayOnly),
                );
            }
            Ok(sources)
        }
        FfiTransferMode::ShowManual => single(
            FfiTransferMode::ShowManual,
            PeerSource::ShowManual {
                token: optional_value(&request.token),
            },
        ),
        FfiTransferMode::ShowInvite => single(
            FfiTransferMode::ShowInvite,
            PeerSource::ShowInvite {
                ttl_secs: INVITE_TTL_SECS,
                token: None,
            },
        ),
        FfiTransferMode::Mdns => single(
            FfiTransferMode::Mdns,
            PeerSource::Mdns {
                token: optional_value(&request.token),
            },
        ),
        FfiTransferMode::Room => {
            let code = required_value(&request.code, "code")?;
            let mut sources = Vec::new();
            if request.rendezvous.use_room && request.rendezvous.internet_available {
                let source = PeerSource::Room {
                    code: code.clone(),
                    broker: rendezvous_broker_for_request(settings, request),
                };
                sources.push(TransferAttemptSource {
                    mode: FfiTransferMode::Room,
                    source: source.clone(),
                    path_policy_override: None,
                });
                if should_retry_room_relay_only(settings, request) {
                    sources.push(
                        TransferAttemptSource::new(FfiTransferMode::Room, source)
                            .with_path_policy(FfiPathPolicy::RelayOnly),
                    );
                }
            }
            if request.rendezvous.use_mdns {
                sources.push(TransferAttemptSource {
                    mode: FfiTransferMode::Mdns,
                    source: PeerSource::Mdns { token: Some(code) },
                    path_policy_override: None,
                });
            }
            if sources.is_empty() {
                let message = if request.rendezvous.use_room
                    && !request.rendezvous.internet_available
                    && !request.rendezvous.use_mdns
                {
                    "room rendezvous is disabled while internet is unavailable and mDNS fallback is disabled"
                } else {
                    "at least one rendezvous route must be enabled"
                };
                Err(EnvoixError::Operation {
                    reason: message.to_string(),
                })
            } else {
                Ok(sources)
            }
        }
        FfiTransferMode::Unknown => Err(EnvoixError::Operation {
            reason: "transfer mode must not be unknown".to_string(),
        }),
    }
}

fn should_retry_invite_relay_only(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    invite: &str,
) -> bool {
    if request.direction != FfiTransferDirection::Send
        || request.path_policy != FfiPathPolicy::Auto
        || relay_url_for_request(settings, request).is_none()
    {
        return false;
    }
    QrInvitePayload::decode(invite)
        .map(|payload| !payload.relay_urls.is_empty())
        .unwrap_or(false)
}

fn should_retry_room_relay_only(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> bool {
    request.path_policy == FfiPathPolicy::Auto && relay_url_for_request(settings, request).is_some()
}

fn path_policy(policy: FfiPathPolicy) -> PathPolicy {
    match policy {
        FfiPathPolicy::Auto => PathPolicy::Auto,
        FfiPathPolicy::RelayOnly => PathPolicy::RelayOnly,
        FfiPathPolicy::DirectOnly => PathPolicy::DirectOnly,
    }
}

fn rendezvous_broker_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> String {
    let broker = request.broker.trim();
    let broker = if broker.is_empty() {
        settings.server_url.trim()
    } else {
        broker
    };
    if broker.is_empty() {
        DEFAULT_RENDEZVOUS_BROKER.to_string()
    } else {
        broker.to_string()
    }
}

fn relay_url_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Option<String> {
    let relay = request.relay.trim();
    let relay = if relay.is_empty() {
        settings.relay_url.trim()
    } else {
        relay
    };
    if relay.is_empty() {
        if request.broker.trim().is_empty() && settings.server_url.trim().is_empty() {
            Some(DEFAULT_RELAY_URL.to_string())
        } else {
            None
        }
    } else {
        Some(relay.to_string())
    }
}

pub(crate) fn required_path(value: &str, field: &str) -> Result<String, EnvoixError> {
    required_value(value, field)
}

pub(crate) fn required_value(value: &str, field: &str) -> Result<String, EnvoixError> {
    let value = value.trim();
    if value.is_empty() {
        Err(EnvoixError::Operation {
            reason: format!("{field} must not be empty"),
        })
    } else {
        Ok(value.to_string())
    }
}

fn optional_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn broker_for_pairing_invite(broker: &str) -> String {
    let broker = broker.trim();
    if broker.is_empty() {
        DEFAULT_RENDEZVOUS_BROKER.to_string()
    } else {
        broker.to_string()
    }
}

pub(crate) fn relay_for_pairing_invite(broker: &str, relay: &str) -> Option<String> {
    let relay = relay.trim();
    if !relay.is_empty() {
        Some(relay.to_string())
    } else if broker.trim().is_empty() {
        Some(DEFAULT_RELAY_URL.to_string())
    } else {
        None
    }
}

pub(crate) fn core_invite_role(role: FfiInviteRole) -> Option<Role> {
    match role {
        FfiInviteRole::Send => Some(Role::Send),
        FfiInviteRole::Receive => Some(Role::Receive),
        FfiInviteRole::Unknown => None,
    }
}

fn ffi_invite_role(role: Option<Role>) -> FfiInviteRole {
    match role {
        Some(Role::Send) => FfiInviteRole::Send,
        Some(Role::Receive) => FfiInviteRole::Receive,
        None => FfiInviteRole::Unknown,
    }
}

fn required_peer_descriptor(value: &str) -> Result<PeerDescriptor, EnvoixError> {
    let value = required_value(value, "peer_descriptor")?;
    PeerDescriptor::parse_compact(&value).map_err(op_err)
}

fn completed_file_path_for_request(
    request: &FfiTransferRequest,
    summary: &TransferSummary,
) -> String {
    if request.direction != FfiTransferDirection::Receive
        || request.output_dir.trim().is_empty()
        || summary.file_name.is_empty()
    {
        return String::new();
    }
    Path::new(&request.output_dir)
        .join(&summary.file_name)
        .to_string_lossy()
        .into_owned()
}

fn request_debug_summary(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    attempts: &[TransferAttemptSource],
) -> String {
    let direction = transfer_direction_label(request.direction);
    let mode = transfer_mode_label(request.mode);
    let routes = attempts
        .iter()
        .map(|attempt| transfer_mode_label(attempt.mode))
        .collect::<Vec<_>>()
        .join(" -> ");
    let path = match request.direction {
        FfiTransferDirection::Send => file_label(&request.file_path),
        FfiTransferDirection::Receive => dir_label(&request.output_dir),
        FfiTransferDirection::Unknown => "path=<unknown>".to_string(),
    };
    let room = optional_code_room(&request.code)
        .map(|room| format!(" room={room}"))
        .unwrap_or_default();
    let broker = effective_broker_label(settings, request);
    let relay = effective_relay_label(settings, request);
    format!(
        "request direction={direction} mode={mode}{room} routes=[{routes}] {path} broker={broker} relay={relay} internet={}",
        request.rendezvous.internet_available,
    )
}

fn attempt_debug_summary(index: usize, count: usize, attempt: &TransferAttemptSource) -> String {
    let path = attempt
        .path_policy_override
        .map(|policy| format!(" path={}", path_policy_label(policy)))
        .unwrap_or_default();
    format!(
        "attempt {}/{} via {}{} {}",
        index + 1,
        count,
        transfer_mode_label(attempt.mode),
        path,
        peer_source_debug(&attempt.source)
    )
}

fn path_policy_label(policy: FfiPathPolicy) -> &'static str {
    match policy {
        FfiPathPolicy::Auto => "auto",
        FfiPathPolicy::RelayOnly => "relay-only",
        FfiPathPolicy::DirectOnly => "direct-only",
    }
}

fn peer_source_debug(source: &PeerSource) -> String {
    match source {
        PeerSource::Manual { .. } => "source=manual".to_string(),
        PeerSource::Invite { invite } => invite_source_debug(invite),
        PeerSource::ShowManual { token } => {
            format!("source=show-manual token={}", token_state(token.as_deref()))
        }
        PeerSource::ShowInvite { ttl_secs, token } => format!(
            "source=show-invite ttl={ttl_secs}s token={}",
            token_state(token.as_deref())
        ),
        PeerSource::Mdns { token } => format!(
            "source=mdns token={}",
            token
                .as_deref()
                .and_then(optional_code_room)
                .unwrap_or_else(|| token_state(token.as_deref()))
        ),
        PeerSource::Room { code, broker } => {
            format!(
                "source=room room={} broker={}",
                code_room(code),
                broker_label(broker)
            )
        }
    }
}

fn invite_source_debug(invite: &str) -> String {
    let Ok(payload) = QrInvitePayload::decode(invite) else {
        return format!("source=invite len={} parse=failed", invite.len());
    };
    format!(
        "source=invite len={} endpoint={} direct={} relay={}",
        invite.len(),
        short_endpoint_id(&payload.peer.endpoint_id),
        payload.peer.direct_addrs.len(),
        payload.relay_urls.len(),
    )
}

fn advertised_endpoint_debug(peer: &PeerDescriptor, invite: Option<&str>) -> String {
    let relay_count = invite
        .and_then(|value| QrInvitePayload::decode(value).ok())
        .map(|payload| payload.relay_urls.len())
        .unwrap_or(0);
    format!(
        "advertised endpoint={} direct={} relay={}",
        short_endpoint_id(&peer.endpoint_id),
        peer.direct_addrs.len(),
        relay_count,
    )
}

fn short_endpoint_id(endpoint_id: &str) -> String {
    endpoint_id.chars().take(12).collect()
}

fn transfer_direction_label(direction: FfiTransferDirection) -> &'static str {
    match direction {
        FfiTransferDirection::Send => "send",
        FfiTransferDirection::Receive => "receive",
        FfiTransferDirection::Unknown => "unknown",
    }
}

fn file_label(path: &str) -> String {
    let name = Path::new(path.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<none>".to_string());
    format!("file={name}")
}

fn dir_label(path: &str) -> String {
    let name = Path::new(path.trim())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<none>".to_string());
    format!("output_dir={name}")
}

fn effective_broker_label(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> String {
    let broker = request.broker.trim();
    let broker = if broker.is_empty() {
        settings.server_url.trim()
    } else {
        broker
    };
    if broker.is_empty() {
        "default".to_string()
    } else {
        broker_label(broker)
    }
}

fn effective_relay_label(settings: &EnvoixRuntimeSettings, request: &FfiTransferRequest) -> String {
    let relay = request.relay.trim();
    let relay = if relay.is_empty() {
        settings.relay_url.trim()
    } else {
        relay
    };
    if relay.is_empty() {
        if request.broker.trim().is_empty() && settings.server_url.trim().is_empty() {
            "default".to_string()
        } else {
            "none".to_string()
        }
    } else {
        relay.to_string()
    }
}

fn broker_label(broker: &str) -> String {
    broker
        .split_once('@')
        .map(|(_, addr)| addr.to_string())
        .unwrap_or_else(|| broker.to_string())
}

fn optional_code_room(code: &str) -> Option<&str> {
    let code = code.trim();
    if code.is_empty() {
        None
    } else {
        Some(code_room(code))
    }
}

fn code_room(code: &str) -> &str {
    code.split('-').next().unwrap_or(code)
}

fn token_state(token: Option<&str>) -> &'static str {
    if token.is_some_and(|token| !token.trim().is_empty()) {
        "set"
    } else {
        "generated"
    }
}

/// Reports the single terminal outcome from the awaited operation result.
fn report_terminal(
    observer: &dyn TransferObserver,
    activity: &mut FfiTransferActivityRecord,
    request: &FfiTransferRequest,
    result: Result<TransferSummary, TransferError>,
    direction: Option<TransferDirection>,
    cancel_requested: bool,
) {
    match result {
        Ok(summary) => {
            let completed_file_path = completed_file_path_for_request(request, &summary);
            activity.apply_completed(&summary, now_ms(), completed_file_path);
            observer.on_transfer_activity(activity.clone());
            observer.on_completed(summary.bytes_transferred);
        }
        Err(error) => {
            let mut failure = to_ffi_failure(&error, direction, activity);
            if cancel_requested {
                activity.apply_canceled(now_ms());
                failure.code = FfiFailureCode::UserCanceled;
                failure.category = FfiFailureCategory::User;
                failure.origin = FfiFailureOrigin::Local;
                failure.retryable = false;
                failure.recovery_action = FfiRecoveryAction::None;
                failure.user_message_key = "transfer.user_canceled".to_string();
                failure.diagnostic_message = "canceled".to_string();
            } else {
                activity.apply_failure(&failure, now_ms());
            }
            observer.on_transfer_activity(activity.clone());
            observer.on_transfer_failed(failure);
            observer.on_failed(error.to_string());
        }
    }
}

pub(crate) async fn drive_transfer_request(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    mut activity: FfiTransferActivityRecord,
    observer: Arc<dyn TransferObserver>,
    mut control: oneshot::Receiver<TransferStop>,
    queue: Arc<Mutex<TransferQueueState>>,
    handle: Handle,
) -> Option<QueuedTransfer> {
    let attempts = match peer_sources_for_request(&settings, &request) {
        Ok(attempts) => attempts,
        Err(error) => {
            report_activity_setup_failure(&*observer, &mut activity, error);
            store_active_activity(&queue, &activity);
            push_activity_history(&queue, &activity);
            return None;
        }
    };
    let modes = attempts
        .iter()
        .map(|attempt| attempt.mode)
        .collect::<Vec<_>>();
    observer.on_status(request_debug_summary(&settings, &request, &attempts));
    let mut stop_requested = None;
    let attempt_count = modes.len();
    for (index, attempt) in attempts.into_iter().enumerate() {
        if index > 0 {
            activity.attempt_id = next_attempt_id();
            activity.updated_at_ms = now_ms();
            store_active_activity(&queue, &activity);
            observer.on_transfer_activity(activity.clone());
        }
        observer.on_status(attempt_debug_summary(index, attempt_count, &attempt));
        let transfer = match build_transfer_for_source(
            &settings,
            &request,
            attempt.source,
            attempt.path_policy_override,
            &handle,
        ) {
            Ok(transfer) => transfer,
            Err(error) => {
                if let Some(next_mode) = modes.get(index + 1) {
                    observer.on_status(format!(
                        "{} setup failed ({}); trying {}",
                        transfer_mode_label(attempt.mode),
                        error,
                        transfer_mode_label(*next_mode)
                    ));
                    continue;
                }
                report_activity_setup_failure(&*observer, &mut activity, error);
                store_active_activity(&queue, &activity);
                push_activity_history(&queue, &activity);
                return None;
            }
        };

        let has_fallback = modes.get(index + 1).is_some();
        let fallback_timeout = fallback_timeout_for_attempt(&request, attempt.mode, has_fallback);
        if let Some(timeout) = fallback_timeout {
            observer.on_status(format!(
                "fallback timeout armed: {}s before connection",
                timeout.as_secs()
            ));
        }
        let outcome = drive_transfer_attempt(
            transfer,
            &mut activity,
            &*observer,
            &mut control,
            &mut stop_requested,
            &queue,
            fallback_timeout,
        )
        .await;
        let can_fallback = can_fallback_after_error(
            outcome.stop_requested,
            outcome.transfer_started,
            has_fallback,
        );
        match outcome.result {
            Ok(summary) => {
                report_terminal(
                    &*observer,
                    &mut activity,
                    &request,
                    Ok(summary),
                    outcome.direction,
                    false,
                );
                store_active_activity(&queue, &activity);
                push_activity_history(&queue, &activity);
                return None;
            }
            Err(_) if outcome.stop_requested == Some(TransferStop::Pause) => {
                activity.apply_paused(now_ms());
                store_active_activity(&queue, &activity);
                return Some(QueuedTransfer {
                    request,
                    observer,
                    activity,
                });
            }
            Err(error) if outcome.stop_requested.is_none() && error.kind == ErrorKind::Paused => {
                activity.apply_peer_paused(now_ms());
                store_active_activity(&queue, &activity);
                observer.on_transfer_activity(activity.clone());
                observer.on_status("peer paused; waiting for resume".to_string());
                schedule_peer_pause_resume(&queue, &activity.activity_id);
                return Some(QueuedTransfer {
                    request,
                    observer,
                    activity,
                });
            }
            Err(error) if can_fallback => {
                let next_mode = modes[index + 1];
                observer.on_status(format!(
                    "{} failed before transfer started ({}); trying {}",
                    transfer_mode_label(attempt.mode),
                    error,
                    transfer_mode_label(next_mode)
                ));
                continue;
            }
            Err(error) => {
                let canceled = outcome.stop_requested == Some(TransferStop::Cancel);
                if canceled {
                    cleanup_canceled_receive(&request, &activity).await;
                }
                report_terminal(
                    &*observer,
                    &mut activity,
                    &request,
                    Err(error),
                    outcome.direction,
                    canceled,
                );
                store_active_activity(&queue, &activity);
                push_activity_history(&queue, &activity);
                return None;
            }
        }
    }
    None
}

/// A peer pause is resumable intent, not a terminal failure. Park a fresh
/// attempt in the queue so the peer that initiated the pause can resume alone.
/// A concurrent local pause/cancel always wins over this automatic action.
fn schedule_peer_pause_resume(queue: &Arc<Mutex<TransferQueueState>>, activity_id: &str) {
    let mut queue = queue.lock().unwrap();
    if let Some(active) = queue.active.get_mut(activity_id) {
        active.control.take();
    }
    queue
        .pending_pause_actions
        .entry(activity_id.to_string())
        .or_insert(PendingPauseAction::Resume);
}

async fn drive_transfer_attempt(
    mut transfer: Transfer,
    activity: &mut FfiTransferActivityRecord,
    observer: &dyn TransferObserver,
    control: &mut oneshot::Receiver<TransferStop>,
    stop_requested: &mut Option<TransferStop>,
    queue: &Arc<Mutex<TransferQueueState>>,
    fallback_timeout: Option<Duration>,
) -> TransferAttemptOutcome {
    let mut direction = None;
    let mut transfer_started = false;
    let mut fallback_elapsed = false;
    let mut last_native_progress_ms = 0;
    let fallback_signal = fallback_watchdog(fallback_timeout);
    tokio::pin!(fallback_signal);
    loop {
        tokio::select! {
            _ = &mut fallback_signal, if fallback_timeout.is_some() && !transfer_started && stop_requested.is_none() => {
                observer.on_status("room send timed out before transfer started; trying fallback".to_string());
                transfer.cancel();
                fallback_elapsed = true;
                break;
            }
            event = transfer.next_event() => {
                let Some(event) = event else { break };
                if direction.is_none() {
                    direction = event_direction(&event.event);
                }
                if matches!(event.event, TransferEvent::Started { .. }) {
                    transfer_started = true;
                }
                if observe_transfer_event(observer, activity, event, &mut last_native_progress_ms) {
                    store_active_activity(queue, activity);
                }
            }
            stop = &mut *control, if stop_requested.is_none() => {
                let stop = stop.unwrap_or(TransferStop::Cancel);
                *stop_requested = Some(stop);
                match stop {
                    TransferStop::Cancel => transfer.cancel(),
                    TransferStop::Pause => transfer.pause(),
                }
                observer.on_status(match stop {
                    TransferStop::Cancel => "cancelling".to_string(),
                    TransferStop::Pause => "pausing".to_string(),
                });
            }
        }
    }
    let result = if fallback_elapsed {
        Err(TransferError::transport(
            Phase::Pairing,
            "rendezvous pairing timed out",
        ))
    } else {
        transfer.wait().await
    };
    TransferAttemptOutcome {
        result,
        direction,
        transfer_started,
        stop_requested: *stop_requested,
    }
}

fn can_fallback_after_error(
    stop_requested: Option<TransferStop>,
    transfer_started: bool,
    has_fallback: bool,
) -> bool {
    stop_requested.is_none() && !transfer_started && has_fallback
}

async fn fallback_watchdog(timeout: Option<Duration>) {
    let Some(timeout) = timeout else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::time::sleep(timeout).await;
}

fn fallback_timeout_for_attempt(
    request: &FfiTransferRequest,
    mode: FfiTransferMode,
    has_fallback: bool,
) -> Option<Duration> {
    if has_fallback
        && request.direction == FfiTransferDirection::Send
        && mode == FfiTransferMode::Room
    {
        Some(ROOM_SEND_FALLBACK_TIMEOUT)
    } else {
        None
    }
}

fn event_direction(event: &TransferEvent) -> Option<TransferDirection> {
    match event {
        TransferEvent::Diagnostic { .. } => None,
        TransferEvent::Binding { direction, .. }
        | TransferEvent::Started { direction, .. }
        | TransferEvent::Verifying { direction, .. }
        | TransferEvent::Verified { direction, .. }
        | TransferEvent::Failed { direction, .. } => Some(*direction),
        _ => None,
    }
}

fn observe_transfer_event(
    observer: &dyn TransferObserver,
    activity: &mut FfiTransferActivityRecord,
    event: StampedEvent,
    last_native_progress_ms: &mut u64,
) -> bool {
    let ffi_event = to_ffi_event(&event, &activity.activity_id);
    activity.apply_event(&ffi_event);
    if !should_emit_native_event(&ffi_event, last_native_progress_ms) {
        return false;
    }
    observer.on_transfer_event(ffi_event);
    observer.on_transfer_activity(activity.clone());
    emit_transfer_event_callbacks(observer, event.event);
    true
}

fn emit_transfer_event_callbacks(observer: &dyn TransferObserver, event: TransferEvent) {
    match event {
        TransferEvent::Diagnostic { message } => {
            observer.on_status(message);
        }
        TransferEvent::Binding { direction, mode } => {
            observer.on_status(format!("binding {direction:?} via {mode:?}"));
        }
        TransferEvent::Advertised { peer, invite, .. } => {
            observer.on_status(advertised_endpoint_debug(&peer, invite.as_deref()));
            if let Some(invite) = invite {
                observer.on_invite_ready(invite);
                observer.on_status("invite ready; waiting for sender".to_string());
            } else {
                observer.on_status("waiting for peer".to_string());
            }
        }
        TransferEvent::Pairing { step } => {
            observer.on_status(format!("pairing: {}", pairing_step_label(step)));
        }
        TransferEvent::Connecting => observer.on_status("connecting".to_string()),
        TransferEvent::Connected { path } => {
            observer.on_status(format!("connected via {path}"));
        }
        TransferEvent::PathChanged { path } => {
            observer.on_status(format!("path changed: {path}"));
        }
        TransferEvent::Started {
            file_name,
            total_bytes,
            ..
        } => observer.on_started(file_name, total_bytes),
        TransferEvent::Progress {
            bytes_transferred,
            total_bytes,
            ..
        } => observer.on_progress(bytes_transferred, total_bytes),
        TransferEvent::Verifying { .. } => observer.on_status("verifying".to_string()),
        TransferEvent::Verified { .. } => observer.on_status("verified".to_string()),
        TransferEvent::Confirming { .. } => observer.on_status("confirming".to_string()),
        TransferEvent::Completed { .. } | TransferEvent::Failed { .. } => {}
        _ => {}
    }
}

pub(crate) fn should_emit_native_event(
    event: &FfiTransferEvent,
    last_progress_ms: &mut u64,
) -> bool {
    if event.kind != FfiTransferEventKind::Progress {
        return true;
    }
    let is_final_progress = event.total_bytes > 0 && event.bytes_transferred >= event.total_bytes;
    let interval_elapsed = *last_progress_ms == 0
        || event.ts_ms.saturating_sub(*last_progress_ms) >= NATIVE_PROGRESS_INTERVAL_MS;
    if interval_elapsed || is_final_progress {
        *last_progress_ms = event.ts_ms;
        return true;
    }
    false
}

pub(crate) fn to_ffi_event(event: &StampedEvent, activity_id: &str) -> FfiTransferEvent {
    let mut ffi = FfiTransferEvent::empty(activity_id, event.ts_ms);
    match &event.event {
        TransferEvent::Diagnostic { message } => {
            ffi.kind = FfiTransferEventKind::Unknown;
            ffi.diagnostic_message = message.clone();
        }
        TransferEvent::Binding { direction, mode } => {
            ffi.kind = FfiTransferEventKind::Binding;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.mode = ffi_transfer_mode(*mode);
        }
        TransferEvent::Advertised {
            peer,
            token,
            invite,
        } => {
            ffi.kind = FfiTransferEventKind::Advertised;
            ffi.peer_descriptor = peer.to_string();
            ffi.token = token.clone().unwrap_or_default();
            ffi.invite = invite.clone().unwrap_or_default();
        }
        TransferEvent::Pairing { step } => {
            ffi.kind = FfiTransferEventKind::Pairing;
            ffi.pairing_step = ffi_pairing_step(*step);
        }
        TransferEvent::Connecting => {
            ffi.kind = FfiTransferEventKind::Connecting;
        }
        TransferEvent::Connected { path } => {
            ffi.kind = FfiTransferEventKind::Connected;
            let (kind, detail) = ffi_data_path(path);
            ffi.data_path_kind = kind;
            ffi.data_path_detail = detail;
        }
        TransferEvent::PathChanged { path } => {
            ffi.kind = FfiTransferEventKind::PathChanged;
            let (kind, detail) = ffi_data_path(path);
            ffi.data_path_kind = kind;
            ffi.data_path_detail = detail;
        }
        TransferEvent::Started {
            transfer_id,
            direction,
            file_name,
            total_bytes,
            bytes_resumed,
        } => {
            ffi.kind = FfiTransferEventKind::Started;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.transfer_id = transfer_id.to_string();
            ffi.file_name = file_name.clone();
            ffi.total_bytes = *total_bytes;
            ffi.bytes_resumed = *bytes_resumed;
        }
        TransferEvent::Progress {
            transfer_id,
            bytes_transferred,
            total_bytes,
        } => {
            ffi.kind = FfiTransferEventKind::Progress;
            ffi.transfer_id = transfer_id.to_string();
            ffi.bytes_transferred = *bytes_transferred;
            ffi.total_bytes = *total_bytes;
        }
        TransferEvent::Verifying {
            transfer_id,
            direction,
            file_name,
            bytes_to_hash,
        } => {
            ffi.kind = FfiTransferEventKind::Verifying;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.transfer_id = transfer_id.to_string();
            ffi.file_name = file_name.clone();
            ffi.total_bytes = *bytes_to_hash;
        }
        TransferEvent::Verified {
            transfer_id,
            direction,
            file_name,
            bytes_hashed,
        } => {
            ffi.kind = FfiTransferEventKind::Verified;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.transfer_id = transfer_id.to_string();
            ffi.file_name = file_name.clone();
            ffi.bytes_transferred = *bytes_hashed;
            ffi.total_bytes = *bytes_hashed;
            if *direction == TransferDirection::Receive {
                ffi.bytes_resumed = *bytes_hashed;
            }
        }
        TransferEvent::Confirming { transfer_id, .. } => {
            ffi.kind = FfiTransferEventKind::Verifying;
            ffi.transfer_id = transfer_id.to_string();
            ffi.diagnostic_message = "confirming".to_string();
        }
        TransferEvent::Completed {
            transfer_id,
            file_name,
            bytes_transferred,
        } => {
            ffi.kind = FfiTransferEventKind::Completed;
            ffi.transfer_id = transfer_id.to_string();
            ffi.file_name = file_name.clone();
            ffi.bytes_transferred = *bytes_transferred;
            ffi.total_bytes = *bytes_transferred;
        }
        TransferEvent::ManifestPreparingEntry {
            manifest_id,
            entry_id,
            relative_path,
            size,
        } => {
            ffi.transfer_id = manifest_id.to_string();
            ffi.file_name = relative_path.clone();
            ffi.total_bytes = *size;
            ffi.diagnostic_message = format!("manifest source check entry_id={entry_id}");
        }
        TransferEvent::ManifestPlanned {
            direction,
            manifest,
        } => {
            ffi.direction = ffi_direction(Some(*direction));
            ffi.transfer_id = manifest.manifest_id.to_string();
            ffi.total_bytes = manifest.total_bytes;
            ffi.diagnostic_message = format!(
                "manifest planned files={} directories={} roots={}",
                manifest.file_count, manifest.directory_count, manifest.root_count
            );
        }
        TransferEvent::ManifestStarted {
            manifest_id,
            direction,
            file_count,
            directory_count,
            total_bytes,
        } => {
            ffi.kind = FfiTransferEventKind::Started;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.transfer_id = manifest_id.to_string();
            ffi.file_name = manifest_id.to_string();
            ffi.total_bytes = *total_bytes;
            ffi.diagnostic_message =
                format!("manifest files={file_count} directories={directory_count}");
        }
        TransferEvent::ManifestEntryStarted {
            manifest_id,
            entry_id,
            transfer_id,
            relative_path,
            total_bytes,
            bytes_resumed,
        } => {
            ffi.kind = FfiTransferEventKind::Started;
            ffi.transfer_id = transfer_id.to_string();
            ffi.file_name = relative_path.clone();
            ffi.total_bytes = *total_bytes;
            ffi.bytes_resumed = *bytes_resumed;
            ffi.diagnostic_message = format!("manifest_id={manifest_id} entry_id={entry_id}");
        }
        TransferEvent::ManifestProgress {
            manifest_id,
            entry_id,
            entry_bytes,
            entry_total_bytes,
            completed_bytes,
            total_bytes,
        } => {
            ffi.kind = FfiTransferEventKind::Progress;
            ffi.transfer_id = manifest_id.to_string();
            ffi.bytes_transferred = *completed_bytes;
            ffi.total_bytes = *total_bytes;
            ffi.diagnostic_message =
                format!("entry_id={entry_id} entry_bytes={entry_bytes}/{entry_total_bytes}");
        }
        TransferEvent::ManifestEntryCompleted {
            manifest_id,
            result,
        } => {
            ffi.kind = if matches!(
                result.status,
                envoix_client::api::ManifestEntryResultStatus::Failed
                    | envoix_client::api::ManifestEntryResultStatus::Cancelled
            ) {
                FfiTransferEventKind::Failed
            } else {
                FfiTransferEventKind::Completed
            };
            ffi.transfer_id = manifest_id.to_string();
            ffi.file_name = result.offered_relative_path.clone();
            ffi.diagnostic_message = format!(
                "entry_id={} status={} final={} failure={}",
                result.entry_id,
                manifest_result_status_label(result.status),
                result.final_relative_path.as_deref().unwrap_or(""),
                result.failure_code.as_deref().unwrap_or("")
            );
        }
        TransferEvent::ManifestCompleted {
            manifest_id,
            file_count,
            directory_count,
            total_bytes,
            ..
        } => {
            ffi.kind = FfiTransferEventKind::Completed;
            ffi.transfer_id = manifest_id.to_string();
            ffi.file_name = manifest_id.to_string();
            ffi.bytes_transferred = *total_bytes;
            ffi.total_bytes = *total_bytes;
            ffi.diagnostic_message =
                format!("manifest files={file_count} directories={directory_count}");
        }
        TransferEvent::Failed {
            direction, reason, ..
        } => {
            ffi.kind = FfiTransferEventKind::Failed;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.diagnostic_message = reason.clone();
        }
        _ => {}
    }
    ffi
}

fn manifest_result_status_label(
    status: envoix_client::api::ManifestEntryResultStatus,
) -> &'static str {
    match status {
        envoix_client::api::ManifestEntryResultStatus::Completed => "completed",
        envoix_client::api::ManifestEntryResultStatus::SkippedIdentical => "skipped_identical",
        envoix_client::api::ManifestEntryResultStatus::Renamed => "renamed",
        envoix_client::api::ManifestEntryResultStatus::Failed => "failed",
        envoix_client::api::ManifestEntryResultStatus::Cancelled => "cancelled",
    }
}

impl FfiTransferEvent {
    fn empty(activity_id: &str, ts_ms: u64) -> Self {
        Self {
            activity_id: activity_id.to_string(),
            kind: FfiTransferEventKind::Unknown,
            ts_ms,
            direction: FfiTransferDirection::Unknown,
            mode: FfiTransferMode::Unknown,
            transfer_id: String::new(),
            file_name: String::new(),
            total_bytes: 0,
            bytes_transferred: 0,
            bytes_resumed: 0,
            pairing_step: FfiPairingStep::None,
            data_path_kind: FfiDataPathKind::None,
            data_path_detail: String::new(),
            invite: String::new(),
            token: String::new(),
            peer_descriptor: String::new(),
            diagnostic_message: String::new(),
        }
    }
}

pub(crate) fn pairing_step_label(step: PairingStep) -> &'static str {
    match step {
        PairingStep::Joining => "joining room",
        PairingStep::Matched => "peer matched",
        PairingStep::Exchanged => "keys exchanged",
    }
}

fn transfer_mode_label(mode: FfiTransferMode) -> &'static str {
    match mode {
        FfiTransferMode::Manual => "manual",
        FfiTransferMode::Invite => "invite",
        FfiTransferMode::ShowManual => "show-manual",
        FfiTransferMode::ShowInvite => "show-invite",
        FfiTransferMode::Mdns => "mDNS",
        FfiTransferMode::Room => "room",
        FfiTransferMode::Unknown => "unknown",
    }
}

fn to_ffi_failure(
    error: &TransferError,
    direction: Option<TransferDirection>,
    activity: &FfiTransferActivityRecord,
) -> FfiTransferFailure {
    let failure = error.to_failure(direction);
    FfiTransferFailure {
        code: ffi_failure_code(failure.code),
        category: ffi_failure_category(failure.category),
        phase: ffi_failure_phase(failure.phase),
        origin: ffi_failure_origin(failure.origin),
        direction: ffi_direction(failure.direction),
        transfer_id: failure
            .transfer_id
            .unwrap_or_else(|| activity.transfer_id.clone()),
        attempt_id: failure
            .attempt_id
            .unwrap_or_else(|| activity.attempt_id.clone()),
        retryable: failure.retryable,
        recovery_action: ffi_recovery_action(failure.recovery_action),
        user_message_key: failure.user_message_key,
        diagnostic_message: failure.diagnostic_message,
    }
}

pub(crate) fn ffi_direction(direction: Option<TransferDirection>) -> FfiTransferDirection {
    match direction {
        Some(TransferDirection::Send) => FfiTransferDirection::Send,
        Some(TransferDirection::Receive) => FfiTransferDirection::Receive,
        None => FfiTransferDirection::Unknown,
    }
}

fn ffi_transfer_mode(mode: TransferMode) -> FfiTransferMode {
    match mode {
        TransferMode::Manual => FfiTransferMode::Manual,
        TransferMode::Invite => FfiTransferMode::Invite,
        TransferMode::ShowManual => FfiTransferMode::ShowManual,
        TransferMode::ShowInvite => FfiTransferMode::ShowInvite,
        TransferMode::Mdns => FfiTransferMode::Mdns,
        TransferMode::Room => FfiTransferMode::Room,
    }
}

fn ffi_pairing_step(step: PairingStep) -> FfiPairingStep {
    match step {
        PairingStep::Joining => FfiPairingStep::Joining,
        PairingStep::Matched => FfiPairingStep::Matched,
        PairingStep::Exchanged => FfiPairingStep::Exchanged,
    }
}

fn ffi_data_path(path: &DataPath) -> (FfiDataPathKind, String) {
    match path {
        DataPath::Direct { addr } => (FfiDataPathKind::Direct, addr.to_string()),
        DataPath::Relay { url } => (FfiDataPathKind::Relay, url.clone()),
        DataPath::Other { description } => (FfiDataPathKind::Other, description.clone()),
    }
}

fn ffi_failure_code(code: FailureCode) -> FfiFailureCode {
    match code {
        FailureCode::UserCanceled => FfiFailureCode::UserCanceled,
        FailureCode::PeerCanceled => FfiFailureCode::PeerCanceled,
        FailureCode::NetworkLost => FfiFailureCode::NetworkLost,
        FailureCode::PeerUnreachable => FfiFailureCode::PeerUnreachable,
        FailureCode::AuthenticationFailed => FfiFailureCode::AuthenticationFailed,
        FailureCode::PermissionDenied => FfiFailureCode::PermissionDenied,
        FailureCode::DiskFull => FfiFailureCode::DiskFull,
        FailureCode::HashMismatch => FfiFailureCode::HashMismatch,
        FailureCode::ProtocolError => FfiFailureCode::ProtocolError,
        FailureCode::DestinationConflict => FfiFailureCode::DestinationConflict,
        FailureCode::UnsupportedFeature => FfiFailureCode::UnsupportedFeature,
        FailureCode::Timeout => FfiFailureCode::Timeout,
        FailureCode::InternalError => FfiFailureCode::InternalError,
        FailureCode::Unknown => FfiFailureCode::Unknown,
    }
}

fn ffi_failure_category(category: FailureCategory) -> FfiFailureCategory {
    match category {
        FailureCategory::User => FfiFailureCategory::User,
        FailureCategory::Network => FfiFailureCategory::Network,
        FailureCategory::Authentication => FfiFailureCategory::Authentication,
        FailureCategory::Permission => FfiFailureCategory::Permission,
        FailureCategory::Storage => FfiFailureCategory::Storage,
        FailureCategory::Integrity => FfiFailureCategory::Integrity,
        FailureCategory::Protocol => FfiFailureCategory::Protocol,
        FailureCategory::Unsupported => FfiFailureCategory::Unsupported,
        FailureCategory::Internal => FfiFailureCategory::Internal,
        FailureCategory::Unknown => FfiFailureCategory::Unknown,
    }
}

fn ffi_failure_origin(origin: FailureOrigin) -> FfiFailureOrigin {
    match origin {
        FailureOrigin::Local => FfiFailureOrigin::Local,
        FailureOrigin::Peer => FfiFailureOrigin::Peer,
        FailureOrigin::Unknown => FfiFailureOrigin::Unknown,
    }
}

fn ffi_failure_phase(phase: FailurePhase) -> FfiFailurePhase {
    match phase {
        FailurePhase::Setup => FfiFailurePhase::Setup,
        FailurePhase::Binding => FfiFailurePhase::Binding,
        FailurePhase::Advertising => FfiFailurePhase::Advertising,
        FailurePhase::Pairing => FfiFailurePhase::Pairing,
        FailurePhase::Connecting => FfiFailurePhase::Connecting,
        FailurePhase::Authenticating => FfiFailurePhase::Authenticating,
        FailurePhase::Negotiating => FfiFailurePhase::Negotiating,
        FailurePhase::Transferring => FfiFailurePhase::Transferring,
        FailurePhase::Verifying => FfiFailurePhase::Verifying,
        FailurePhase::Committing => FfiFailurePhase::Committing,
        FailurePhase::Acknowledging => FfiFailurePhase::Acknowledging,
        FailurePhase::CleaningUp => FfiFailurePhase::CleaningUp,
    }
}

fn ffi_recovery_action(action: RecoveryAction) -> FfiRecoveryAction {
    match action {
        RecoveryAction::Retry => FfiRecoveryAction::Retry,
        RecoveryAction::Resume => FfiRecoveryAction::Resume,
        RecoveryAction::ChooseFolder => FfiRecoveryAction::ChooseFolder,
        RecoveryAction::OpenSettings => FfiRecoveryAction::OpenSettings,
        RecoveryAction::RePair => FfiRecoveryAction::RePair,
        RecoveryAction::UpdateApp => FfiRecoveryAction::UpdateApp,
        RecoveryAction::SwitchPairingMethod => FfiRecoveryAction::SwitchPairingMethod,
        RecoveryAction::DiscardPartial => FfiRecoveryAction::DiscardPartial,
        RecoveryAction::None => FfiRecoveryAction::None,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
