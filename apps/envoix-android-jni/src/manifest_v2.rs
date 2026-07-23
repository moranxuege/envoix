use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use envoix_client::api::{
    CanonicalTransferJob, Client, CompressionPolicyV2, DestinationDecisionV2, DestinationRequestV2,
    EventSink, JobIdV2, JobLifecycle, LocalSourceOrigin, ManifestV2DataError,
    ManifestV2ProgressPhase, ManifestV2ResultGate, PairingConfig, PendingManifestV2Receive,
    ProviderSourceIssue, RootPlanV2, SavedEntryV2, SenderManifestV2SessionSummary, SourceDecision,
    SourceIssueKind, SourceItemId, SourceSelectionState, TransferCancelToken, TransferEvent,
    TransferJobStore, TransferOptions, local_allocatable_bytes, parse_broker_addr,
    receive_manifest_v2_offer_enable_mdns, receive_manifest_v2_offer_via_room,
    send_manifest_v2_enable_mdns, send_manifest_v2_via_room,
};
use envoix_error::{CoreError, TransferCause};
use envoix_protocol::manifest_v2::{ManifestEntryKindV2, ManifestV2};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jlong, jstring};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use super::*;

type DecisionMap = HashMap<i64, oneshot::Sender<ReceiveDecision>>;
static MANIFEST_CANCELS: OnceLock<Mutex<HashMap<i64, TransferCancelToken>>> = OnceLock::new();
static RECEIVE_DECISIONS: OnceLock<Mutex<DecisionMap>> = OnceLock::new();
static OFFER_INVENTORIES: OnceLock<Mutex<HashMap<i64, Vec<OfferEntry>>>> = OnceLock::new();

fn manifest_cancels() -> &'static Mutex<HashMap<i64, TransferCancelToken>> {
    MANIFEST_CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn receive_decisions() -> &'static Mutex<DecisionMap> {
    RECEIVE_DECISIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn offer_inventories() -> &'static Mutex<HashMap<i64, Vec<OfferEntry>>> {
    OFFER_INVENTORIES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CompressionChoice {
    Never,
    Always,
    Smart,
}

