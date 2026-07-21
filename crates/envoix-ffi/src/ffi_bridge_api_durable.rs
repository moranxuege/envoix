#[derive(Clone)]
enum NativeMailboxObserver {
    V1(Arc<dyn MailboxObserver>),
    V2(Arc<dyn MailboxObserverV2>),
}

impl NativeMailboxObserver {
    fn fetch(&self, activity_id: String, key: String, server: Option<String>) {
        match self {
            Self::V1(observer) => observer.on_fetch_receipt(activity_id, key),
            Self::V2(observer) => observer.on_fetch_receipt(activity_id, key, server),
        }
    }

    fn post(&self, activity_id: String, key: String, blob: Vec<u8>, server: Option<String>) {
        match self {
            Self::V1(observer) => observer.on_post_receipt(activity_id, key, blob),
            Self::V2(observer) => observer.on_post_receipt(activity_id, key, blob, server),
        }
    }
}

/// One durable transfer card driven by the canonical Rust state machine.
#[derive(uniffi::Object)]
pub struct DurableEnvoixSession {
    driver: Mutex<Option<CanonicalTransferSession>>,
    activity: Arc<Mutex<FfiTransferActivityRecord>>,
    pending_receipt_key: Arc<Mutex<Option<String>>>,
    platform_extras: Mutex<serde_json::Value>,
}

#[uniffi::export]
impl DurableEnvoixSession {
    pub fn pause(&self) -> bool {
        if !can_pause_durable_activity(&self.activity.lock().unwrap()) {
            return false;
        }
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.pause()
    }

    pub fn resume(&self) -> bool {
        if !can_resume_durable_activity(&self.activity.lock().unwrap()) {
            return false;
        }
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.resume()
    }

    pub fn cancel(&self) -> bool {
        if !can_cancel_durable_activity(&self.activity.lock().unwrap()) {
            return false;
        }
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.cancel()
    }

    pub fn receipt_response(&self, blob: Vec<u8>) -> bool {
        let Some(key) = self.pending_receipt_key.lock().unwrap().take() else {
            return false;
        };
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.receipt_response(key, (!blob.is_empty()).then_some(blob))
    }

    pub fn receipt_posted(&self) -> bool {
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.receipt_posted()
    }

    /// Persist or replace the native publication destination without
    /// retransmitting the staged receive. Replacing a target clears the last
    /// publication failure so the same card can be retried in place.
    pub fn set_publication_target(&self, mut target: FfiNativePublicationTarget) -> bool {
        target.destination_path = target.destination_path.trim().to_string();
        let activity = self.activity.lock().unwrap();
        if target.destination_path.is_empty()
            || activity.direction != FfiTransferDirection::Receive
            || matches!(
                activity.state,
                FfiTransferActivityState::Completed
                    | FfiTransferActivityState::Canceled
                    | FfiTransferActivityState::Failed
            )
        {
            return false;
        }
        drop(activity);

        let mut extras = self.platform_extras.lock().unwrap();
        let mut candidate = extras.clone();
        let Some(object) = candidate.as_object_mut() else {
            return false;
        };
        object.insert(
            NATIVE_PUBLICATION_EXTRAS_KEY.to_string(),
            serde_json::to_value(PersistedNativePublication {
                target: Some(target),
                failure: None,
            })
            .expect("native publication metadata must serialize"),
        );
        let driver_guard = self.driver.lock().unwrap();
        let Some(driver) = driver_guard.as_ref() else {
            return false;
        };
        if !driver.set_extras(candidate.clone()) {
            return false;
        }
        drop(driver_guard);
        *extras = candidate;
        drop(extras);
        let mut activity = self.activity.lock().unwrap();
        activity.clear_failure_metadata(now_ms());
        true
    }

    /// Returns the canonical native publication destination after restore.
    pub fn publication_target(&self) -> Option<FfiNativePublicationTarget> {
        native_publication_metadata_from_extras(&self.platform_extras.lock().unwrap())?.target
    }

