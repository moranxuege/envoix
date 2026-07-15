//! Additive UniFFI projection for durable Manifest activities.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use envoix_client::api::{
    ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryV1, ManifestHashAlgorithm,
    ManifestId, ManifestSendRequest, ManifestV1, StampedEvent,
    driver::{ClientContext, SessionContext, SessionParams, SessionSnapshot},
    machine::{PauseOrigin, State as CanonicalState},
    manifest_activity::{
        ManifestActivity, ManifestOperation, ManifestRecordStore, ManifestSessionContext,
        ManifestSessionParams, ManifestTransferRecord,
    },
    manifest_driver::{
        ManifestSessionNotice, ManifestSessionSnapshot,
        ManifestTransferSession as CanonicalManifestTransferSession,
    },
};
use tokio::sync::mpsc;

use super::{
    EXTERNAL_RECORD_ID_KEY, EnvoixError, EnvoixRuntimeSettings, FfiNativePublicationTarget,
    FfiTransferActivityActions, FfiTransferActivityRecord, FfiTransferActivityState,
    FfiTransferDirection, FfiTransferFailure, FfiTransferRequest, NATIVE_PROGRESS_INTERVAL_MS,
    NATIVE_PUBLICATION_EXTRAS_KEY, PersistedNativePublication, apply_canonical_snapshot,
    durable_runtime, ffi_direction, native_publication_metadata_from_extras, next_activity_id,
    normalize_transfer_limits, now_ms, op_err, peer_sources_for_request, required_path,
    required_value, stable_record_id, to_ffi_event, transfer_activity_actions,
    transfer_options_for_request, validate_direction_mode,
};

/// Portable entry type in a prepared Manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestEntryKind {
    File,
    Directory,
}

/// One typed entry returned by Manifest preparation.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPreparedManifestEntry {
    pub entry_id: u32,
    pub relative_path: String,
    pub kind: FfiManifestEntryKind,
    pub size: u64,
    /// BLAKE3-256 for files; empty for directories.
    pub hash: Vec<u8>,
    pub modified_at_unix_ms: Option<u64>,
    /// Local source for files; empty for directories.
    pub source_path: String,
}

/// Fully prepared, typed Manifest send plan. Native code may persist this
/// value, but Rust revalidates every field before a network attempt.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPreparedManifestSend {
    pub manifest_id: String,
    pub entries: Vec<FfiPreparedManifestEntry>,
    pub file_count: u32,
    pub directory_count: u32,
    pub root_count: u32,
    pub total_bytes: u64,
}

impl FfiPreparedManifestSend {
    fn from_core(request: &ManifestSendRequest) -> Result<Self, EnvoixError> {
        let entries = request
            .manifest
            .entries
            .iter()
            .map(|entry| {
                let source_path = if entry.kind == ManifestEntryKind::RegularFile {
                    request
                        .source_path(entry.entry_id)
                        .map_err(op_err)?
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                };
                Ok(FfiPreparedManifestEntry {
                    entry_id: entry.entry_id,
                    relative_path: entry.relative_path.clone(),
                    kind: ffi_manifest_entry_kind(entry.kind),
                    size: entry.size,
                    hash: entry.hash.map_or_else(Vec::new, |value| value.to_vec()),
                    modified_at_unix_ms: entry.modified_at_unix_ms,
                    source_path,
                })
            })
            .collect::<Result<Vec<_>, EnvoixError>>()?;
        Ok(Self {
            manifest_id: request.manifest.manifest_id.to_string(),
            entries,
            file_count: request.manifest.file_count,
            directory_count: request.manifest.directory_count,
            root_count: request.manifest.root_count,
            total_bytes: request.manifest.total_bytes,
        })
    }