impl From<CompressionChoice> for CompressionPolicyV2 {
    fn from(value: CompressionChoice) -> Self {
        match value {
            CompressionChoice::Never => Self::Never,
            CompressionChoice::Always => Self::Always,
            CompressionChoice::Smart => Self::Smart,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    direction: String,
    room: String,
    broker: String,
    relay: String,
    state_directory: String,
    job_store_directory: String,
    #[serde(default)]
    job_id: Option<String>,
    use_room: bool,
    use_mdns: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveDecision {
    target_directory: String,
    target_allocatable_bytes: u64,
    exceptional_transfer_approved: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedProviderRoot {
    path: String,
    requested_name: String,
    origin: ProviderOrigin,
    issues: Vec<PreparedProviderIssue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderOrigin {
    Photos,
    Share,
    ContentUri,
    FileProvider,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedProviderIssue {
    relative_components: Vec<String>,
    kind: ProviderIssueKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderIssueKind {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SpecialFile,
}

#[derive(Clone, Serialize)]
struct OfferEntry {
    entry_id: u32,
    root_id: u32,
    parent_entry_id: Option<u32>,
    name: String,
    kind: &'static str,
    plaintext_size: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveReply {
    roots: Vec<SavedRoot>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlannedRoot {
    root_id: u32,
    planned_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanReply {
    roots: Vec<PlannedRoot>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedRoot {
    root_id: u32,
    final_name: String,
    uri: String,
}

struct AndroidEvents {
    vm: Arc<JavaVM>,
    callback: Arc<GlobalRef>,
    direction: &'static str,
}

impl AndroidEvents {
    fn send(&self, value: Value) {
        emit(self.vm.as_ref(), self.callback.as_ref(), &value.to_string());
    }
}

impl EventSink for AndroidEvents {
    fn on_event(&self, event: TransferEvent) {
        match event {
            TransferEvent::Diagnostic { message } => {
                self.send(json!({"notice":"manifest_v2","kind":"diagnostic","message":message}));
            }
            TransferEvent::Pairing { step } => {
                self.send(json!({"notice":"manifest_v2","state":"connecting","pairing":format!("{step:?}")}));
            }
            TransferEvent::Connecting => {
                self.send(json!({"notice":"manifest_v2","state":"connecting"}));
            }
            TransferEvent::Connected { path } | TransferEvent::PathChanged { path } => {
                self.send(json!({"notice":"manifest_v2","kind":"path","path":path.to_string()}));
            }
            TransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => self.send(json!({
                "notice":"manifest_v2",
                "kind":"progress",
                "bytes":bytes_transferred,
                "total":total_bytes,
            })),
            TransferEvent::ManifestV2Phase { phase, .. } => {
                let state = match phase {
                    ManifestV2ProgressPhase::Transferring if self.direction == "receive" => {
                        "receiving"
                    }
                    ManifestV2ProgressPhase::Transferring => "transferring",
                    ManifestV2ProgressPhase::Verifying => "verifying",
                    ManifestV2ProgressPhase::Saving => "saving",
                    ManifestV2ProgressPhase::WaitingForReceiverSave => "waiting_for_receiver_save",
                    ManifestV2ProgressPhase::FinalizingDelivery => "finalizing_delivery",
                };
                self.send(json!({"notice":"manifest_v2","state":state}));
            }
        }
    }
}

struct AndroidResultGate {
    vm: Arc<JavaVM>,
    callback: Arc<GlobalRef>,
    target_directory: PathBuf,
    public_roots: Vec<PlannedRoot>,
    committed: Mutex<Option<Vec<SavedRoot>>>,
}

#[async_trait]
impl ManifestV2ResultGate for AndroidResultGate {
    async fn commit_results(
        &self,
        manifest: &ManifestV2,
        saved_entries: &mut [SavedEntryV2],
    ) -> Result<(), ManifestV2DataError> {
        let public_by_root = self
            .public_roots
            .iter()
            .map(|root| (root.root_id, root))
            .collect::<BTreeMap<_, _>>();
        let root_entries = manifest
            .roots
            .iter()
            .map(|root| {
                let public = public_by_root.get(&root.root_id).ok_or_else(|| {
                    ManifestV2DataError::DestinationContract(
                        "Android public name plan omitted a root".into(),
                    )
                })?;
                let result = saved_entries
                    .get(root.root_entry_id as usize)
                    .ok_or_else(|| {
                        ManifestV2DataError::DestinationContract(
                            "root result is missing before Android save".into(),
                        )
                    })?;
                let private_name = result
                    .final_component_override
                    .as_deref()
                    .unwrap_or(root.requested_name.as_str());
                let entry = &manifest.entries[root.root_entry_id as usize];
                Ok(json!({
                    "root_id": root.root_id,
                    "local_path": self.target_directory.join(private_name).to_string_lossy(),
                    "planned_name": public.planned_name,
                    "kind": match entry.kind {
                        ManifestEntryKindV2::RegularFile => "file",
                        ManifestEntryKindV2::Directory => "directory",
                    },
                }))
            })
            .collect::<Result<Vec<_>, ManifestV2DataError>>()?;
        let request = json!({
            "job_id": encode_job_id(manifest.job_id),
            "generation": manifest.generation,
            "roots": root_entries,
        })
        .to_string();
        let reply = call_save_required(self.vm.as_ref(), self.callback.as_ref(), &request)?;
        let reply: SaveReply = serde_json::from_str(&reply).map_err(|error| {
            ManifestV2DataError::DestinationContract(format!(
                "Android save reply is invalid: {error}"
            ))
        })?;
        if reply.roots.len() != manifest.roots.len() {
            return Err(ManifestV2DataError::DestinationContract(
                "Android did not save every root".into(),
            ));
        }
        let by_root = reply
            .roots
            .iter()
            .map(|root| (root.root_id, root))
            .collect::<BTreeMap<_, _>>();
        if by_root.len() != manifest.roots.len() {
            return Err(ManifestV2DataError::DestinationContract(
                "Android save reply contains duplicate roots".into(),
            ));
        }
        for root in &manifest.roots {
            let saved = by_root.get(&root.root_id).ok_or_else(|| {
                ManifestV2DataError::DestinationContract(
                    "Android save reply omitted a manifest root".into(),
                )
            })?;
            if !valid_component(&saved.final_name) || saved.uri.trim().is_empty() {
                return Err(ManifestV2DataError::DestinationContract(
                    "Android returned an invalid final name or URI".into(),
                ));
            }
            if saved.final_name != public_by_root[&root.root_id].planned_name {
                return Err(ManifestV2DataError::DestinationContract(
                    "Android destination changed a frozen public name".into(),
                ));
            }
            saved_entries[root.root_entry_id as usize].final_component_override =
                Some(saved.final_name.clone());
        }
        *self.committed.lock().map_err(|_| {
            ManifestV2DataError::Internal("Android save result mutex is poisoned".into())
        })? = Some(reply.roots);
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_createManifestV2Job(
    mut env: JNIEnv,
    _class: JClass,
    store_directory: JString,
    compression: JString,
) -> jstring {
    let store_directory = jstr(&mut env, &store_directory);
    let compression = jstr(&mut env, &compression);
    let result = runtime().block_on(async {
        let policy: CompressionChoice = serde_json::from_str(&format!("\"{compression}\""))
            .map_err(|_| "compression must be never, always, or smart".to_string())?;
        require_directory(&store_directory, "job store")?;
        let store = TransferJobStore::new(&store_directory);
        let job = CanonicalTransferJob::new(policy.into()).map_err(|error| error.to_string())?;
        store.save(&job).await.map_err(|error| error.to_string())?;
        Ok::<_, String>(job_snapshot(&job))
    });
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_listManifestV2PreparingJobs(
    mut env: JNIEnv,
    _class: JClass,
    store_directory: JString,
) -> jstring {
    let store_directory = jstr(&mut env, &store_directory);
    let result = runtime().block_on(async {
        require_directory(&store_directory, "job store")?;
        let jobs = TransferJobStore::new(&store_directory)
            .load_all()
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(json!({
            "jobs": jobs
                .iter()
                .filter(|job| {
                    !job.source_selections().is_empty()
                        && matches!(
                            job.lifecycle(),
                            JobLifecycle::Preparing
                                | JobLifecycle::NeedsSourceDecision
                                | JobLifecycle::ReadyToSend
                        )
                })
                .map(job_snapshot)
                .collect::<Vec<_>>()
        }))
    });
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_prepareManifestV2Job(
    mut env: JNIEnv,
    _class: JClass,
    store_directory: JString,
    job_id: JString,
    paths_json: JString,
) -> jstring {
    let store_directory = jstr(&mut env, &store_directory);
    let job_id = jstr(&mut env, &job_id);
    let paths_json = jstr(&mut env, &paths_json);
    let result = runtime().block_on(async {
        let roots: Vec<PreparedProviderRoot> = serde_json::from_str(&paths_json)
            .map_err(|error| format!("prepared roots JSON is invalid: {error}"))?;
        if roots.is_empty()
            || roots
                .iter()
                .any(|root| root.path.trim().is_empty() || root.requested_name.trim().is_empty())
        {
            return Err("prepared roots must contain a path and requested name".into());
        }
        let store = TransferJobStore::new(&store_directory);
        let mut job = load_job(&store, &job_id).await?;
        for root in roots {
            let origin = match root.origin {
                ProviderOrigin::Photos => LocalSourceOrigin::PhotosStaging,
                ProviderOrigin::Share => LocalSourceOrigin::ShareStaging,
                ProviderOrigin::ContentUri => LocalSourceOrigin::ContentUriStaging,
                ProviderOrigin::FileProvider => LocalSourceOrigin::FileProviderStaging,
            };
            let issues = core_provider_issues(root.issues);
            job.add_provider_path(
                PathBuf::from(root.path),
                root.requested_name,
                origin,
                issues,
            )
            .await
            .map_err(|error| error.to_string())?;
            store.save(&job).await.map_err(|error| error.to_string())?;
        }
        Ok::<_, String>(job_snapshot(&job))
    });
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_reauthorizeManifestV2ProviderSource(
    mut env: JNIEnv,
    _class: JClass,
    store_directory: JString,
    job_id: JString,
    root_item_id: jlong,
    root_json: JString,
) -> jstring {
    let store_directory = jstr(&mut env, &store_directory);
    let job_id = jstr(&mut env, &job_id);
    let root_json = jstr(&mut env, &root_json);
    let result = runtime().block_on(async {
        let mut roots: Vec<PreparedProviderRoot> = serde_json::from_str(&root_json)
            .map_err(|error| format!("prepared provider root JSON is invalid: {error}"))?;
        if roots.len() != 1 {
            return Err("reauthorization requires exactly one stabilized root".into());
        }
        let root = roots.remove(0);
        if root.path.trim().is_empty() {
            return Err("reauthorized provider path must not be empty".into());
        }
        let root_item_id = u64::try_from(root_item_id)
            .map(SourceItemId)
            .map_err(|_| "root_item_id must be non-negative".to_string())?;
        let store = TransferJobStore::new(&store_directory);
        let mut job = load_job(&store, &job_id).await?;
        job.reauthorize_provider_source(
            root_item_id,
            PathBuf::from(root.path),
            core_provider_issues(root.issues),
        )
        .await
        .map_err(|error| error.to_string())?;
        store.save(&job).await.map_err(|error| error.to_string())?;
        Ok::<_, String>(job_snapshot(&job))
    });
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_resolveManifestV2Source(
    mut env: JNIEnv,
    _class: JClass,
    store_directory: JString,
    job_id: JString,
    root_item_id: jlong,
    decision: JString,
    reauthorized_path: JString,
) -> jstring {
    let store_directory = jstr(&mut env, &store_directory);
    let job_id = jstr(&mut env, &job_id);
    let decision = jstr(&mut env, &decision);
    let reauthorized_path = jstr(&mut env, &reauthorized_path);
    let result = runtime().block_on(async {
        let store = TransferJobStore::new(&store_directory);
        let mut job = load_job(&store, &job_id).await?;
        let source_decision = match decision.as_str() {
            "approve_partial" => SourceDecision::ApprovePartial,
            "remove_selection" => SourceDecision::RemoveSelection,
            "cancel_job" => SourceDecision::CancelJob,
            "reauthorize" if !reauthorized_path.trim().is_empty() => SourceDecision::Reauthorize {
                local_path: PathBuf::from(&reauthorized_path),
            },
            "reauthorize" => return Err("reauthorized path is required".into()),
            _ => return Err("unknown source decision".into()),
        };
        let reprepare = matches!(&source_decision, SourceDecision::Reauthorize { .. });
        let root_item_id = u64::try_from(root_item_id)
            .map(SourceItemId)
            .map_err(|_| "root_item_id must be non-negative".to_string())?;
        store
            .apply_source_decision(&mut job, root_item_id, source_decision)
            .await
            .map_err(|error| error.to_string())?;
        if reprepare {
            job.prepare_selection(root_item_id)
                .await
                .map_err(|error| error.to_string())?;
            store.save(&job).await.map_err(|error| error.to_string())?;
        }
        Ok::<_, String>(job_snapshot(&job))
    });
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_startManifestV2Session(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let params_json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "startManifestV2Session") else {
        return;
    };
    let Some(callback) = callback_or_log(&env, &callback, "startManifestV2Session") else {
        return;
    };
    let params: StartParams = match serde_json::from_str(&params_json) {
        Ok(params) => params,
        Err(error) => {
            emit_failed_manifest(&vm, &callback, "invalid Manifest v2 params", error);
            return;
        }
    };
    if params.direction != "send" && params.direction != "receive" {
        emit_failed_manifest(
            &vm,
            &callback,
            "invalid Manifest v2 direction",
            params.direction,
        );
        return;
    }
    let token = TransferCancelToken::new();
    let Ok(mut cancels) = manifest_cancels().lock() else {
        emit_failed_manifest(&vm, &callback, "Manifest v2 registry unavailable", id);
        return;
    };
    if cancels.insert(id, token.clone()).is_some() {
        emit_failed_manifest(&vm, &callback, "Manifest v2 session is already active", id);
        return;
    }
    drop(cancels);
    let vm = Arc::new(vm);
    let callback = Arc::new(callback);
    runtime().spawn(async move {
        let result = run_session(id, &params, vm.clone(), callback.clone(), &token).await;
        if let Err(error) = result {
            let (code, detail) = error_fact(&error);
            emit(
                vm.as_ref(),
                callback.as_ref(),
                &json!({
                    "notice":"manifest_v2",
                    "state":"failed",
                    "cause":code,
                    "detail":detail,
                })
                .to_string(),
            );
        }
        if let Ok(mut map) = manifest_cancels().lock() {
            map.remove(&id);
        }
        if let Ok(mut map) = receive_decisions().lock() {
            map.remove(&id);
        }
        if let Ok(mut map) = offer_inventories().lock() {
            map.remove(&id);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_continueManifestV2Receive(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    decision_json: JString,
) -> jstring {
    let decision_json = jstr(&mut env, &decision_json);
    let result = (|| {
        let decision = serde_json::from_str::<ReceiveDecision>(&decision_json)
            .map_err(|error| format!("invalid receive decision: {error}"))?;
        if decision.target_directory.trim().is_empty() || decision.target_allocatable_bytes == 0 {
            return Err(
                "receive destination and non-zero allocatable capacity are required".into(),
            );
        }
        let sender = receive_decisions()
            .lock()
            .map_err(|_| "receive decision registry unavailable".to_string())?
            .remove(&id)
            .ok_or_else(|| "this offer is not waiting for a decision".to_string())?;
        sender
            .send(decision)
            .map_err(|_| "the receive session ended before the decision arrived".to_string())?;
        Ok(json!({"accepted":true}))
    })();
    json_result(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_listManifestV2OfferEntries(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    offset: jlong,
    limit: jlong,
) -> jstring {
    let result = (|| {
        let offset = usize::try_from(offset).map_err(|_| "offset must be non-negative")?;
        let limit = usize::try_from(limit)
            .map_err(|_| "limit must be positive")?
            .clamp(1, 512);
        let inventories = offer_inventories()
            .lock()
            .map_err(|_| "offer inventory registry unavailable")?;
        let entries = inventories
            .get(&id)
            .ok_or("offer inventory is unavailable")?;
        let start = offset.min(entries.len());
        let end = start.saturating_add(limit).min(entries.len());
        Ok::<_, &str>(json!({
            "entries": &entries[start..end],
            "next_offset": (end < entries.len()).then_some(end),
        }))
    })();
    json_result(&mut env, result.map_err(str::to_string))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_cancelManifestV2Session(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    if let Some(token) = manifest_cancels()
        .lock()
        .ok()
        .and_then(|map| map.get(&id).cloned())
    {
        token.cancel();
    }
}

async fn run_session(
    id: i64,
    params: &StartParams,
    vm: Arc<JavaVM>,
    callback: Arc<GlobalRef>,
    cancel: &TransferCancelToken,
) -> Result<(), CoreError> {
    require_directory(&params.state_directory, "state directory")
        .map_err(CoreError::InvalidInput)?;
    require_directory(&params.job_store_directory, "job store directory")
        .map_err(CoreError::InvalidInput)?;
    let mut options = TransferOptions::default();
    options.relay = (!params.relay.trim().is_empty()).then(|| params.relay.clone());
    let config = Client::default().session_config(&options);
    if !params.use_room && !params.use_mdns {
        return Err(CoreError::InvalidInput(
            "at least one rendezvous route must be enabled".into(),
        ));
    }
    let broker = params
        .use_room
        .then(|| parse_broker_addr(&params.broker, options.relay.as_deref()))
        .transpose()?;
    let events: Arc<dyn EventSink> = Arc::new(AndroidEvents {
        vm: vm.clone(),
        callback: callback.clone(),
        direction: if params.direction == "send" {
            "send"
        } else {
            "receive"
        },
    });
    if params.direction == "send" {
        let job_id = params
            .job_id
            .as_deref()
            .ok_or_else(|| CoreError::InvalidInput("send requires job_id".into()))?;
        let store = TransferJobStore::new(&params.job_store_directory);
        let mut job = load_job(&store, job_id)
            .await
            .map_err(CoreError::InvalidInput)?;
        if job.manifest().is_none() {
            job.seal_for_send()
                .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
            store
                .save(&job)
                .await
                .map_err(|error| CoreError::Storage(error.to_string()))?;
        }
        send_with_enabled_routes(
            EnabledSendRoutes {
                broker,
                code: &params.room,
                use_mdns: params.use_mdns,
            },
            &job,
            PathBuf::from(&params.state_directory),
            config,
            events,
            cancel,
        )
        .await?;
        emit(
            vm.as_ref(),
            callback.as_ref(),
            &json!({"notice":"manifest_v2","state":"completed","job_id":job_id}).to_string(),
        );
        return Ok(());
    }

    let pending = receive_from_enabled_routes(
        broker,
        &params.room,
        params.use_room,
        params.use_mdns,
        config,
        events,
        cancel,
    )
    .await?;
    let manifest = &pending.offer().manifest;
    let inventory = manifest
        .entries
        .iter()
        .map(|entry| OfferEntry {
            entry_id: entry.entry_id,
            root_id: entry.root_id,
            parent_entry_id: entry.parent_entry_id,
            name: entry.component.clone(),
            kind: match entry.kind {
                ManifestEntryKindV2::RegularFile => "file",
                ManifestEntryKindV2::Directory => "directory",
            },
            plaintext_size: entry.plaintext_size,
        })
        .collect::<Vec<_>>();
    offer_inventories()
        .lock()
        .map_err(|_| CoreError::Transfer("offer inventory registry unavailable".into()))?
        .insert(id, inventory);
    let (decision_sender, decision_receiver) = oneshot::channel();
    receive_decisions()
        .lock()
        .map_err(|_| CoreError::Transfer("receive decision registry unavailable".into()))?
        .insert(id, decision_sender);
    let exceptional = manifest.totals.total_plaintext_bytes
        > envoix_client::api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES;
    emit(
        vm.as_ref(),
        callback.as_ref(),
        &json!({
            "notice":"manifest_v2",
            "state":"offer",
            "job_id":encode_job_id(manifest.job_id),
            "root_count":manifest.roots.len(),
            "file_count":manifest.totals.file_count,
            "directory_count":manifest.totals.directory_count,
            "total":manifest.totals.total_plaintext_bytes,
            "exceptional":exceptional,
        })
        .to_string(),
    );
    let decision = tokio::select! {
        decision = decision_receiver => decision.map_err(|_| CoreError::Cancelled)?,
        () = cancel.cancelled() => return Err(CoreError::Cancelled),
    };
    let target_directory = PathBuf::from(&decision.target_directory);
    let actual_capacity = local_allocatable_bytes(&target_directory)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    let capacity = decision.target_allocatable_bytes.min(actual_capacity);
    let plan_request = json!({
        "job_id": encode_job_id(manifest.job_id),
        "generation": manifest.generation,
        "reserved_names": [".envoix-staging-v2", ".envoix-reservations-v2"],
        "roots": manifest.roots.iter().map(|root| {
            let entry = &manifest.entries[root.root_entry_id as usize];
            json!({
                "root_id": root.root_id,
                "requested_name": root.requested_name,
                "kind": match entry.kind {
                    ManifestEntryKindV2::RegularFile => "file",
                    ManifestEntryKindV2::Directory => "directory",
                },
            })
        }).collect::<Vec<_>>(),
    })
    .to_string();
    let public_plan =
        call_plan_required(vm.as_ref(), callback.as_ref(), &plan_request).map_err(|error| {
            CoreError::Cause {
                cause: TransferCause::ReceiverDestinationUnavailable,
                detail: error.to_string(),
            }
        })?;
    let public_plan: PlanReply =
        serde_json::from_str(&public_plan).map_err(|error| CoreError::Cause {
            cause: TransferCause::ReceiverDestinationUnavailable,
            detail: format!("Android destination plan reply is invalid: {error}"),
        })?;
    validate_public_plan(manifest, &public_plan.roots)?;
    let gate = AndroidResultGate {
        vm: vm.clone(),
        callback: callback.clone(),
        target_directory: target_directory.clone(),
        public_roots: public_plan.roots.clone(),
        committed: Mutex::new(None),
    };
    let summary = pending
        .receive_with_result_gate(
            DestinationRequestV2 {
                target_directory,
                copy_staging_directory: None,
                decision: DestinationDecisionV2::UseDirectSave,
                target_allocatable_bytes: Some(capacity),
                staging_allocatable_bytes: None,
                // Android's public target is SAF/MediaStore, for which this
                // adapter deliberately has no stable reusable object identity.
                stable_object_identity: false,
                exceptional_transfer_approved: decision.exceptional_transfer_approved,
                preplanned_root_names: Some(
                    public_plan
                        .roots
                        .iter()
                        .map(|root| RootPlanV2 {
                            root_id: root.root_id,
                            planned_name: root.planned_name.clone(),
                        })
                        .collect(),
                ),
            },
            PathBuf::from(&params.state_directory),
            &gate,
            cancel,
        )
        .await?;
    let roots = gate
        .committed
        .lock()
        .map_err(|_| CoreError::Transfer("Android save result mutex is poisoned".into()))?
        .clone()
        .unwrap_or_default();
    emit(
        vm.as_ref(),
        callback.as_ref(),
        &json!({
            "notice":"manifest_v2",
            "state":"completed",
            "job_id":encode_job_id(summary.destination_plan.job_id),
            "roots":roots,
        })
        .to_string(),
    );
    Ok(())
}

struct EnabledSendRoutes<'a> {
    broker: Option<envoix_client::EndpointAddr>,
    code: &'a str,
    use_mdns: bool,
}

async fn send_with_enabled_routes(
    routes: EnabledSendRoutes<'_>,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<SenderManifestV2SessionSummary, CoreError> {
    let mut last_error = None;
    if let Some(broker) = routes.broker {
        match send_manifest_v2_via_room(
            broker,
            routes.code,
            job,
            state_directory.clone(),
            config.clone(),
            events.clone(),
            cancel,
        )
        .await
        {
            Ok(summary) => return Ok(summary),
            Err(error) if !cancel.is_cancelled() => {
                events.on_event(TransferEvent::Diagnostic {
                    message: format!("Room route failed; trying another enabled route: {error}"),
                });
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    if routes.use_mdns {
        let pairing = PairingConfig::spake2_shared_token(routes.code.to_string())?;
        match send_manifest_v2_enable_mdns(
            job.clone(),
            state_directory,
            config,
            &pairing,
            events,
            cancel.clone(),
        )
        .await
        {
            Ok(summary) => return Ok(summary),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| CoreError::InvalidInput("no enabled send route is available".into())))
}

async fn receive_from_enabled_routes(
    broker: Option<envoix_client::EndpointAddr>,
    code: &str,
    use_room: bool,
    use_mdns: bool,
    config: envoix_client::api::SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, CoreError> {
    if use_room && !use_mdns {
        let broker = broker
            .ok_or_else(|| CoreError::InvalidInput("Room rendezvous requires a broker".into()))?;
        return receive_manifest_v2_offer_via_room(
            broker,
            code,
            envoix_client::BindAddrs::dual_stack(0),
            config,
            events,
            cancel,
        )
        .await;
    }
    if use_mdns && !use_room {
        let pairing = PairingConfig::spake2_shared_token(code.to_string())?;
        return receive_manifest_v2_offer_enable_mdns(
            envoix_client::BindAddrs::dual_stack(0),
            config,
            &pairing,
            events,
            |_, _| {},
            cancel,
        )
        .await;
    }

    let broker = broker
        .ok_or_else(|| CoreError::InvalidInput("Room rendezvous requires a broker".into()))?;
    let room_cancel = TransferCancelToken::new();
    let mdns_cancel = TransferCancelToken::new();
    let mut routes = JoinSet::new();
    {
        let code = code.to_string();
        let config = config.clone();
        let events = events.clone();
        let route_cancel = room_cancel.clone();
        routes.spawn(async move {
            let result = receive_manifest_v2_offer_via_room(
                broker,
                &code,
                envoix_client::BindAddrs::dual_stack(0),
                config,
                events,
                &route_cancel,
            )
            .await;
            (0_usize, result)
        });
    }
    {
        let pairing = PairingConfig::spake2_shared_token(code.to_string())?;
        let events = events.clone();
        let route_cancel = mdns_cancel.clone();
        routes.spawn(async move {
            let result = receive_manifest_v2_offer_enable_mdns(
                envoix_client::BindAddrs::dual_stack(0),
                config,
                &pairing,
                events,
                |_, _| {},
                &route_cancel,
            )
            .await;
            (1_usize, result)
        });
    }

    let mut last_error = None;
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                room_cancel.cancel();
                mdns_cancel.cancel();
                while routes.join_next().await.is_some() {}
                return Err(CoreError::Cancelled);
            }
            joined = routes.join_next() => match joined {
                Some(Ok((winner, Ok(pending)))) => {
                    if winner == 0 { mdns_cancel.cancel(); } else { room_cancel.cancel(); }
                    while routes.join_next().await.is_some() {}
                    return Ok(pending);
                }
                Some(Ok((_, Err(error)))) => {
                    events.on_event(TransferEvent::Diagnostic {
                        message: format!("Receive route failed; keeping the other route active: {error}"),
                    });
                    last_error = Some(error);
                    if routes.is_empty() { break; }
                }
                Some(Err(error)) => {
                    last_error = Some(CoreError::Transfer(format!("receive route task failed: {error}")));
                    if routes.is_empty() { break; }
                }
                None => break,
            },
            () = cancel.cancelled() => {
                room_cancel.cancel();
                mdns_cancel.cancel();
                while routes.join_next().await.is_some() {}
                return Err(CoreError::Cancelled);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| CoreError::InvalidInput("no enabled receive route is available".into())))
}

fn call_save_required(
    vm: &JavaVM,
    callback: &GlobalRef,
    request: &str,
) -> Result<String, ManifestV2DataError> {
    call_string_callback(vm, callback, "onSaveRequired", request)
}

fn call_plan_required(
    vm: &JavaVM,
    callback: &GlobalRef,
    request: &str,
) -> Result<String, ManifestV2DataError> {
    call_string_callback(vm, callback, "onPlanRequired", request)
}

fn call_string_callback(
    vm: &JavaVM,
    callback: &GlobalRef,
    method: &str,
    request: &str,
) -> Result<String, ManifestV2DataError> {
    let mut env = vm.attach_current_thread().map_err(|error| {
        ManifestV2DataError::Internal(format!("attach Android platform callback: {error}"))
    })?;
    let request = env.new_string(request).map_err(|error| {
        ManifestV2DataError::Internal(format!("allocate Android callback request: {error}"))
    })?;
    let request_object = JObject::from(request);
    let result = match env.call_method(
        callback.as_obj(),
        method,
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&request_object)],
    ) {
        Ok(value) => value.l().map_err(|error| {
            ManifestV2DataError::DestinationContract(format!(
                "Android platform callback returned an invalid value: {error}"
            ))
        })?,
        Err(error) => {
            if env.exception_check().unwrap_or(false) {
                env.exception_clear().map_err(|clear_error| {
                    ManifestV2DataError::Internal(format!(
                        "clear Android platform callback exception: {clear_error}"
                    ))
                })?;
            }
            return Err(ManifestV2DataError::DestinationContract(format!(
                "Android platform callback failed: {error}"
            )));
        }
    };
    if result.is_null() {
        return Err(ManifestV2DataError::DestinationContract(
            "Android platform callback returned null".into(),
        ));
    }
    let result = JString::from(result);
    env.get_string(&result)
        .map(|value| value.into())
        .map_err(|error| {
            ManifestV2DataError::DestinationContract(format!(
                "read Android platform callback result: {error}"
            ))
        })
}

fn validate_public_plan(manifest: &ManifestV2, roots: &[PlannedRoot]) -> Result<(), CoreError> {
    let mut names = std::collections::HashSet::new();
    if roots.len() != manifest.roots.len()
        || roots.iter().enumerate().any(|(index, root)| {
            root.root_id != index as u32
                || !valid_component(&root.planned_name)
                || !names.insert(root.planned_name.to_lowercase())
        })
    {
        return Err(CoreError::Cause {
            cause: TransferCause::ReceiverDestinationUnavailable,
            detail: "Android public destination returned an invalid root name plan".into(),
        });
    }
    Ok(())
}

async fn load_job(store: &TransferJobStore, encoded: &str) -> Result<CanonicalTransferJob, String> {
    let job_id = decode_job_id(encoded)?;
    store
        .load(job_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Manifest v2 job was not found".into())
}

fn decode_job_id(encoded: &str) -> Result<JobIdV2, String> {
    if encoded.len() != 32 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("job_id must contain exactly 32 hexadecimal characters".into());
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16)
            .map_err(|_| "job_id contains invalid hexadecimal".to_string())?;
    }
    Ok(JobIdV2(bytes))
}

fn encode_job_id(job_id: JobIdV2) -> String {
    job_id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn job_snapshot(job: &CanonicalTransferJob) -> Value {
    let inventory = job.inventory_summary();
    json!({
        "job_id":encode_job_id(job.job_id()),
        "state":match job.lifecycle() {
            JobLifecycle::Preparing => "preparing",
            JobLifecycle::NeedsSourceDecision => "needs_source_decision",
            JobLifecycle::ReadyToSend => "ready_to_send",
            JobLifecycle::Sealed => "sealed",
            JobLifecycle::Canceled => "canceled",
        },
        "selection_revision":job.selection_revision(),
        "updated_unix_ms":job.updated_unix_ms(),
        "root_count":inventory.root_count,
        "file_count":inventory.file_count,
        "directory_count":inventory.directory_count,
        "total":inventory.total_plaintext_bytes,
        "warning_count":inventory.warning_count,
        "selections":job.source_selections().into_iter().map(|selection| {
            let local_path = job.local_path_for_item(selection.root_item_id);
            json!({
            "root_item_id":selection.root_item_id.0,
            "name":selection.requested_name,
            "local_path":local_path.map(|path| path.to_string_lossy().into_owned()),
            "directory":local_path.is_some_and(Path::is_dir),
            "state":match selection.state {
                SourceSelectionState::Pending => "pending",
                SourceSelectionState::Enumerating => "enumerating",
                SourceSelectionState::NeedsDecision => "needs_decision",
                SourceSelectionState::Ready => "ready",
            },
            "partial_approved":selection.partial_approved,
            "issues":selection.issues.into_iter().map(|issue| json!({
                "issue_id":issue.issue_id,
                "path":issue.relative_components,
                "kind":source_issue_kind(issue.kind),
            })).collect::<Vec<_>>(),
        })}).collect::<Vec<_>>(),
    })
}

fn source_issue_kind(kind: SourceIssueKind) -> &'static str {
    match kind {
        SourceIssueKind::PermissionDenied => "permission_denied",
        SourceIssueKind::Unavailable => "unavailable",
        SourceIssueKind::InvalidName => "invalid_name",
        SourceIssueKind::SymbolicLink => "symbolic_link",
        SourceIssueKind::SpecialFile => "special_file",
        SourceIssueKind::SourceChanged => "source_changed",
        SourceIssueKind::DepthLimit => "depth_limit",
        SourceIssueKind::EntryLimit => "entry_limit",
    }
}

fn core_provider_issues(issues: Vec<PreparedProviderIssue>) -> Vec<ProviderSourceIssue> {
    issues
        .into_iter()
        .map(|issue| ProviderSourceIssue {
            relative_components: issue.relative_components,
            kind: match issue.kind {
                ProviderIssueKind::PermissionDenied => SourceIssueKind::PermissionDenied,
                ProviderIssueKind::Unavailable => SourceIssueKind::Unavailable,
                ProviderIssueKind::InvalidName => SourceIssueKind::InvalidName,
                ProviderIssueKind::SpecialFile => SourceIssueKind::SpecialFile,
            },
        })
        .collect()
}

fn json_result(env: &mut JNIEnv, result: Result<Value, String>) -> jstring {
    let value = match result {
        Ok(value) => value,
        Err(error) => json!({"error":error}),
    };
    to_jstring(env, &value.to_string())
}

fn require_directory(path: &str, label: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    let path = Path::new(path);
    std::fs::create_dir_all(path).map_err(|error| format!("create {label}: {error}"))?;
    if !path.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(())
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
}

fn error_fact(error: &CoreError) -> (&'static str, String) {
    match error {
        CoreError::Cause { cause, detail } => (cause.code(), detail.clone()),
        CoreError::Cancelled => ("user_canceled", "operation cancelled".into()),
        CoreError::InvalidInput(detail) => ("invalid_input", detail.clone()),
        CoreError::Io(detail) => ("io", detail.clone()),
        CoreError::Protocol(detail) => ("protocol_or_integrity_failure", detail.clone()),
        CoreError::Transport(detail) => ("transport", detail.clone()),
        CoreError::Crypto(detail) => ("protocol_or_integrity_failure", detail.clone()),
        CoreError::Storage(detail) => ("receiver_save_failed", detail.clone()),
        CoreError::Discovery(detail) => ("discovery", detail.clone()),
        CoreError::Transfer(detail) => ("transfer", detail.clone()),
    }
}

fn emit_failed_manifest(
    vm: &JavaVM,
    callback: &GlobalRef,
    context: &str,
    error: impl std::fmt::Display,
) {
    emit(
        vm,
        callback,
        &json!({
            "notice":"manifest_v2",
            "state":"failed",
            "cause":"invalid_input",
            "detail":format!("{context}: {error}"),
        })
        .to_string(),
    );
}