    /// Persist a platform publication failure while keeping the canonical
    /// transfer in Publishing so it can retry the same staged bytes.
    pub fn publication_failed(&self, failure: FfiTransferFailure) -> bool {
        let activity = self.activity.lock().unwrap();
        if activity.state != FfiTransferActivityState::Publishing
            || !failure.retryable
            || !matches!(
                failure.direction,
                FfiTransferDirection::Receive | FfiTransferDirection::Unknown
            )
        {
            return false;
        }
        drop(activity);

        let mut extras = self.platform_extras.lock().unwrap();
        let mut candidate = extras.clone();
        let Some(object) = candidate.as_object_mut() else {
            return false;
        };
        let mut publication: PersistedNativePublication = object
            .get(NATIVE_PUBLICATION_EXTRAS_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        publication.failure = Some(failure.clone());
        object.insert(
            NATIVE_PUBLICATION_EXTRAS_KEY.to_string(),
            serde_json::to_value(publication).expect("native publication metadata must serialize"),
        );
        let driver_guard = self.driver.lock().unwrap();
        let Some(driver) = driver_guard.as_ref() else {
            return false;
        };
        if !driver.set_extras(candidate.clone()) {
            return false;
        }
        drop(driver_guard);
        *extras = candidate;
        drop(extras);
        self.activity
            .lock()
            .unwrap()
            .apply_publication_failure(&failure, now_ms());
        true
    }

    /// Confirms that a staged receive is now visible in Files/MediaStore.
    pub fn publication_succeeded(&self, path: String) -> bool {
        let path = path.trim();
        if path.is_empty()
            || self.activity.lock().unwrap().state != FfiTransferActivityState::Publishing
        {
            return false;
        }
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.published(path.to_string())
    }

    /// Remove is the one true abandon: discard exact partial/sidecars and the
    /// durable record, then stop this session. Idempotent.
    pub fn remove(&self) -> bool {
        let Some(driver) = self.driver.lock().unwrap().take() else {
            return false;
        };
        driver.discard()
    }

    pub fn activity(&self) -> FfiTransferActivityRecord {
        self.activity.lock().unwrap().clone()
    }
}

fn can_pause_durable_activity(activity: &FfiTransferActivityRecord) -> bool {
    matches!(
        activity.state,
        FfiTransferActivityState::Queued
            | FfiTransferActivityState::Binding
            | FfiTransferActivityState::WaitingForPeer
            | FfiTransferActivityState::Pairing
            | FfiTransferActivityState::Connecting
            | FfiTransferActivityState::Transferring
            | FfiTransferActivityState::Verifying
    ) && !is_finalizing_activity(activity)
}

fn can_resume_durable_activity(activity: &FfiTransferActivityRecord) -> bool {
    matches!(
        activity.state,
        FfiTransferActivityState::Paused
            | FfiTransferActivityState::Unconfirmed
            | FfiTransferActivityState::Failed
            | FfiTransferActivityState::Canceled
    )
}

fn can_cancel_durable_activity(activity: &FfiTransferActivityRecord) -> bool {
    matches!(
        activity.state,
        FfiTransferActivityState::Queued
            | FfiTransferActivityState::Binding
            | FfiTransferActivityState::WaitingForPeer
            | FfiTransferActivityState::Pairing
            | FfiTransferActivityState::Connecting
            | FfiTransferActivityState::Transferring
            | FfiTransferActivityState::Verifying
            | FfiTransferActivityState::Paused
            | FfiTransferActivityState::Unconfirmed
            | FfiTransferActivityState::Publishing
    ) && !is_finalizing_activity(activity)
}

#[uniffi::export]
pub fn start_durable_transfer(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserver>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    start_durable_transfer_impl(
        settings,
        request,
        records_dir,
        observer,
        NativeMailboxObserver::V1(mailbox),
        None,
    )
}

/// Starts a durable transfer with a versioned courier contract. The receipt
/// endpoint is frozen into the canonical context before the first snapshot.
#[uniffi::export]
pub fn start_durable_transfer_v2(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    records_dir: String,
    receipt_server: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserverV2>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    let receipt_server = normalized_receipt_server(&receipt_server)?;
    start_durable_transfer_impl(
        settings,
        request,
        records_dir,
        observer,
        NativeMailboxObserver::V2(mailbox),
        receipt_server,
    )
}

fn start_durable_transfer_impl(
    settings: EnvoixRuntimeSettings,
    mut request: FfiTransferRequest,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: NativeMailboxObserver,
    receipt_server: Option<String>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    if request.activity_id.trim().is_empty() {
        request.activity_id = next_activity_id();
    }
    normalize_transfer_limits(&settings, &mut request.limits);
    validate_transfer_request(&settings, &request)?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = RecordStore::new(records_dir);
    let record_id = stable_record_id(&request.activity_id);
    let mut context = canonical_context_for_request(&settings, &request)?;
    if receipt_server.is_some() {
        context.client.receipt_server = receipt_server;
    }
    if context.requires_stable_listener_identity() {
        context.client.identity_file = Some(store.identity_path(record_id));
    }
    let activity = Arc::new(Mutex::new(FfiTransferActivityRecord::from_request(
        &request,
        now_ms(),
    )));
    let runtime = durable_runtime()?;
    if let Some(existing) = runtime.block_on(store.load(record_id))
        && external_activity_id(&existing) != request.activity_id
    {
        return Err(EnvoixError::Operation {
            reason: "activity id collided with an existing durable record".to_string(),
        });
    }
    let extras = serde_json::json!({ "external_record_id": request.activity_id.clone() });
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalTransferSession::start(
            context.clone(),
            Some((store, record_id)),
            Some(extras.clone()),
        )
        .map_err(op_err)?
    };
    let pending_receipt_key = Arc::new(Mutex::new(None));
    let session = Arc::new(DurableEnvoixSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
        pending_receipt_key: pending_receipt_key.clone(),
        platform_extras: Mutex::new(extras),
    });
    runtime.handle().spawn(drive_durable_notices(
        request.activity_id,
        context,
        notices,
        activity,
        observer,
        mailbox,
        pending_receipt_key,
    ));
    Ok(session)
}