    fn to_core(&self) -> Result<ManifestSendRequest, EnvoixError> {
        let manifest_id = required_value(&self.manifest_id, "manifest_id")?;
        let mut entries = Vec::with_capacity(self.entries.len());
        let mut source_paths = Vec::new();
        for entry in &self.entries {
            let (kind, hash) =
                match entry.kind {
                    FfiManifestEntryKind::File => {
                        let hash: [u8; 32] = entry.hash.as_slice().try_into().map_err(|_| {
                            EnvoixError::Operation {
                                reason: format!(
                                    "Manifest file entry {} must have a 32-byte hash",
                                    entry.entry_id
                                ),
                            }
                        })?;
                        if entry.source_path.is_empty() {
                            return Err(EnvoixError::Operation {
                                reason: "source_path must not be empty".to_string(),
                            });
                        }
                        source_paths.push((entry.entry_id, PathBuf::from(&entry.source_path)));
                        (ManifestEntryKind::RegularFile, Some(hash))
                    }
                    FfiManifestEntryKind::Directory => {
                        if !entry.hash.is_empty() || !entry.source_path.is_empty() {
                            return Err(EnvoixError::Operation {
                                reason: format!(
                                    "Manifest directory entry {} cannot have a hash or source file",
                                    entry.entry_id
                                ),
                            });
                        }
                        (ManifestEntryKind::Directory, None)
                    }
                };
            entries.push(ManifestEntryV1 {
                entry_id: entry.entry_id,
                relative_path: entry.relative_path.clone(),
                kind,
                size: entry.size,
                hash,
                modified_at_unix_ms: entry.modified_at_unix_ms,
            });
        }
        ManifestSendRequest::new(
            ManifestV1 {
                manifest_id: ManifestId::new(manifest_id),
                entries,
                file_count: self.file_count,
                directory_count: self.directory_count,
                root_count: self.root_count,
                total_bytes: self.total_bytes,
                hash_algorithm: ManifestHashAlgorithm::Blake3_256,
            },
            source_paths,
        )
        .map_err(op_err)
    }
}

/// Prepares selected files/folders without blocking the Swift/Kotlin caller.
#[uniffi::export]
pub async fn prepare_manifest_send(
    activity_id: String,
    selected_paths: Vec<String>,
) -> Result<FfiPreparedManifestSend, EnvoixError> {
    let activity_id = if activity_id.trim().is_empty() {
        next_activity_id()
    } else {
        activity_id.trim().to_string()
    };
    if selected_paths.is_empty() || selected_paths.iter().any(String::is_empty) {
        return Err(EnvoixError::Operation {
            reason: "selected_paths must contain at least one non-empty path".to_string(),
        });
    }
    durable_runtime()?
        .handle()
        .spawn(async move {
            let request = ManifestSendRequest::from_paths(
                ManifestId::new(activity_id),
                selected_paths.into_iter().map(PathBuf::from),
            )
            .await
            .map_err(op_err)?;
            FfiPreparedManifestSend::from_core(&request)
        })
        .await
        .map_err(|error| EnvoixError::Operation {
            reason: format!("Manifest preparation task failed: {error}"),
        })?
}

/// Phase of the active Manifest entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestEntryPhase {
    None,
    Preparing,
    Transferring,
}

/// Active per-entry projection.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestCurrentEntry {
    pub entry_id: u32,
    pub phase: FfiManifestEntryPhase,
    pub transfer_id: String,
    pub relative_path: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub bytes_resumed: u64,
}

/// Receiver-authoritative terminal status for one Manifest entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestEntryResultStatus {
    Completed,
    SkippedIdentical,
    Renamed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestEntryResult {
    pub entry_id: u32,
    pub status: FfiManifestEntryResultStatus,
    pub offered_relative_path: String,
    pub final_relative_path: String,
    pub failure_code: String,
}

/// Manifest-aware Activity record. `activity` retains the compatible common
/// card surface; the remaining fields describe the transfer set.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestActivityRecord {
    pub activity: FfiTransferActivityRecord,
    pub manifest_id: String,
    pub root_count: u32,
    pub file_count: u32,
    pub directory_count: u32,
    pub completed_files: u32,
    pub entries: Vec<FfiPreparedManifestEntry>,
    pub current_entry: Option<FfiManifestCurrentEntry>,
    pub entry_results: Vec<FfiManifestEntryResult>,
}

/// Observer implemented by Apple/Android for one durable Manifest card.
#[uniffi::export(with_foreign)]
pub trait ManifestTransferObserver: Send + Sync {
    fn on_manifest_activity(&self, record: FfiManifestActivityRecord);
}

/// One durable Manifest transfer card.
#[derive(uniffi::Object)]
pub struct DurableEnvoixManifestSession {
    driver: Mutex<Option<CanonicalManifestTransferSession>>,
    activity: Arc<Mutex<FfiManifestActivityRecord>>,
    platform_extras: Mutex<serde_json::Value>,
}

