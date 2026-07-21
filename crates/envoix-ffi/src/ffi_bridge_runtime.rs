#[derive(uniffi::Object)]
pub struct EnvoixSession {
    runtime: Runtime,
    queue: Arc<Mutex<TransferQueueState>>,
    settings: EnvoixRuntimeSettings,
}

#[derive(Default)]
struct TransferQueueState {
    pending: VecDeque<QueuedTransfer>,
    active: HashMap<String, ActiveTransfer>,
    paused: HashMap<String, QueuedTransfer>,
    pending_pause_actions: HashMap<String, PendingPauseAction>,
    history: VecDeque<FfiTransferActivityRecord>,
    requests: HashMap<String, FfiTransferRequest>,
    discarded: HashSet<String>,
}

struct QueuedTransfer {
    request: FfiTransferRequest,
    observer: Arc<dyn TransferObserver>,
    activity: FfiTransferActivityRecord,
}

#[derive(Debug)]
struct TransferAttemptSource {
    mode: FfiTransferMode,
    source: PeerSource,
    path_policy_override: Option<FfiPathPolicy>,
}

struct TransferAttemptOutcome {
    result: Result<TransferSummary, TransferError>,
    direction: Option<TransferDirection>,
    transfer_started: bool,
    stop_requested: Option<TransferStop>,
}

struct ActiveTransfer {
    control: Option<oneshot::Sender<TransferStop>>,
    limit: usize,
    activity: FfiTransferActivityRecord,
}

struct FinishedActivityNotice {
    observer: Arc<dyn TransferObserver>,
    activity: FfiTransferActivityRecord,
    status: &'static str,
    cleanup_request: Option<FfiTransferRequest>,
}

