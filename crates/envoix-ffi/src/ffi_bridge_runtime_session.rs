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
