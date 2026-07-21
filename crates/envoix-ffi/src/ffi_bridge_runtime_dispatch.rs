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