#[uniffi::export]
impl DurableEnvoixManifestSession {
    pub fn pause(&self) -> bool {
        let activity = self.activity.lock().unwrap();
        if !manifest_can_pause(&activity.activity) {
            return false;
        }
        drop(activity);
        self.driver
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(CanonicalManifestTransferSession::pause)
    }

    pub fn resume(&self) -> bool {
        let activity = self.activity.lock().unwrap();
        if !manifest_can_resume(&activity.activity) {
            return false;
        }
        drop(activity);
        self.driver
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(CanonicalManifestTransferSession::resume)
    }

    pub fn cancel(&self) -> bool {
        let activity = self.activity.lock().unwrap();
        if !manifest_can_cancel(&activity.activity) {
            return false;
        }
        drop(activity);
        self.driver
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(CanonicalManifestTransferSession::cancel)
    }

    pub fn actions(&self) -> FfiTransferActivityActions {
        transfer_activity_actions(self.activity.lock().unwrap().activity.clone())
    }

    pub fn set_publication_target(&self, mut target: FfiNativePublicationTarget) -> bool {
        target.destination_path = target.destination_path.trim().to_string();
        let activity = self.activity.lock().unwrap();
        if target.destination_path.is_empty()
            || activity.activity.direction != FfiTransferDirection::Receive
            || matches!(
                activity.activity.state,
                FfiTransferActivityState::Completed
                    | FfiTransferActivityState::Canceled
                    | FfiTransferActivityState::Failed
            )
        {
            return false;
        }
        drop(activity);
        self.update_publication_metadata(PersistedNativePublication {
            target: Some(target),
            failure: None,
        })
    }

    pub fn publication_target(&self) -> Option<FfiNativePublicationTarget> {
        native_publication_metadata_from_extras(&self.platform_extras.lock().unwrap())?.target
    }

    pub fn publication_failed(&self, failure: FfiTransferFailure) -> bool {
        let activity = self.activity.lock().unwrap();
        if activity.activity.state != FfiTransferActivityState::Publishing
            || !failure.retryable
            || !matches!(
                failure.direction,
                FfiTransferDirection::Receive | FfiTransferDirection::Unknown
            )
        {
            return false;
        }
        drop(activity);
        let mut publication =
            native_publication_metadata_from_extras(&self.platform_extras.lock().unwrap())
                .unwrap_or_default();
        publication.failure = Some(failure.clone());
        if !self.update_publication_metadata(publication) {
            return false;
        }
        self.activity
            .lock()
            .unwrap()
            .activity
            .apply_publication_failure(&failure, now_ms());
        true
    }

    pub fn publication_succeeded(&self, path: String) -> bool {
        let path = path.trim();
        if path.is_empty()
            || self.activity.lock().unwrap().activity.state != FfiTransferActivityState::Publishing
        {
            return false;
        }
        self.driver
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|driver| driver.published(path.to_string()))
    }

    pub fn remove(&self) -> bool {
        self.driver
            .lock()
            .unwrap()
            .take()
            .is_some_and(|driver| driver.discard())
    }

    pub fn activity(&self) -> FfiManifestActivityRecord {
        self.activity.lock().unwrap().clone()
    }
}

#[uniffi::export]
pub fn start_durable_manifest_send(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    prepared: FfiPreparedManifestSend,
    records_dir: String,
    observer: Arc<dyn ManifestTransferObserver>,
) -> Result<Arc<DurableEnvoixManifestSession>, EnvoixError> {
    let operation = ManifestOperation::Send {
        request: prepared.to_core()?,
    };
    start_durable_manifest(settings, request, operation, records_dir, observer)
}

#[uniffi::export]
pub fn start_durable_manifest_receive(
    settings: EnvoixRuntimeSettings,
    request: FfiTransferRequest,
    records_dir: String,
    observer: Arc<dyn ManifestTransferObserver>,
) -> Result<Arc<DurableEnvoixManifestSession>, EnvoixError> {
    let output_dir = required_path(&request.output_dir, "output_dir")?;
    start_durable_manifest(
        settings,
        request,
        ManifestOperation::Receive {
            output_dir: PathBuf::from(output_dir),
        },
        records_dir,
        observer,
    )
}

