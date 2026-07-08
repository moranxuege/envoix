//! uniffi bridge exposing the envoix client core to native UIs (Swift/Kotlin).
//!
//! The bridge is intentionally thin: it wires the unified envoix client API to a
//! small, foreign-implementable observer.
//! All networking, pairing, and transfer logic stays in the Rust core.
//!
//! Operations are non-blocking. Each call spawns work on a session-owned tokio
//! runtime and returns immediately; results arrive through [`TransferObserver`]
//! callbacks, which fire on runtime threads — the UI must hop to its own main
//! thread before touching UI state.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use envoix_client::{
    BindAddrs, PeerDescriptor, TransferDirection, TransferSummary,
    api::{
        Client, DataPath, FailureCategory, FailureCode, FailureOrigin, FailurePhase, PairingStep,
        PathPolicy, PeerSource, RecoveryAction, StampedEvent, Transfer, TransferError,
        TransferEvent, TransferMode, TransferOptions,
    },
};
use envoix_rendezvous_iroh::generate_code;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

uniffi::setup_scaffolding!();

/// Lifetime of a generated invite before it expires, in seconds.
const INVITE_TTL_SECS: u64 = 300;
/// Default rendezvous broker used by the macOS app for room pairing.
const DEFAULT_RENDEZVOUS_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
/// Default relay used with the hosted rendezvous broker.
const DEFAULT_RELAY_URL: &str = "https://envoix.chkxwlyh.us:8444";
static NEXT_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);
/// Runtime settings supplied by native UIs.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct EnvoixRuntimeSettings {
    /// Whether the UI permits send and receive tasks at the same time.
    pub concurrent_transfers: bool,
    /// UI language preference, kept for cross-platform settings parity.
    pub language: String,
    /// Optional rendezvous broker URL/address. Empty uses the built-in default.
    pub server_url: String,
    /// Optional relay URL. Empty uses the built-in default.
    pub relay_url: String,
    /// Optional path to a RuntimeConfig TOML file. Empty means no extra config.
    pub config_path: String,
    /// Reserved for future throttling; currently advisory only.
    pub speed_limit_mbps: u64,
}

impl Default for EnvoixRuntimeSettings {
    fn default() -> Self {
        Self {
            concurrent_transfers: true,
            language: "en".to_string(),
            server_url: String::new(),
            relay_url: String::new(),
            config_path: String::new(),
            speed_limit_mbps: 40,
        }
    }
}

/// Generates a short room code such as `135790-amber-comet`.
#[uniffi::export]
pub fn generate_room_code() -> Result<String, EnvoixError> {
    generate_code(2).map_err(op_err)
}

/// Creates the initial Activity/queue record for a transfer request.
#[uniffi::export]
pub fn make_transfer_activity_record(mut request: FfiTransferRequest) -> FfiTransferActivityRecord {
    if request.activity_id.trim().is_empty() {
        request.activity_id = next_activity_id();
    }
    FfiTransferActivityRecord::from_request(&request, now_ms())
}

/// Folds one lifecycle event into an Activity/queue record.
#[uniffi::export]
pub fn fold_transfer_activity(
    mut record: FfiTransferActivityRecord,
    event: FfiTransferEvent,
) -> FfiTransferActivityRecord {
    record.apply_event(&event);
    record
}

/// Error surfaced across the FFI boundary.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EnvoixError {
    /// An operation failed; `message` is a human-readable reason.
    #[error("{message}")]
    Operation {
        /// Human-readable failure reason.
        message: String,
    },
}