#[uniffi::export]
pub fn restore_durable_transfer(
    activity_id: String,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserver>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    restore_durable_transfer_impl(
        activity_id,
        records_dir,
        observer,
        NativeMailboxObserver::V1(mailbox),
    )
}

/// Restores a durable transfer using the endpoint-aware courier. The endpoint
/// comes exclusively from the persisted session context.
#[uniffi::export]
pub fn restore_durable_transfer_v2(
    activity_id: String,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserverV2>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    restore_durable_transfer_impl(
        activity_id,
        records_dir,
        observer,
        NativeMailboxObserver::V2(mailbox),
    )
}

fn restore_durable_transfer_impl(
    activity_id: String,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: NativeMailboxObserver,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    let activity_id = required_value(&activity_id, "activity_id")?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = RecordStore::new(records_dir);
    let runtime = durable_runtime()?;
    let mut record = runtime
        .block_on(store.load_all())
        .into_iter()
        .find(|record| external_activity_id(record) == activity_id)
        .ok_or_else(|| EnvoixError::Operation {
            reason: format!("transfer record not found: {activity_id}"),
        })?;
    if record.context.requires_stable_listener_identity()
        && record.context.client.identity_file.is_none()
    {
        record.context.client.identity_file = Some(store.identity_path(record.id));
    }
    let record_id = record.id;
    let context = record.context.clone();
    let platform_extras = record
        .platform_extras
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "external_record_id": activity_id.clone() }));
    let activity = Arc::new(Mutex::new(activity_from_canonical_record(&record)));
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalTransferSession::restore(record, Some((store, record_id))).map_err(op_err)?
    };
    let pending_receipt_key = Arc::new(Mutex::new(None));
    let session = Arc::new(DurableEnvoixSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
        pending_receipt_key: pending_receipt_key.clone(),
        platform_extras: Mutex::new(platform_extras),
    });
    runtime.handle().spawn(drive_durable_notices(
        activity_id,
        context,
        notices,
        activity,
        observer,
        mailbox,
        pending_receipt_key,
    ));
    Ok(session)
}

#[uniffi::export]
pub fn list_durable_transfer_records(
    records_dir: String,
) -> Result<Vec<FfiTransferActivityRecord>, EnvoixError> {
    let records_dir = required_value(&records_dir, "records_dir")?;
    let runtime = durable_runtime()?;
    Ok(runtime
        .block_on(RecordStore::new(records_dir).load_all())
        .iter()
        .map(activity_from_canonical_record)
        .collect())
}

fn durable_runtime() -> Result<&'static Runtime, EnvoixError> {
    DURABLE_RUNTIME