fn start_durable_manifest(
    settings: EnvoixRuntimeSettings,
    mut request: FfiTransferRequest,
    operation: ManifestOperation,
    records_dir: String,
    observer: Arc<dyn ManifestTransferObserver>,
) -> Result<Arc<DurableEnvoixManifestSession>, EnvoixError> {
    let expected_direction = ffi_direction(Some(operation.direction()));
    if request.activity_id.trim().is_empty() {
        request.activity_id = operation
            .send_request()
            .map(|prepared| prepared.manifest.manifest_id.to_string())
            .unwrap_or_else(next_activity_id);
    }
    if let Some(prepared) = operation.send_request()
        && prepared.manifest.manifest_id.0 != request.activity_id
    {
        return Err(EnvoixError::Operation {
            reason: "prepared Manifest id must match activity_id".to_string(),
        });
    }
    if request.direction != expected_direction {
        return Err(EnvoixError::Operation {
            reason: format!(
                "Manifest operation requires {expected_direction:?}, got {:?}",
                request.direction
            ),
        });
    }
    normalize_transfer_limits(&settings, &mut request.limits);
    validate_manifest_transport(&settings, &request)?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = ManifestRecordStore::new(records_dir);
    let record_id = stable_record_id(&request.activity_id);
    let mut context = canonical_manifest_context(&settings, &request, operation)?;
    if context.requires_stable_listener_identity() {
        context.client.identity_file = Some(store.identity_path(record_id));
    }
    let runtime = durable_runtime()?;
    if let Some(existing) = runtime.block_on(store.load(record_id))
        && external_manifest_activity_id(&existing) != request.activity_id
    {
        return Err(EnvoixError::Operation {
            reason: "activity id collided with an existing durable Manifest record".to_string(),
        });
    }
    let extras = serde_json::json!({ EXTERNAL_RECORD_ID_KEY: request.activity_id.clone() });
    let canonical_activity = ManifestActivity::new(&context).map_err(op_err)?;
    let activity = Arc::new(Mutex::new(manifest_record_from_activity(
        &request,
        &context,
        canonical_activity,
        0,
        now_ms(),
        now_ms(),
    )));
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalManifestTransferSession::start(
            context.clone(),
            Some((store, record_id)),
            Some(extras.clone()),
        )
        .map_err(op_err)?
    };
    let session = Arc::new(DurableEnvoixManifestSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
        platform_extras: Mutex::new(extras),
    });
    runtime
        .handle()
        .spawn(drive_manifest_notices(context, notices, activity, observer));
    Ok(session)
}

#[uniffi::export]
pub fn restore_durable_manifest_transfer(
    activity_id: String,
    records_dir: String,
    observer: Arc<dyn ManifestTransferObserver>,
) -> Result<Arc<DurableEnvoixManifestSession>, EnvoixError> {
    let activity_id = required_value(&activity_id, "activity_id")?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = ManifestRecordStore::new(records_dir);
    let runtime = durable_runtime()?;
    let mut record = runtime
        .block_on(store.load_all())
        .into_iter()
        .find(|record| external_manifest_activity_id(record) == activity_id)
        .ok_or_else(|| EnvoixError::Operation {
            reason: format!("Manifest transfer record not found: {activity_id}"),
        })?;
    if record.context.requires_stable_listener_identity()
        && record.context.client.identity_file.is_none()
    {
        record.context.client.identity_file = Some(store.identity_path(record.id));
    }
    let context = record.context.clone();
    let record_id = record.id;
    let platform_extras = record
        .platform_extras
        .clone()
        .unwrap_or_else(|| serde_json::json!({ EXTERNAL_RECORD_ID_KEY: activity_id.clone() }));
    let request = request_from_manifest_context(&activity_id, &context);
    let activity = Arc::new(Mutex::new(manifest_record_from_canonical_record(
        &request, &record,
    )));
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalManifestTransferSession::restore(record, Some((store, record_id)))
            .map_err(op_err)?
    };
    let session = Arc::new(DurableEnvoixManifestSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
        platform_extras: Mutex::new(platform_extras),
    });
    runtime
        .handle()
        .spawn(drive_manifest_notices(context, notices, activity, observer));
    Ok(session)
}

#[uniffi::export]
pub fn list_durable_manifest_records(
    records_dir: String,
) -> Result<Vec<FfiManifestActivityRecord>, EnvoixError> {
    let records_dir = required_value(&records_dir, "records_dir")?;
    let runtime = durable_runtime()?;
    Ok(runtime
        .block_on(ManifestRecordStore::new(records_dir).load_all())
        .iter()
        .map(|record| {
            let activity_id = external_manifest_activity_id(record);
            let request = request_from_manifest_context(&activity_id, &record.context);
            manifest_record_from_canonical_record(&request, record)
        })
        .collect())
}