fn op_err(error: impl std::fmt::Display) -> EnvoixError {
    EnvoixError::Operation {
        message: error.to_string(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferDirection {
    Send,
    Receive,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferMode {
    Manual,
    Invite,
    ShowManual,
    ShowInvite,
    Mdns,
    Room,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPathPolicy {
    Auto,
    RelayOnly,
    DirectOnly,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferLimits {
    /// Maximum independent transfer tasks a native queue may run at once.
    pub max_parallel_transfers: u32,
    /// Reserved for directory/multi-file sends. Current engine supports one file.
    pub max_parallel_files: u32,
    /// Reserved for future chunk-level parallelism. Current engine supports one chunk stream.
    pub max_parallel_chunks_per_file: u32,
    /// Advisory speed cap in bytes/s. Zero means unlimited; current engine does not enforce it.
    pub speed_limit_bps: u64,
}

impl Default for FfiTransferLimits {
    fn default() -> Self {
        Self {
            max_parallel_transfers: 1,
            max_parallel_files: 1,
            max_parallel_chunks_per_file: 1,
            speed_limit_bps: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferRequest {
    /// Native-side activity id used to correlate pre-start events in a queue.
    pub activity_id: String,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub file_path: String,
    pub output_dir: String,
    pub peer_descriptor: String,
    pub invite: String,
    pub code: String,
    pub token: String,
    pub broker: String,
    pub relay: String,
    pub config_path: String,
    pub path_policy: FfiPathPolicy,
    pub resume: bool,
    pub limits: FfiTransferLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferEventKind {
    Binding,
    Advertised,
    Pairing,
    Connecting,
    Connected,
    PathChanged,
    Started,
    Progress,
    Verifying,
    Verified,
    Completed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPairingStep {
    None,
    Joining,
    Matched,
    Exchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiDataPathKind {
    None,
    Direct,
    Relay,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferEvent {
    pub activity_id: String,
    pub kind: FfiTransferEventKind,
    pub ts_ms: u64,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub transfer_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub bytes_resumed: u64,
    pub pairing_step: FfiPairingStep,
    pub data_path_kind: FfiDataPathKind,
    pub data_path_detail: String,
    pub invite: String,
    pub token: String,
    pub peer_descriptor: String,
    pub diagnostic_message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferActivityState {
    Queued,
    Binding,
    WaitingForPeer,
    Pairing,
    Connecting,
    Transferring,
    Verifying,
    Completed,
    Failed,
    Canceled,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityRecord {
    pub activity_id: String,
    pub state: FfiTransferActivityState,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub transfer_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub bytes_resumed: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub data_path_kind: FfiDataPathKind,
    pub data_path_detail: String,
    pub invite: String,
    pub token: String,
    pub peer_descriptor: String,
    pub diagnostic_message: String,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub limits: FfiTransferLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCode {
    UserCanceled,
    PeerCanceled,
    NetworkLost,
    PeerUnreachable,
    AuthenticationFailed,
    PermissionDenied,
    DiskFull,
    HashMismatch,
    ProtocolError,
    DestinationConflict,
    UnsupportedFeature,
    Timeout,
    InternalError,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Protocol,
    Unsupported,
    Internal,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureOrigin {
    Local,
    Peer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailurePhase {
    Setup,
    Binding,
    Advertising,
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
    Acknowledging,
    CleaningUp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    UpdateApp,
    SwitchPairingMethod,
    DiscardPartial,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferFailure {
    pub code: FfiFailureCode,
    pub category: FfiFailureCategory,
    pub phase: FfiFailurePhase,
    pub origin: FfiFailureOrigin,
    pub direction: FfiTransferDirection,
    pub transfer_id: String,
    pub attempt_id: String,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub user_message_key: String,
    pub diagnostic_message: String,
}

/// Observer implemented by the native UI to receive transfer updates.
///
/// Callbacks arrive on a Rust runtime thread; the UI must marshal to its main
/// thread before mutating UI state. Exactly one of [`on_completed`] /
/// [`on_failed`] fires per operation.
///
/// [`on_completed`]: TransferObserver::on_completed
/// [`on_failed`]: TransferObserver::on_failed
#[uniffi::export(with_foreign)]
pub trait TransferObserver: Send + Sync {
    /// Receiver only: the `envoix:…` invite string to render as a QR / share.
    fn on_invite_ready(&self, invite: String);
    /// A transfer started; `total_bytes` is the full file size.
    fn on_started(&self, file_name: String, total_bytes: u64);
    /// Progress update: `transferred` of `total` plaintext bytes.
    fn on_progress(&self, transferred: u64, total: u64);
    /// Terminal success: the transfer finished and was verified.
    fn on_completed(&self, bytes: u64);
    /// Terminal failure with machine-readable classification.
    fn on_transfer_failed(&self, failure: FfiTransferFailure);
    /// Terminal failure with a human-readable reason.
    fn on_failed(&self, reason: String);
    /// Structured lifecycle event for Activity, queues, and diagnostics.
    fn on_transfer_event(&self, event: FfiTransferEvent);
    /// Folded Activity/queue snapshot after each lifecycle event.
    fn on_transfer_activity(&self, record: FfiTransferActivityRecord);
    /// Free-form lifecycle/status text for display or logging.
    fn on_status(&self, message: String);
}

/// A send/receive session driving the envoix core off its own runtime.
#[derive(uniffi::Object)]
pub struct EnvoixSession {
    runtime: Runtime,
    cancels: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    settings: EnvoixRuntimeSettings,
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        Arc::new(Self {
            runtime,
            cancels: Arc::new(Mutex::new(HashMap::new())),
            settings,
        })
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
        let client = build_client_for_request(&self.settings, &request)?;
        let options = transfer_options_for_request(&self.settings, &request)?;
        let source = peer_source_for_request(&self.settings, &request)?;
        let activity = FfiTransferActivityRecord::from_request(&request, now_ms());
        observer.on_transfer_activity(activity.clone());
        let _guard = self.runtime.enter();
        let transfer = match request.direction {
            FfiTransferDirection::Send => client
                .send(
                    required_path(&request.file_path, "file_path")?.into(),
                    source,
                    options,
                )
                .map_err(op_err)?,
            FfiTransferDirection::Receive => client
                .receive(
                    required_path(&request.output_dir, "output_dir")?.into(),
                    source,
                    options,
                )
                .map_err(op_err)?,
            FfiTransferDirection::Unknown => {
                return Err(EnvoixError::Operation {
                    message: "transfer direction must be send or receive".to_string(),
                });
            }
        };
        self.spawn_transfer(request.activity_id, transfer, activity, observer);
        Ok(())
    }

    /// Requests cancellation of one queued/running activity.
    pub fn cancel_activity(&self, activity_id: String) -> bool {
        if let Some(cancel) = self.cancels.lock().unwrap().remove(activity_id.trim()) {
            let _ = cancel.send(());
            true
        } else {
            false
        }
    }

    /// Requests cancellation of the in-flight transfer, if any.
    pub fn cancel(&self) {
        let cancels = std::mem::take(&mut *self.cancels.lock().unwrap());
        for (_, cancel) in cancels {
            let _ = cancel.send(());
        }
    }
}

impl EnvoixSession {
    /// Installs a fresh cancel signal for a new operation and returns its receiver.
    fn replace_cancel(&self, activity_id: &str) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        self.cancels
            .lock()
            .unwrap()
            .insert(activity_id.to_string(), sender);
        receiver
    }

    fn spawn_transfer(
        &self,
        activity_id: String,
        transfer: Transfer,
        activity: FfiTransferActivityRecord,
        observer: Arc<dyn TransferObserver>,
    ) {
        let cancel = self.replace_cancel(&activity_id);
        let cancels = self.cancels.clone();
        self.runtime.spawn(drive_transfer(
            activity_id,
            transfer,
            activity,
            observer,
            cancel,
            cancels,
        ));
    }
}

impl FfiTransferRequest {
    fn send(file_path: String, mode: FfiTransferMode) -> Self {
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
            limits: FfiTransferLimits::default(),
        }
    }

    fn receive(output_dir: String, mode: FfiTransferMode) -> Self {
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
            limits: FfiTransferLimits::default(),
        }
    }
}

impl FfiTransferActivityRecord {
    fn from_request(request: &FfiTransferRequest, now_ms: u64) -> Self {
        Self {
            activity_id: request.activity_id.clone(),
            state: FfiTransferActivityState::Queued,
            direction: request.direction,
            mode: request.mode,
            transfer_id: String::new(),
            file_name: request_file_name(request),
            total_bytes: 0,
            bytes_transferred: 0,
            bytes_resumed: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            started_at_ms: 0,
            completed_at_ms: 0,
            data_path_kind: FfiDataPathKind::None,
            data_path_detail: String::new(),
            invite: request.invite.clone(),
            token: request.token.clone(),
            peer_descriptor: request.peer_descriptor.clone(),
            diagnostic_message: String::new(),
            retryable: false,
            recovery_action: FfiRecoveryAction::None,
            limits: request.limits.clone(),
        }
    }

    fn apply_event(&mut self, event: &FfiTransferEvent) {
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

        self.state = match event.kind {
            FfiTransferEventKind::Binding => FfiTransferActivityState::Binding,
            FfiTransferEventKind::Advertised => FfiTransferActivityState::WaitingForPeer,
            FfiTransferEventKind::Pairing => FfiTransferActivityState::Pairing,
            FfiTransferEventKind::Connecting
            | FfiTransferEventKind::Connected
            | FfiTransferEventKind::PathChanged => FfiTransferActivityState::Connecting,
            FfiTransferEventKind::Started | FfiTransferEventKind::Progress => {
                if self.started_at_ms == 0 {
                    self.started_at_ms = event.ts_ms;
                }
                FfiTransferActivityState::Transferring
            }
            FfiTransferEventKind::Verifying | FfiTransferEventKind::Verified => {
                FfiTransferActivityState::Verifying
            }
            FfiTransferEventKind::Completed => {
                self.completed_at_ms = event.ts_ms;
                FfiTransferActivityState::Completed
            }
            FfiTransferEventKind::Failed => {
                self.completed_at_ms = event.ts_ms;
                FfiTransferActivityState::Failed
            }
            FfiTransferEventKind::Unknown => self.state,
        };
    }

    fn apply_failure(&mut self, failure: &FfiTransferFailure, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Failed;
        if failure.direction != FfiTransferDirection::Unknown {
            self.direction = failure.direction;
        }
        if !failure.transfer_id.is_empty() {
            self.transfer_id = failure.transfer_id.clone();
        }
        self.diagnostic_message = failure.diagnostic_message.clone();
        self.retryable = failure.retryable;
        self.recovery_action = failure.recovery_action;
    }

    fn apply_completed(&mut self, summary: &TransferSummary, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Completed;
        self.bytes_transferred = summary.bytes_transferred;
        self.total_bytes = self.total_bytes.max(summary.bytes_transferred);
    }

    fn apply_canceled(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Canceled;
        self.diagnostic_message = "canceled".to_string();
        self.retryable = true;
        self.recovery_action = FfiRecoveryAction::Resume;
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

fn next_activity_id() -> String {
    let id = NEXT_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    format!("ffi-{id}")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn build_client_for_request(
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
        message: error.to_string(),
    })
}

fn transfer_options_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<TransferOptions, EnvoixError> {
    let mut options = TransferOptions::default();
    options.relay = relay_url_for_request(settings, request);
    options.path = path_policy(request.path_policy);
    options.resume = request.resume;
    options.listen_addrs = Some(receive_addrs());
    Ok(options)
}

fn receive_addrs() -> BindAddrs {
    BindAddrs::dual_stack(0)
}

fn peer_source_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<PeerSource, EnvoixError> {
    match request.mode {
        FfiTransferMode::Manual => Ok(PeerSource::Manual {
            peer: required_peer_descriptor(&request.peer_descriptor)?,
            token: required_value(&request.token, "token")?,
        }),
        FfiTransferMode::Invite => Ok(PeerSource::Invite {
            invite: required_value(&request.invite, "invite")?,
        }),
        FfiTransferMode::ShowManual => Ok(PeerSource::ShowManual {
            token: optional_value(&request.token),
        }),
        FfiTransferMode::ShowInvite => Ok(PeerSource::ShowInvite {
            ttl_secs: INVITE_TTL_SECS,
        }),
        FfiTransferMode::Mdns => Ok(PeerSource::Mdns {
            token: optional_value(&request.token),
        }),
        FfiTransferMode::Room => Ok(PeerSource::Room {
            code: required_value(&request.code, "code")?,
            broker: rendezvous_broker_for_request(settings, request),
        }),
        FfiTransferMode::Unknown => Err(EnvoixError::Operation {
            message: "transfer mode must not be unknown".to_string(),
        }),
    }
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

fn required_path(value: &str, field: &str) -> Result<String, EnvoixError> {
    required_value(value, field)
}

fn required_value(value: &str, field: &str) -> Result<String, EnvoixError> {
    let value = value.trim();
    if value.is_empty() {
        Err(EnvoixError::Operation {
            message: format!("{field} must not be empty"),
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

fn required_peer_descriptor(value: &str) -> Result<PeerDescriptor, EnvoixError> {
    let value = required_value(value, "peer_descriptor")?;
    PeerDescriptor::parse_compact(&value).map_err(op_err)
}

/// Reports the single terminal outcome from the awaited operation result.
fn report_terminal(
    observer: &dyn TransferObserver,
    activity: &mut FfiTransferActivityRecord,
    result: Result<TransferSummary, TransferError>,
    direction: Option<TransferDirection>,
    cancel_requested: bool,
) {
    match result {
        Ok(summary) => {
            activity.apply_completed(&summary, now_ms());
            observer.on_transfer_activity(activity.clone());
            observer.on_completed(summary.bytes_transferred);
        }
        Err(error) => {
            let failure = to_ffi_failure(&error, direction);
            if cancel_requested {
                activity.apply_canceled(now_ms());
            } else {
                activity.apply_failure(&failure, now_ms());
            }
            observer.on_transfer_activity(activity.clone());
            observer.on_transfer_failed(failure);
            observer.on_failed(error.to_string());
        }
    }
}

async fn drive_transfer(
    activity_id: String,
    mut transfer: Transfer,
    mut activity: FfiTransferActivityRecord,
    observer: Arc<dyn TransferObserver>,
    mut cancel: oneshot::Receiver<()>,
    cancels: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
) {
    let mut cancel_requested = false;
    let mut direction = None;
    loop {
        tokio::select! {
            event = transfer.next_event() => {
                let Some(event) = event else { break };
                if direction.is_none() {
                    direction = event_direction(&event.event);
                }
                observe_transfer_event(&*observer, &mut activity, event);
            }
            _ = &mut cancel, if !cancel_requested => {
                cancel_requested = true;
                transfer.cancel();
                observer.on_status("cancelling".to_string());
            }
        }
    }
    report_terminal(
        &*observer,
        &mut activity,
        transfer.wait().await,
        direction,
        cancel_requested,
    );
    cancels.lock().unwrap().remove(&activity_id);
}

fn event_direction(event: &TransferEvent) -> Option<TransferDirection> {
    match event {
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
) {
    let ffi_event = to_ffi_event(&event, &activity.activity_id);
    activity.apply_event(&ffi_event);
    observer.on_transfer_event(ffi_event);
    observer.on_transfer_activity(activity.clone());
    let event = event.event;
    match event {
        TransferEvent::Binding { direction, mode } => {
            observer.on_status(format!("binding {direction:?} via {mode:?}"));
        }
        TransferEvent::Advertised { invite, .. } => {
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
        TransferEvent::Completed { .. } | TransferEvent::Failed { .. } => {}
        _ => {}
    }
}

fn to_ffi_event(event: &StampedEvent, activity_id: &str) -> FfiTransferEvent {
    let mut ffi = FfiTransferEvent::empty(activity_id, event.ts_ms);
    match &event.event {
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
        }
        TransferEvent::Completed {
            transfer_id,
            bytes_transferred,
        } => {
            ffi.kind = FfiTransferEventKind::Completed;
            ffi.transfer_id = transfer_id.to_string();
            ffi.bytes_transferred = *bytes_transferred;
            ffi.total_bytes = *bytes_transferred;
        }
        TransferEvent::Failed { direction, reason } => {
            ffi.kind = FfiTransferEventKind::Failed;
            ffi.direction = ffi_direction(Some(*direction));
            ffi.diagnostic_message = reason.clone();
        }
        _ => {}
    }
    ffi
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

fn pairing_step_label(step: PairingStep) -> &'static str {
    match step {
        PairingStep::Joining => "joining room",
        PairingStep::Matched => "peer matched",
        PairingStep::Exchanged => "keys exchanged",
    }
}

fn to_ffi_failure(
    error: &TransferError,
    direction: Option<TransferDirection>,
) -> FfiTransferFailure {
    let failure = error.to_failure(direction);
    FfiTransferFailure {
        code: ffi_failure_code(failure.code),
        category: ffi_failure_category(failure.category),
        phase: ffi_failure_phase(failure.phase),
        origin: ffi_failure_origin(failure.origin),
        direction: ffi_direction(failure.direction),
        transfer_id: failure.transfer_id.unwrap_or_default(),
        attempt_id: failure.attempt_id.unwrap_or_default(),
        retryable: failure.retryable,
        recovery_action: ffi_recovery_action(failure.recovery_action),
        user_message_key: failure.user_message_key,
        diagnostic_message: failure.diagnostic_message,
    }
}

fn ffi_direction(direction: Option<TransferDirection>) -> FfiTransferDirection {
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
mod tests {
    use super::*;
    use envoix_qr::QrInvitePayload;
    use envoix_rendezvous::RoomRegistry;
    use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
    use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::mpsc::{Sender, channel};
    use std::thread;
    use std::time::Duration;

    enum Msg {
        Invite(String),
        Completed(u64),
        Failed(String),
        Event(FfiTransferEvent),
        Activity(FfiTransferActivityRecord),
    }

    async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
        for _ in 0..100 {
            if ep.addr().ip_addrs().next().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        endpoint_addr(ep)
    }

    struct TestObserver(Sender<Msg>);

    impl TransferObserver for TestObserver {
        fn on_invite_ready(&self, invite: String) {
            let _ = self.0.send(Msg::Invite(invite));
        }
        fn on_started(&self, _file_name: String, _total_bytes: u64) {}
        fn on_progress(&self, _transferred: u64, _total: u64) {}
        fn on_completed(&self, bytes: u64) {
            let _ = self.0.send(Msg::Completed(bytes));
        }
        fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}
        fn on_failed(&self, reason: String) {
            let _ = self.0.send(Msg::Failed(reason));
        }
        fn on_transfer_event(&self, event: FfiTransferEvent) {
            let _ = self.0.send(Msg::Event(event));
        }
        fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
            let _ = self.0.send(Msg::Activity(record));
        }
        fn on_status(&self, _message: String) {}
    }

    fn recv_invite(rx: &std::sync::mpsc::Receiver<Msg>, timeout: Duration) -> String {
        loop {
            match rx.recv_timeout(timeout).unwrap() {
                Msg::Invite(invite) => return invite,
                Msg::Failed(reason) => panic!("transfer failed before invite: {reason}"),
                Msg::Completed(_) => panic!("transfer completed before invite"),
                Msg::Event(_) => {}
                Msg::Activity(_) => {}
            }
        }
    }

    fn recv_completed(
        rx: &std::sync::mpsc::Receiver<Msg>,
        timeout: Duration,
    ) -> (u64, Vec<FfiTransferEvent>) {
        let mut events = Vec::new();
        loop {
            match rx.recv_timeout(timeout).unwrap() {
                Msg::Completed(bytes) => return (bytes, events),
                Msg::Failed(reason) => panic!("transfer failed: {reason}"),
                Msg::Invite(_) => {}
                Msg::Event(event) => events.push(event),
                Msg::Activity(record) => {
                    if record.state == FfiTransferActivityState::Failed {
                        panic!("transfer failed: {}", record.diagnostic_message);
                    }
                }
            }
        }
    }

    #[test]
    fn activity_record_folds_transfer_events() {
        let request = FfiTransferRequest {
            activity_id: "activity-1".to_string(),
            direction: FfiTransferDirection::Send,
            mode: FfiTransferMode::Room,
            file_path: "/tmp/report.pdf".to_string(),
            output_dir: String::new(),
            peer_descriptor: String::new(),
            invite: String::new(),
            code: "135790-amber-comet".to_string(),
            token: String::new(),
            broker: String::new(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            resume: true,
            limits: FfiTransferLimits {
                max_parallel_transfers: 2,
                ..FfiTransferLimits::default()
            },
        };
        let mut record = make_transfer_activity_record(request);
        assert_eq!(record.activity_id, "activity-1");
        assert_eq!(record.state, FfiTransferActivityState::Queued);
        assert_eq!(record.file_name, "report.pdf");
        assert_eq!(record.limits.max_parallel_transfers, 2);

        let mut started = FfiTransferEvent::empty("activity-1", 10);
        started.kind = FfiTransferEventKind::Started;
        started.direction = FfiTransferDirection::Send;
        started.transfer_id = "tx1".to_string();
        started.file_name = "report.pdf".to_string();
        started.total_bytes = 100;
        record = fold_transfer_activity(record, started);
        assert_eq!(record.state, FfiTransferActivityState::Transferring);
        assert_eq!(record.started_at_ms, 10);
        assert_eq!(record.transfer_id, "tx1");

        let mut completed = FfiTransferEvent::empty("activity-1", 20);
        completed.kind = FfiTransferEventKind::Completed;
        completed.transfer_id = "tx1".to_string();
        completed.bytes_transferred = 100;
        record = fold_transfer_activity(record, completed);
        assert_eq!(record.state, FfiTransferActivityState::Completed);
        assert_eq!(record.bytes_transferred, 100);
        assert_eq!(record.completed_at_ms, 20);
    }

    /// Rewrites an invite's direct addresses to loopback, keeping the port, so
    /// the transfer stays on the local machine (mirrors the CLI loopback test).
    fn loopback_invite(invite: &str) -> String {
        let mut payload = QrInvitePayload::decode(invite).unwrap();
        let port = payload.peer.direct_addrs[0].port();
        payload.peer.direct_addrs = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)];
        payload.encode()
    }

    #[test]
    fn ffi_qr_invite_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("hello.txt");
        let text = b"hello from the ffi bridge";
        std::fs::write(&source, text).unwrap();

        let receiver = EnvoixSession::new();
        let (rtx, rrx) = channel();
        receiver
            .receive(
                output_dir.to_str().unwrap().to_string(),
                Arc::new(TestObserver(rtx)),
            )
            .unwrap();

        let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));

        // Let the receiver's accept loop start before dialing.
        std::thread::sleep(Duration::from_millis(300));

        let sender = EnvoixSession::new();
        let (stx, srx) = channel();
        sender
            .send_invite(
                invite,
                source.to_str().unwrap().to_string(),
                Arc::new(TestObserver(stx)),
            )
            .unwrap();

        let (_, sender_events) = recv_completed(&srx, Duration::from_secs(15));
        let (bytes, receiver_events) = recv_completed(&rrx, Duration::from_secs(15));

        assert_eq!(bytes, text.len() as u64);
        assert_eq!(std::fs::read(output_dir.join("hello.txt")).unwrap(), text);
        assert!(
            sender_events
                .iter()
                .any(|event| event.kind == FfiTransferEventKind::Binding
                    && event.direction == FfiTransferDirection::Send
                    && event.mode == FfiTransferMode::Invite)
        );
        assert!(
            receiver_events
                .iter()
                .any(|event| event.kind == FfiTransferEventKind::Started
                    && event.direction == FfiTransferDirection::Receive
                    && event.file_name == "hello.txt")
        );
    }

    #[test]
    fn ffi_room_loopback() {
        let (broker_tx, broker_rx) = channel();
        let _server = thread::spawn(move || {
            let runtime = Runtime::new().unwrap();
            runtime.block_on(async move {
                let server = build_endpoint(
                    "127.0.0.1:0".parse().unwrap(),
                    SecretKey::generate(),
                    RelayMode::Disabled,
                )
                .await
                .unwrap();
                let server_id = server.id();
                let server_addr = *ready_addr(&server)
                    .await
                    .ip_addrs()
                    .next()
                    .expect("server should have a direct address");
                let broker = format!("{server_id}@{server_addr}");
                broker_tx.send(broker).unwrap();
                serve_endpoint(server, Arc::new(RoomRegistry::new()), None)
                    .await
                    .unwrap();
            });
        });
        let broker = broker_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("room.txt");
        let text = b"hello from ffi room";
        std::fs::write(&source, text).unwrap();

        let settings = EnvoixRuntimeSettings {
            server_url: broker,
            relay_url: String::new(),
            ..EnvoixRuntimeSettings::default()
        };
        let code = "135790-amber-comet".to_string();

        let receiver = EnvoixSession::new_with_settings(settings.clone());
        let (rtx, rrx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_str().unwrap().to_string(),
            FfiTransferMode::Room,
        );
        receive_request.code = code.clone();
        receiver
            .start_transfer(receive_request, Arc::new(TestObserver(rtx)))
            .unwrap();

        thread::sleep(Duration::from_millis(200));

        let sender = EnvoixSession::new_with_settings(settings);
        let (stx, srx) = channel();
        let mut send_request =
            FfiTransferRequest::send(source.to_str().unwrap().to_string(), FfiTransferMode::Room);
        send_request.code = code;
        sender
            .start_transfer(send_request, Arc::new(TestObserver(stx)))
            .unwrap();

        let (_, sender_events) = recv_completed(&srx, Duration::from_secs(20));
        let (bytes, receiver_events) = recv_completed(&rrx, Duration::from_secs(20));

        assert_eq!(bytes, text.len() as u64);
        assert_eq!(std::fs::read(output_dir.join("room.txt")).unwrap(), text);
        assert!(
            sender_events
                .iter()
                .any(|event| event.kind == FfiTransferEventKind::Pairing
                    && event.pairing_step == FfiPairingStep::Exchanged)
        );
        assert!(
            receiver_events
                .iter()
                .any(|event| event.kind == FfiTransferEventKind::Started
                    && event.file_name == "room.txt")
        );
    }
}