fn is_finalizing_activity(activity: &FfiTransferActivityRecord) -> bool {
    activity.state == FfiTransferActivityState::Verifying
        && activity.diagnostic_message == "confirming"
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferStop {
    Cancel,
    Pause,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPauseAction {
    Pause,
    Resume,
    Cancel,
}

impl TransferAttemptSource {
    fn new(mode: FfiTransferMode, source: PeerSource) -> Self {
        Self {
            mode,
            source,
            path_policy_override: None,
        }
    }

    fn with_path_policy(mut self, path_policy: FfiPathPolicy) -> Self {
        self.path_policy_override = Some(path_policy);
        self
    }
}

impl TransferQueueState {
    fn contains_activity(&self, activity_id: &str) -> bool {
        self.active.contains_key(activity_id)
            || self
                .pending
                .iter()
                .any(|job| job.activity.activity_id == activity_id)
            || self.paused.contains_key(activity_id)
    }

    fn activities(&self) -> Vec<FfiTransferActivityRecord> {
        let mut seen = HashMap::new();
        let mut records = Vec::new();
        for record in self.history.iter() {
            if self.discarded.contains(&record.activity_id) {
                continue;
            }
            seen.insert(record.activity_id.clone(), ());
            records.push(record.clone());
        }
        for record in self.active.values().map(|active| &active.activity) {
            if !self.discarded.contains(&record.activity_id)
                && !seen.contains_key(&record.activity_id)
            {
                seen.insert(record.activity_id.clone(), ());
                records.push(record.clone());
            }
        }
        for job in self.pending.iter() {
            if !self.discarded.contains(&job.activity.activity_id)
                && !seen.contains_key(&job.activity.activity_id)
            {
                seen.insert(job.activity.activity_id.clone(), ());
                records.push(job.activity.clone());
            }
        }
        for job in self.paused.values() {
            if !self.discarded.contains(&job.activity.activity_id)
                && !seen.contains_key(&job.activity.activity_id)
            {
                seen.insert(job.activity.activity_id.clone(), ());
                records.push(job.activity.clone());
            }
        }
        records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        records
    }

    fn activity(&self, activity_id: &str) -> Option<FfiTransferActivityRecord> {
        self.activities()
            .into_iter()
            .find(|record| record.activity_id == activity_id)
    }

    fn push_history(&mut self, activity: FfiTransferActivityRecord) {
        if self.discarded.contains(&activity.activity_id) {
            return;
        }
        self.history
            .retain(|record| record.activity_id != activity.activity_id);
        self.history.push_front(activity);
        if self.history.len() > TRANSFER_ACTIVITY_HISTORY_CAP {
            self.history.truncate(TRANSFER_ACTIVITY_HISTORY_CAP);
        }
        let retained_ids = self
            .history
            .iter()
            .map(|record| record.activity_id.clone())
            .chain(self.active.keys().cloned())
            .chain(
                self.pending
                    .iter()
                    .map(|job| job.activity.activity_id.clone()),
            )
            .chain(self.paused.keys().cloned())
            .collect::<HashSet<_>>();
        self.requests
            .retain(|activity_id, _| retained_ids.contains(activity_id));
    }

    fn can_start(&self, next_limit: usize) -> bool {
        let limit = self
            .active
            .values()
            .map(|active| active.limit)
            .chain(std::iter::once(next_limit))
            .min()
            .unwrap_or(1)
            .max(1);
        self.active.len() < limit
    }
}

#[uniffi::export]
impl EnvoixSession {
    /// Creates a session with its own multi-threaded runtime.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::new_with_settings(EnvoixRuntimeSettings::default())
    }

    /// Creates a session with explicit runtime settings.
    #[uniffi::constructor]
    pub fn new_with_settings(settings: EnvoixRuntimeSettings) -> Arc<Self> {
        #[cfg(not(target_os = "android"))]
        init_env_logging();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        Arc::new(Self {
            runtime,
            queue: Arc::new(Mutex::new(TransferQueueState::default())),
            settings,
        })
    }

    /// Returns the current in-memory transfer queue and recent terminal records.
    pub fn list_transfer_activities(&self) -> Vec<FfiTransferActivityRecord> {
        self.queue.lock().unwrap().activities()
    }

    /// Returns one visible transfer activity by id.
    pub fn get_transfer_activity(&self, activity_id: String) -> Option<FfiTransferActivityRecord> {
        let activity_id = activity_id.trim();
        if activity_id.is_empty() {
            return None;
        }
        self.queue.lock().unwrap().activity(activity_id)
    }

    /// Removes a transfer activity from the shared queue/history.
    ///
    /// Pending or paused items are canceled before removal. Active items receive
    /// a cancel signal and are hidden immediately; their terminal callbacks may
    /// still arrive at the observer, but the shared queue will not list them.
    pub fn discard_transfer_activity(&self, activity_id: String) -> bool {
        let activity_id = activity_id.trim().to_string();
        if activity_id.is_empty() {
            return false;
        }
        let (stored, control, cancel_after_pause, removed_history, cleanup) = {
            let mut queue = self.queue.lock().unwrap();
            let record = queue.activity(&activity_id);
            let was_active = queue.active.contains_key(&activity_id);
            queue.discarded.insert(activity_id.clone());
            let pending = queue
                .pending
                .iter()
                .position(|job| job.activity.activity_id == activity_id)
                .and_then(|index| queue.pending.remove(index));
            let paused = queue.paused.remove(&activity_id);
            let control = queue
                .active
                .get_mut(&activity_id)
                .and_then(|active| active.control.take());
            let cancel_after_pause = control.is_none()
                && queue.active.contains_key(&activity_id)
                && queue.pending_pause_actions.contains_key(&activity_id);
            if control.is_some() || cancel_after_pause {
                queue
                    .pending_pause_actions
                    .insert(activity_id.clone(), PendingPauseAction::Cancel);
            }
            let before = queue.history.len();
            queue
                .history
                .retain(|record| record.activity_id != activity_id);
            let request = queue.requests.remove(&activity_id);
            (
                pending.or(paused),
                control,
                cancel_after_pause,
                queue.history.len() != before,
                (!was_active).then_some(request.zip(record)).flatten(),
            )
        };
        let removed_stored = stored.is_some();
        let removed_active = control.is_some();
        if let Some(job) = stored {
            report_queued_cancel(job);
        }
        if let Some(control) = control {
            let _ = control.send(TransferStop::Cancel);
        }
        if let Some((request, record)) = cleanup {
            self.runtime
                .block_on(cleanup_canceled_receive(&request, &record));
        }
        removed_stored || removed_active || cancel_after_pause || removed_history
    }

    /// Clears terminal transfer history while preserving active, pending, and paused items.
    pub fn clear_transfer_history(&self) -> u32 {
        let mut queue = self.queue.lock().unwrap();
        let removed = queue.history.len();
        let records = queue.history.drain(..).collect::<Vec<_>>();
        let cleanup = records
            .into_iter()
            .filter_map(|record| {
                let request = queue.requests.remove(&record.activity_id)?;
                Some((request, record))
            })
            .collect::<Vec<_>>();
        drop(queue);
        for (request, record) in cleanup {
            self.runtime
                .block_on(cleanup_canceled_receive(&request, &record));
        }
        removed.min(u32::MAX as usize) as u32
    }

    /// Starts receiving one file into `output_dir`.
    ///
    /// Returns immediately. A fresh pairing token is generated and the invite
    /// is delivered via [`TransferObserver::on_invite_ready`]; the outcome
    /// arrives via `on_completed` / `on_failed`.
    pub fn receive(
        &self,
        output_dir: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        self.start_transfer(
            FfiTransferRequest::receive(output_dir, FfiTransferMode::ShowInvite),
            observer,
        )
    }

    /// Starts sending `file_path` to the peer encoded in `invite`.
    ///
    /// Returns immediately; the outcome arrives via `on_completed` /
    /// `on_failed`. The invite is validated (expiry, version) before any
    /// connection is attempted.
    pub fn send_invite(
        &self,
        invite: String,
        file_path: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = FfiTransferRequest::send(file_path, FfiTransferMode::Invite);
        request.invite = invite;
        self.start_transfer(request, observer)
    }

    /// Starts receiving one file into `output_dir`, pairing on the local
    /// network with a shared `token` (no invite needed).
    ///
    /// Both peers enter the same token; the receiver advertises over mDNS and
    /// the sender discovers it. Requires both peers on the same LAN. The token
    /// must be at least 12 ASCII bytes.
    pub fn receive_mdns(
        &self,
        output_dir: String,
        token: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = FfiTransferRequest::receive(output_dir, FfiTransferMode::Mdns);
        request.token = token;
        self.start_transfer(request, observer)
    }

    /// Starts sending `file_path`, discovering the receiver on the local
    /// network via a shared `token` (no invite needed).
    ///
    /// Both peers enter the same token; requires both on the same LAN. The
    /// token must be at least 12 ASCII bytes.
    pub fn send_mdns(
        &self,
        file_path: String,
        token: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = FfiTransferRequest::send(file_path, FfiTransferMode::Mdns);
        request.token = token;
        self.start_transfer(request, observer)
    }

    /// Starts receiving one file by pairing in a rendezvous room with `code`.
    pub fn receive_room(
        &self,
        output_dir: String,
        code: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = FfiTransferRequest::receive(output_dir, FfiTransferMode::Room);
        request.code = code;
        self.start_transfer(request, observer)
    }

    /// Starts sending `file_path` by pairing in a rendezvous room with `code`.
    pub fn send_room(
        &self,
        file_path: String,
        code: String,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = FfiTransferRequest::send(file_path, FfiTransferMode::Room);
        request.code = code;
        self.start_transfer(request, observer)
    }

    /// Starts a transfer from one cross-platform request object.
    ///
    /// This is the preferred API for new native clients. The narrower methods
    /// above are kept as compatibility wrappers while Apple/Android migrate.
    pub fn start_transfer(
        &self,
        request: FfiTransferRequest,
        observer: Arc<dyn TransferObserver>,
    ) -> Result<(), EnvoixError> {
        let mut request = request;
        if request.activity_id.trim().is_empty() {
            request.activity_id = next_activity_id();
        }
        normalize_transfer_limits(&self.settings, &mut request.limits);
        validate_transfer_request(&self.settings, &request)?;
        let activity = FfiTransferActivityRecord::from_request(&request, now_ms());
        {
            let mut queue = self.queue.lock().unwrap();
            if queue.contains_activity(&request.activity_id) {
                return Err(EnvoixError::Operation {
                    reason: format!("activity_id already exists: {}", request.activity_id),
                });
            }
            queue
                .requests
                .insert(request.activity_id.clone(), request.clone());
            queue.pending.push_back(QueuedTransfer {
                request,
                observer: observer.clone(),
                activity: activity.clone(),
            });
        }
        observer.on_transfer_activity(activity);
        drain_transfer_queue(
            self.queue.clone(),
            self.settings.clone(),
            self.runtime.handle().clone(),
        );
        Ok(())
    }

    /// Requests cancellation of one queued/running activity.
    pub fn cancel_activity(&self, activity_id: String) -> bool {
        let activity_id = activity_id.trim().to_string();
        if activity_id.is_empty() {
            return false;
        }
        let stored = {
            let mut queue = self.queue.lock().unwrap();
            if let Some(index) = queue
                .pending
                .iter()
                .position(|job| job.activity.activity_id == activity_id)
            {
                queue.pending.remove(index)
            } else {
                queue.paused.remove(&activity_id)
            }
        };
        if let Some(job) = stored {
            let activity = report_queued_cancel(job);
            self.queue.lock().unwrap().push_history(activity);
            return true;
        }

        let (control, cancel_after_pause) = {
            let mut queue = self.queue.lock().unwrap();
            let control = queue.active.get_mut(&activity_id).and_then(|active| {
                (!is_finalizing_activity(&active.activity))
                    .then(|| active.control.take())
                    .flatten()
            });
            let cancel_after_pause = control.is_none()
                && queue.active.contains_key(&activity_id)
                && queue.pending_pause_actions.contains_key(&activity_id);
            if control.is_some() || cancel_after_pause {
                queue
                    .pending_pause_actions
                    .insert(activity_id.clone(), PendingPauseAction::Cancel);
            }
            (control, cancel_after_pause)
        };
        if let Some(control) = control {
            let _ = control.send(TransferStop::Cancel);
            true
        } else {
            cancel_after_pause
        }
    }

    /// Pauses a queued/running activity while keeping its request for resume.
    pub fn pause_activity(&self, activity_id: String) -> bool {
        let activity_id = activity_id.trim().to_string();
        if activity_id.is_empty() {
            return false;
        }
        let pending = {
            let mut queue = self.queue.lock().unwrap();
            if queue.paused.contains_key(&activity_id) {
                return true;
            }
            queue
                .pending
                .iter()
                .position(|job| job.activity.activity_id == activity_id)
                .and_then(|index| queue.pending.remove(index))
        };
        if let Some(mut job) = pending {
            job.activity.apply_paused(now_ms());
            let observer = job.observer.clone();
            let activity = job.activity.clone();
            self.queue.lock().unwrap().paused.insert(activity_id, job);
            observer.on_transfer_activity(activity);
            observer.on_status("paused".to_string());
            return true;
        }

        let control = {
            let mut queue = self.queue.lock().unwrap();
            let control = queue.active.get_mut(&activity_id).and_then(|active| {
                (!is_finalizing_activity(&active.activity))
                    .then(|| active.control.take())
                    .flatten()
            });
            if control.is_some() {
                queue
                    .pending_pause_actions
                    .insert(activity_id.clone(), PendingPauseAction::Pause);
            }
            control
        };
        if let Some(control) = control {
            let _ = control.send(TransferStop::Pause);
            true
        } else {
            false
        }
    }

    /// Requeues a paused activity with its original request and observer.
    pub fn resume_activity(&self, activity_id: String) -> bool {
        let activity_id = activity_id.trim().to_string();
        if activity_id.is_empty() {
            return false;
        }
        let mut job = {
            let mut queue = self.queue.lock().unwrap();
            match queue.paused.remove(&activity_id) {
                Some(job) => job,
                None if queue.active.contains_key(&activity_id)
                    && matches!(
                        queue.pending_pause_actions.get(&activity_id),
                        Some(PendingPauseAction::Pause | PendingPauseAction::Resume)
                    ) =>
                {
                    queue
                        .pending_pause_actions
                        .insert(activity_id, PendingPauseAction::Resume);
                    return true;
                }
                None => return false,
            }
        };
        job.activity.apply_requeued(now_ms());
        let observer = job.observer.clone();
        let activity = job.activity.clone();
        {
            let mut queue = self.queue.lock().unwrap();
            if queue.contains_activity(&activity_id) {
                queue.paused.insert(activity_id, job);
                return false;
            }
            queue.pending.push_back(job);
        }
        observer.on_transfer_activity(activity);
        observer.on_status("resuming".to_string());
        drain_transfer_queue(
            self.queue.clone(),
            self.settings.clone(),
            self.runtime.handle().clone(),
        );
        true
    }

    /// Requests cancellation of all queued/running transfers, if any.
    pub fn cancel(&self) {
        let (stored, controls) = {
            let mut queue = self.queue.lock().unwrap();
            let mut stored = queue.pending.drain(..).collect::<Vec<_>>();
            stored.extend(queue.paused.drain().map(|(_, job)| job));
            let active_controls = queue
                .active
                .iter_mut()
                .filter_map(|(activity_id, active)| {
                    (!is_finalizing_activity(&active.activity))
                        .then(|| active.control.take())
                        .flatten()
                        .map(|control| (activity_id.clone(), control))
                })
                .collect::<Vec<_>>();
            for (activity_id, _) in &active_controls {
                queue
                    .pending_pause_actions
                    .insert(activity_id.clone(), PendingPauseAction::Cancel);
            }
            let controls: Vec<oneshot::Sender<TransferStop>> = active_controls
                .into_iter()
                .map(|(_, control)| control)
                .collect();
            (stored, controls)
        };
        for job in stored {
            let activity = report_queued_cancel(job);
            self.queue.lock().unwrap().push_history(activity);
        }
        for control in controls {
            let _ = control.send(TransferStop::Cancel);
        }
    }
}

fn drain_transfer_queue(
    queue: Arc<Mutex<TransferQueueState>>,
    settings: EnvoixRuntimeSettings,
    handle: Handle,
) {
    loop {
        let (job, control) = {
            let mut queue = queue.lock().unwrap();
            let Some(next) = queue.pending.front() else {
                return;
            };
            let limit = effective_parallel_limit(&settings, &next.request.limits);
            if !queue.can_start(limit) {
                return;
            }

            let mut job = queue.pending.pop_front().expect("pending job exists");
            if job.activity.attempt_id.trim().is_empty() {
                job.activity.attempt_id = next_attempt_id();
                job.activity.updated_at_ms = now_ms();
            }
            let activity_id = job.activity.activity_id.clone();
            let (control_sender, control_receiver) = oneshot::channel();
            queue.active.insert(
                activity_id,
                ActiveTransfer {
                    control: Some(control_sender),
                    limit,
                    activity: job.activity.clone(),
                },
            );
            (job, control_receiver)
        };

        let activity_id = job.activity.activity_id.clone();
        job.observer.on_transfer_activity(job.activity.clone());

        let queue_for_task = queue.clone();
        let settings_for_task = settings.clone();
        let handle_for_task = handle.clone();
        handle.spawn(async move {
            let paused_job = drive_transfer_request(
                settings_for_task.clone(),
                job.request,
                job.activity,
                job.observer,
                control,
                queue_for_task.clone(),
                handle_for_task.clone(),
            )
            .await;
            if let Some(notice) =
                finish_transfer_activity(&activity_id, paused_job, &queue_for_task)
            {
                if let Some(request) = notice.cleanup_request {
                    cleanup_canceled_receive(&request, &notice.activity).await;
                }
                notice.observer.on_transfer_activity(notice.activity);
                notice.observer.on_status(notice.status.to_string());
            }
            drain_transfer_queue(queue_for_task, settings_for_task, handle_for_task);
        });
    }
}

fn finish_transfer_activity(
    activity_id: &str,
    paused_job: Option<QueuedTransfer>,
    queue: &Arc<Mutex<TransferQueueState>>,
) -> Option<FinishedActivityNotice> {
    let mut queue = queue.lock().unwrap();
    queue.active.remove(activity_id);
    let pending_action = queue.pending_pause_actions.remove(activity_id);
    let mut job = paused_job?;
    match pending_action {
        Some(PendingPauseAction::Cancel) => {
            job.activity.apply_canceled(now_ms());
            let observer = job.observer;
            let activity = job.activity;
            let request = job.request;
            queue.push_history(activity.clone());
            Some(FinishedActivityNotice {
                observer,
                activity,
                status: "canceled",
                cleanup_request: Some(request),
            })
        }
        Some(PendingPauseAction::Resume) => {
            job.activity.apply_requeued(now_ms());
            let observer = job.observer.clone();
            let activity = job.activity.clone();
            queue.pending.push_back(job);
            Some(FinishedActivityNotice {
                observer,
                activity,
                status: "resuming",
                cleanup_request: None,
            })
        }
        Some(PendingPauseAction::Pause) | None => {
            let observer = job.observer.clone();
            let activity = job.activity.clone();
            queue.paused.insert(activity_id.to_string(), job);
            Some(FinishedActivityNotice {
                observer,
                activity,
                status: "paused",
                cleanup_request: None,
            })
        }
    }
}

fn store_active_activity(
    queue: &Arc<Mutex<TransferQueueState>>,
    activity: &FfiTransferActivityRecord,
) {
    if let Some(active) = queue.lock().unwrap().active.get_mut(&activity.activity_id) {
        active.activity = activity.clone();
    }
}

fn push_activity_history(
    queue: &Arc<Mutex<TransferQueueState>>,
    activity: &FfiTransferActivityRecord,
) {
    queue.lock().unwrap().push_history(activity.clone());
}

async fn cleanup_canceled_receive(
    request: &FfiTransferRequest,
    activity: &FfiTransferActivityRecord,
) {
    if request.direction != FfiTransferDirection::Receive
        || activity.transfer_id.trim().is_empty()
        || activity.file_name.trim().is_empty()
    {
        return;
    }
    let output_dir = Path::new(request.output_dir.trim());
    let transfer_id = TransferId::new(activity.transfer_id.clone());
    if let Err(error) =
        LocalFileStorage::delete_resume_temp(output_dir, &activity.file_name, &transfer_id).await
    {
        tracing::warn!(%error, activity_id = activity.activity_id, "failed to delete canceled receive partial");
    }
    if let Err(error) =
        LocalFileStorage::delete_resume_state(output_dir, &activity.file_name, &transfer_id).await
    {
        tracing::warn!(%error, activity_id = activity.activity_id, "failed to delete canceled receive state");
    }
}

fn report_queued_cancel(mut job: QueuedTransfer) -> FfiTransferActivityRecord {
    let message = if job.activity.state == FfiTransferActivityState::Paused {
        "canceled paused transfer"
    } else {
        "canceled before start"
    };
    job.activity.apply_canceled(now_ms());
    let activity = job.activity.clone();
    job.observer.on_transfer_activity(activity.clone());
    job.observer.on_status(message.to_string());
    activity
}

fn report_activity_setup_failure(
    observer: &dyn TransferObserver,
    activity: &mut FfiTransferActivityRecord,
    error: EnvoixError,
) {
    let message = error.to_string();
    let failure = setup_failure(message.clone(), activity.direction, activity);
    activity.apply_failure(&failure, now_ms());
    observer.on_transfer_activity(activity.clone());
    observer.on_transfer_failed(failure);
    observer.on_failed(message);
}

fn setup_failure(
    reason: String,
    direction: FfiTransferDirection,
    activity: &FfiTransferActivityRecord,
) -> FfiTransferFailure {
    FfiTransferFailure {
        code: FfiFailureCode::InternalError,
        category: FfiFailureCategory::Internal,
        phase: FfiFailurePhase::Setup,
        origin: FfiFailureOrigin::Local,
        direction,
        transfer_id: String::new(),
        attempt_id: activity.attempt_id.clone(),
        retryable: true,
        recovery_action: FfiRecoveryAction::Retry,
        user_message_key: "transfer.setup_failed".to_string(),
        diagnostic_message: reason,
    }
}

fn validate_transfer_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<(), EnvoixError> {
    validate_direction_mode(request.direction, request.mode)?;
    match request.direction {
        FfiTransferDirection::Send => {
            required_path(&request.file_path, "file_path")?;
        }
        FfiTransferDirection::Receive => {
            required_path(&request.output_dir, "output_dir")?;
        }
        FfiTransferDirection::Unknown => {
            return Err(EnvoixError::Operation {
                reason: "transfer direction must be send or receive".to_string(),
            });
        }
    }
    build_client_for_request(settings, request)?;
    transfer_options_for_request(settings, request, None)?;
    peer_sources_for_request(settings, request)?;
    Ok(())
}

fn validate_direction_mode(
    direction: FfiTransferDirection,
    mode: FfiTransferMode,
) -> Result<(), EnvoixError> {
    match (direction, mode) {
        (
            FfiTransferDirection::Send,
            FfiTransferMode::Manual
            | FfiTransferMode::Invite
            | FfiTransferMode::Mdns
            | FfiTransferMode::Room,
        )
        | (
            FfiTransferDirection::Receive,
            FfiTransferMode::ShowManual
            | FfiTransferMode::ShowInvite
            | FfiTransferMode::Mdns
            | FfiTransferMode::Room,
        ) => Ok(()),
        (FfiTransferDirection::Unknown, _) => Err(EnvoixError::Operation {
            reason: "transfer direction must be send or receive".to_string(),
        }),
        (_, FfiTransferMode::Unknown) => Err(EnvoixError::Operation {
            reason: "transfer mode must not be unknown".to_string(),
        }),
        (direction, mode) => Err(EnvoixError::Operation {
            reason: format!("transfer mode {mode:?} is not supported for {direction:?}"),
        }),
    }
}

fn build_transfer_for_source(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    source: PeerSource,
    path_policy_override: Option<FfiPathPolicy>,
    handle: &Handle,
) -> Result<Transfer, EnvoixError> {
    let client = build_client_for_request(settings, request)?;
    let options = transfer_options_for_request(settings, request, path_policy_override)?;
    let _guard = handle.enter();
    match request.direction {
        FfiTransferDirection::Send => client
            .send(
                required_path(&request.file_path, "file_path")?.into(),
                source,
                options,
            )
            .map_err(op_err),
        FfiTransferDirection::Receive => client
            .receive(
                required_path(&request.output_dir, "output_dir")?.into(),
                source,
                options,
            )
            .map_err(op_err),
        FfiTransferDirection::Unknown => Err(EnvoixError::Operation {
            reason: "transfer direction must be send or receive".to_string(),
        }),
    }
}

fn normalize_transfer_limits(settings: &EnvoixRuntimeSettings, limits: &mut FfiTransferLimits) {
    limits.max_parallel_transfers = effective_parallel_limit(settings, limits) as u32;
}

fn effective_parallel_limit(settings: &EnvoixRuntimeSettings, limits: &FfiTransferLimits) -> usize {
    if !settings.concurrent_transfers {
        return 1;
    }
    limits.max_parallel_transfers.max(1) as usize
}