fn validate_manifest_transport(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<(), EnvoixError> {
    validate_direction_mode(request.direction, request.mode)?;
    if request.direction == FfiTransferDirection::Receive {
        required_path(&request.output_dir, "output_dir")?;
    }
    super::build_client_for_request(settings, request)?;
    transfer_options_for_request(settings, request, None)?;
    peer_sources_for_request(settings, request)?;
    Ok(())
}

fn canonical_manifest_context(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    operation: ManifestOperation,
) -> Result<ManifestSessionContext, EnvoixError> {
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
    let context = ManifestSessionContext {
        client,
        params: ManifestSessionParams {
            operation,
            sources,
            options: transfer_options_for_request(settings, request, None)?,
            publication_required: request.publication_required,
        },
    };
    context.validate().map_err(op_err)?;
    Ok(context)
}

fn projection_context(context: &ManifestSessionContext) -> SessionContext {
    let path = match &context.params.operation {
        ManifestOperation::Receive { output_dir } => output_dir.clone(),
        ManifestOperation::Send { request } => request
            .manifest
            .entries
            .iter()
            .find(|entry| entry.kind == ManifestEntryKind::RegularFile)
            .and_then(|entry| request.source_path(entry.entry_id).ok())
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    };
    SessionContext {
        client: context.client.clone(),
        params: SessionParams {
            direction: context.params.direction(),
            path,
            sources: context.params.sources.clone(),
            options: context.params.options.clone(),
            publication_required: context.params.publication_required,
        },
    }
}

fn request_from_manifest_context(
    activity_id: &str,
    context: &ManifestSessionContext,
) -> FfiTransferRequest {
    let projection = projection_context(context);
    let mut request = super::request_from_canonical_context(activity_id, &projection);
    request.activity_id = activity_id.to_string();
    request
}

fn external_manifest_activity_id(record: &ManifestTransferRecord) -> String {
    record
        .platform_extras
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|extras| extras.get(EXTERNAL_RECORD_ID_KEY))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| record.id.to_string())
}

async fn drive_manifest_notices(
    context: ManifestSessionContext,
    mut notices: mpsc::UnboundedReceiver<ManifestSessionNotice>,
    activity: Arc<Mutex<FfiManifestActivityRecord>>,
    observer: Arc<dyn ManifestTransferObserver>,
) {
    let projection = projection_context(&context);
    let mut previous_activity: Option<ManifestActivity> = None;
    let mut last_progress_activity_ms = 0_u64;
    while let Some(notice) = notices.recv().await {
        match notice {
            ManifestSessionNotice::Event(event) => {
                observe_manifest_transport_event(&activity, event);
            }
            ManifestSessionNotice::Snapshot(snapshot) => {
                let timestamp = now_ms();
                let progress_only = previous_activity.as_ref().is_some_and(|previous| {
                    is_manifest_progress_only(previous, &snapshot.activity)
                });
                let terminal_or_publication = matches!(
                    snapshot.activity.session.state,
                    CanonicalState::AwaitingPublication
                        | CanonicalState::Completed
                        | CanonicalState::Failed
                        | CanonicalState::Cancelled
                );
                let emit = !progress_only
                    || terminal_or_publication
                    || last_progress_activity_ms == 0
                    || timestamp.saturating_sub(last_progress_activity_ms)
                        >= NATIVE_PROGRESS_INTERVAL_MS;
                let record = {
                    let mut record = activity.lock().unwrap();
                    apply_manifest_snapshot(&mut record, &snapshot, &projection, timestamp);
                    record.clone()
                };
                if emit {
                    if progress_only {
                        last_progress_activity_ms = timestamp;
                    }
                    observer.on_manifest_activity(record);
                }
                previous_activity = Some(snapshot.activity);
            }
        }
    }
}

fn observe_manifest_transport_event(
    activity: &Arc<Mutex<FfiManifestActivityRecord>>,
    event: StampedEvent,
) {
    let mut activity = activity.lock().unwrap();
    let ffi_event = to_ffi_event(&event, &activity.activity.activity_id);
    activity.activity.apply_observation(&ffi_event);
}

fn is_manifest_progress_only(previous: &ManifestActivity, current: &ManifestActivity) -> bool {
    if previous.session.bytes == current.session.bytes
        && previous.current_entry.as_ref().map(|entry| entry.bytes)
            == current.current_entry.as_ref().map(|entry| entry.bytes)
    {
        return false;
    }
    let mut normalized = current.clone();
    normalized.session.bytes = previous.session.bytes;
    if let (Some(previous), Some(current)) = (
        previous.current_entry.as_ref(),
        normalized.current_entry.as_mut(),
    ) && previous.entry_id == current.entry_id
    {
        current.bytes = previous.bytes;
    }
    &normalized == previous
}

fn manifest_record_from_canonical_record(
    request: &FfiTransferRequest,
    record: &ManifestTransferRecord,
) -> FfiManifestActivityRecord {
    let mut activity = record.activity.clone();
    if matches!(
        activity.session.state,
        CanonicalState::Waiting
            | CanonicalState::Connecting
            | CanonicalState::Verifying
            | CanonicalState::Transferring
            | CanonicalState::Confirming
    ) {
        activity.session.state = CanonicalState::Paused(PauseOrigin::Lost);
        activity.session.reason = Some("interrupted by an app restart".to_string());
    }
    let mut projected = manifest_record_from_activity(
        request,
        &record.context,
        activity,
        0,
        record.created_ms,
        record.updated_ms,
    );
    if projected.activity.state == FfiTransferActivityState::Publishing
        && let Some(failure) = record
            .platform_extras
            .as_ref()
            .and_then(native_publication_metadata_from_extras)
            .and_then(|publication| publication.failure)
    {
        projected
            .activity
            .apply_publication_failure(&failure, record.updated_ms);
    }
    projected
}

fn manifest_record_from_activity(
    request: &FfiTransferRequest,
    context: &ManifestSessionContext,
    activity: ManifestActivity,
    sequence: u64,
    created_ms: u64,
    updated_ms: u64,
) -> FfiManifestActivityRecord {
    let mut record = FfiManifestActivityRecord {
        activity: FfiTransferActivityRecord::from_request(request, created_ms),
        manifest_id: String::new(),
        root_count: 0,
        file_count: 0,
        directory_count: 0,
        completed_files: 0,
        entries: Vec::new(),
        current_entry: None,
        entry_results: Vec::new(),
    };
    apply_manifest_snapshot(
        &mut record,
        &ManifestSessionSnapshot {
            seq: sequence,
            speed_bps: 0.0,
            avg_bps: 0.0,
            activity,
        },
        &projection_context(context),
        updated_ms,
    );
    record.activity.created_at_ms = created_ms;
    record
}

fn apply_manifest_snapshot(
    record: &mut FfiManifestActivityRecord,
    snapshot: &ManifestSessionSnapshot,
    projection: &SessionContext,
    timestamp: u64,
) {
    apply_canonical_snapshot(
        &mut record.activity,
        &SessionSnapshot {
            seq: snapshot.seq,
            speed_bps: snapshot.speed_bps,
            avg_bps: snapshot.avg_bps,
            session: snapshot.activity.session.clone(),
        },
        projection,
        timestamp,
    );
    let activity = &snapshot.activity;
    record.completed_files = activity.completed_files;
    record.current_entry = activity
        .current_entry
        .as_ref()
        .map(|entry| FfiManifestCurrentEntry {
            entry_id: entry.entry_id,
            phase: match entry.phase {
                envoix_client::api::manifest_activity::ManifestEntryPhase::Preparing => {
                    FfiManifestEntryPhase::Preparing
                }
                envoix_client::api::manifest_activity::ManifestEntryPhase::Transferring => {
                    FfiManifestEntryPhase::Transferring
                }
            },
            transfer_id: entry.transfer_id.clone().unwrap_or_default(),
            relative_path: entry.relative_path.clone(),
            bytes_transferred: entry.bytes,
            total_bytes: entry.total,
            bytes_resumed: entry.bytes_resumed,
        });
    record.entry_results = activity
        .entry_results
        .iter()
        .map(|result| FfiManifestEntryResult {
            entry_id: result.entry_id,
            status: ffi_manifest_result_status(result.status),
            offered_relative_path: result.offered_relative_path.clone(),
            final_relative_path: result.final_relative_path.clone().unwrap_or_default(),
            failure_code: result.failure_code.clone().unwrap_or_default(),
        })
        .collect();
    let Some(manifest) = &activity.manifest else {
        record.manifest_id.clear();
        record.root_count = 0;
        record.file_count = 0;
        record.directory_count = 0;
        record.entries.clear();
        return;
    };
    record.manifest_id = manifest.manifest_id.to_string();
    record.root_count = manifest.root_count;
    record.file_count = manifest.file_count;
    record.directory_count = manifest.directory_count;
    record.activity.file_name = manifest_display_name(manifest);
    record.entries = manifest
        .entries
        .iter()
        .map(|entry| FfiPreparedManifestEntry {
            entry_id: entry.entry_id,
            relative_path: entry.relative_path.clone(),
            kind: ffi_manifest_entry_kind(entry.kind),
            size: entry.size,
            hash: entry.hash.map_or_else(Vec::new, |value| value.to_vec()),
            modified_at_unix_ms: entry.modified_at_unix_ms,
            source_path: String::new(),
        })
        .collect();
}

fn manifest_display_name(manifest: &ManifestV1) -> String {
    if manifest.root_count == 1 {
        manifest
            .entries
            .first()
            .map(|entry| entry.relative_path.clone())
            .unwrap_or_else(|| manifest.manifest_id.to_string())
    } else {
        format!("{} items", manifest.root_count)
    }
}

fn manifest_can_pause(activity: &FfiTransferActivityRecord) -> bool {
    matches!(
        activity.state,
        FfiTransferActivityState::Queued
            | FfiTransferActivityState::Binding
            | FfiTransferActivityState::WaitingForPeer
            | FfiTransferActivityState::Pairing
            | FfiTransferActivityState::Connecting
            | FfiTransferActivityState::Transferring
            | FfiTransferActivityState::Verifying
    )
}

fn manifest_can_resume(activity: &FfiTransferActivityRecord) -> bool {
    matches!(
        activity.state,
        FfiTransferActivityState::Paused
            | FfiTransferActivityState::Failed
            | FfiTransferActivityState::Canceled
    )
}

fn manifest_can_cancel(activity: &FfiTransferActivityRecord) -> bool {
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
            | FfiTransferActivityState::Publishing
    )
}

fn ffi_manifest_entry_kind(kind: ManifestEntryKind) -> FfiManifestEntryKind {
    match kind {
        ManifestEntryKind::RegularFile => FfiManifestEntryKind::File,
        ManifestEntryKind::Directory => FfiManifestEntryKind::Directory,
    }
}

fn ffi_manifest_result_status(status: ManifestEntryResultStatus) -> FfiManifestEntryResultStatus {
    match status {
        ManifestEntryResultStatus::Completed => FfiManifestEntryResultStatus::Completed,
        ManifestEntryResultStatus::SkippedIdentical => {
            FfiManifestEntryResultStatus::SkippedIdentical
        }
        ManifestEntryResultStatus::Renamed => FfiManifestEntryResultStatus::Renamed,
        ManifestEntryResultStatus::Failed => FfiManifestEntryResultStatus::Failed,
        ManifestEntryResultStatus::Cancelled => FfiManifestEntryResultStatus::Canceled,
    }
}

impl DurableEnvoixManifestSession {
    fn update_publication_metadata(&self, publication: PersistedNativePublication) -> bool {
        let mut extras = self.platform_extras.lock().unwrap();
        let mut candidate = extras.clone();
        let Some(object) = candidate.as_object_mut() else {
            return false;
        };
        object.insert(
            NATIVE_PUBLICATION_EXTRAS_KEY.to_string(),
            serde_json::to_value(publication).expect("native publication metadata must serialize"),
        );
        if !self
            .driver
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|driver| driver.set_extras(candidate.clone()))
        {
            return false;
        }
        *extras = candidate;
        self.activity
            .lock()
            .unwrap()
            .activity
            .clear_failure_metadata(now_ms());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as sync_mpsc;
    use std::time::Duration;

    use tempfile::tempdir;

    struct RecordingObserver {
        records: sync_mpsc::Sender<FfiManifestActivityRecord>,
    }

    impl ManifestTransferObserver for RecordingObserver {
        fn on_manifest_activity(&self, record: FfiManifestActivityRecord) {
            let _ = self.records.send(record);
        }
    }

    #[tokio::test]
    async fn async_preparation_returns_a_typed_revalidatable_plan() {
        let selected = tempdir().unwrap();
        let album = selected.path().join("Album");
        tokio::fs::create_dir(&album).await.unwrap();
        tokio::fs::write(album.join("b.jpg"), b"b").await.unwrap();
        tokio::fs::write(album.join("a.jpg"), b"aa").await.unwrap();

        let prepared = prepare_manifest_send(
            "ffi-manifest".into(),
            vec![album.to_string_lossy().into_owned()],
        )
        .await
        .unwrap();

        assert_eq!(prepared.manifest_id, "ffi-manifest");
        assert_eq!(prepared.file_count, 2);
        assert_eq!(prepared.directory_count, 1);
        assert_eq!(prepared.root_count, 1);
        assert_eq!(
            prepared
                .entries
                .iter()
                .map(|entry| entry.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["Album", "Album/a.jpg", "Album/b.jpg"]
        );
        assert!(prepared.to_core().is_ok());

        let mut tampered = prepared;
        tampered.entries[1].hash.pop();
        assert!(tampered.to_core().is_err());
    }

    #[tokio::test]
    async fn projection_keeps_common_card_and_manifest_inventory_together() {
        let selected = tempdir().unwrap();
        let album = selected.path().join("Album");
        tokio::fs::create_dir(&album).await.unwrap();
        tokio::fs::write(album.join("photo.jpg"), b"photo")
            .await
            .unwrap();
        let prepared =
            ManifestSendRequest::from_paths(ManifestId::new("projection-manifest"), [album])
                .await
                .unwrap();
        let mut request = FfiTransferRequest::send(
            selected.path().to_string_lossy().into_owned(),
            super::super::FfiTransferMode::Mdns,
        );
        request.activity_id = "projection-manifest".into();
        request.token = "stable-test-token".into();
        let context = canonical_manifest_context(
            &EnvoixRuntimeSettings::default(),
            &request,
            ManifestOperation::Send { request: prepared },
        )
        .unwrap();
        let activity = ManifestActivity::new(&context).unwrap();

        let projected = manifest_record_from_activity(&request, &context, activity, 7, 10, 20);

        assert_eq!(projected.activity.activity_id, "projection-manifest");
        assert_eq!(projected.activity.sequence, 7);
        assert_eq!(projected.activity.file_name, "Album");
        assert_eq!(projected.manifest_id, "projection-manifest");
        assert_eq!(projected.root_count, 1);
        assert_eq!(projected.file_count, 1);
        assert_eq!(projected.directory_count, 1);
        assert_eq!(projected.entries.len(), 2);
    }

    #[test]
    fn durable_bridge_persists_and_lists_a_failed_manifest_attempt() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("photo.jpg");
        std::fs::write(&source, b"photo").unwrap();
        let prepared = durable_runtime()
            .unwrap()
            .block_on(ManifestSendRequest::from_paths(
                ManifestId::new("ffi-durable-manifest"),
                [source.clone()],
            ))
            .unwrap();
        let prepared = FfiPreparedManifestSend::from_core(&prepared).unwrap();
        let mut request = FfiTransferRequest::send(
            source.to_string_lossy().into_owned(),
            super::super::FfiTransferMode::Room,
        );
        request.activity_id = "ffi-durable-manifest".into();
        request.code = "123456-test-code".into();
        request.broker = "invalid-broker".into();
        request.rendezvous.use_mdns = false;
        let records_dir = temp.path().join("records");
        let (sender, receiver) = sync_mpsc::channel();

        let _session = start_durable_manifest_send(
            EnvoixRuntimeSettings::default(),
            request,
            prepared,
            records_dir.to_string_lossy().into_owned(),
            Arc::new(RecordingObserver { records: sender }),
        )
        .unwrap();

        let failed = (0..5)
            .find_map(|_| {
                receiver
                    .recv_timeout(Duration::from_secs(2))
                    .ok()
                    .filter(|record| record.activity.state == FfiTransferActivityState::Failed)
            })
            .expect("Manifest attempt should fail visibly");
        assert_eq!(failed.manifest_id, "ffi-durable-manifest");
        let stored =
            list_durable_manifest_records(records_dir.to_string_lossy().into_owned()).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].activity.state, FfiTransferActivityState::Failed);
    }
}
