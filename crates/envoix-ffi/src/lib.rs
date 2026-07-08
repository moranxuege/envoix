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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use envoix_client::{
    BindAddrs, PeerDescriptor, TransferDirection, TransferSummary,
    api::{
        Client, DataPath, FailureCategory, FailureCode, FailureOrigin, FailurePhase, Invite,
        PairingStep, PathPolicy, PeerSource, RecoveryAction, Role, StampedEvent, Transfer,
        TransferError, TransferEvent, TransferMode, TransferOptions,
    },
};
use envoix_rendezvous_iroh::generate_code;
use tokio::runtime::{Handle, Runtime};
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
static NEXT_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);
const TRANSFER_ACTIVITY_HISTORY_CAP: usize = 50;
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

/// Creates a cross-platform room invite whose payload is safe to render as a QR.
#[uniffi::export]
pub fn make_pairing_invite(
    role: FfiInviteRole,
    broker: String,
    relay: String,
) -> Result<FfiPairingInvite, EnvoixError> {
    let broker_input = broker.trim();
    let relay = relay_for_pairing_invite(broker_input, &relay);
    let broker = broker_for_pairing_invite(broker_input);
    let mut invite = Invite::room(broker, relay).map_err(op_err)?;
    if let Some(role) = core_invite_role(role) {
        invite = invite.with_role(role);
    }
    Ok(FfiPairingInvite::from_invite(&invite))
}

/// Parses a typed room code or scanned `envoix://pair/...` payload.
#[uniffi::export]
pub fn parse_pairing_invite(input: String) -> Result<FfiPairingInvite, EnvoixError> {
    let input = input.trim();
    let lowercased = input.to_ascii_lowercase();
    if lowercased.starts_with("envoix:") && !lowercased.starts_with("envoix://pair/") {
        return Err(EnvoixError::Operation {
            message: "unsupported Envoix pairing invite scheme".to_string(),
        });
    }
    let invite = Invite::parse(input).map_err(op_err)?;
    Ok(FfiPairingInvite::from_invite(&invite))
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
pub enum FfiInviteRole {
    Send,
    Receive,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPairingInvite {
    /// Short pairing code typed by users and reused as the mDNS token.
    pub code: String,
    /// `envoix://pair/...` payload rendered into the QR code.
    pub payload: String,
    /// Broker advertised by the QR payload, empty when the input was a bare code.
    pub broker: String,
    /// Relay advertised by the QR payload, empty when not supplied.
    pub relay: String,
    /// Role advertised by the payload creator; scanners should choose the opposite.
    pub role: FfiInviteRole,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRendezvousPlan {
    /// Try the hosted rendezvous room before any local-network fallback.
    pub use_room: bool,
    /// Reuse the room code as the mDNS token when room pairing is unavailable.
    pub use_mdns: bool,
    /// Whether the native shell currently considers broker access viable.
    pub internet_available: bool,
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

impl Default for FfiRendezvousPlan {
    fn default() -> Self {
        Self {
            use_room: true,
            use_mdns: true,
            internet_available: true,
        }
    }
}

impl FfiRendezvousPlan {
    fn for_mode(mode: FfiTransferMode) -> Self {
        match mode {
            FfiTransferMode::Room => Self::default(),
            FfiTransferMode::Mdns => Self {
                use_room: false,
                use_mdns: true,
                internet_available: true,
            },
            _ => Self {
                use_room: false,
                use_mdns: false,
                internet_available: true,
            },
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
    pub rendezvous: FfiRendezvousPlan,
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
    Paused,
    Canceled,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityRecord {
    pub activity_id: String,
    pub attempt_id: String,
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
    pub completed_file_path: String,
    pub data_path_kind: FfiDataPathKind,
    pub data_path_detail: String,
    pub invite: String,
    pub token: String,
    pub peer_descriptor: String,
    pub diagnostic_message: String,
    pub failure_code: FfiFailureCode,
    pub failure_category: FfiFailureCategory,
    pub failure_phase: FfiFailurePhase,
    pub failure_origin: FfiFailureOrigin,
    pub user_message_key: String,
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
    queue: Arc<Mutex<TransferQueueState>>,
    settings: EnvoixRuntimeSettings,
}

#[derive(Default)]
struct TransferQueueState {
    pending: VecDeque<QueuedTransfer>,
    active: HashMap<String, ActiveTransfer>,
    paused: HashMap<String, QueuedTransfer>,
    history: VecDeque<FfiTransferActivityRecord>,
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
}

struct TransferAttemptOutcome {
    result: Result<TransferSummary, TransferError>,
    direction: Option<TransferDirection>,
    connected: bool,
    stop_requested: Option<TransferStop>,
}

struct ActiveTransfer {
    control: Option<oneshot::Sender<TransferStop>>,
    limit: usize,
    activity: FfiTransferActivityRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferStop {
    Cancel,
    Pause,
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
        records.sort_by(|lhs, rhs| rhs.updated_at_ms.cmp(&lhs.updated_at_ms));
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
        let (stored, control, removed_history) = {
            let mut queue = self.queue.lock().unwrap();
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
            let before = queue.history.len();
            queue
                .history
                .retain(|record| record.activity_id != activity_id);
            (pending.or(paused), control, queue.history.len() != before)
        };
        let removed_stored = stored.is_some();
        let removed_active = control.is_some();
        if let Some(job) = stored {
            report_queued_cancel(job);
        }
        if let Some(control) = control {
            let _ = control.send(TransferStop::Cancel);
        }
        removed_stored || removed_active || removed_history
    }

    /// Clears terminal transfer history while preserving active, pending, and paused items.
    pub fn clear_transfer_history(&self) -> u32 {
        let mut queue = self.queue.lock().unwrap();
        let removed = queue.history.len();
        queue.history.clear();
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
                    message: format!("activity_id already exists: {}", request.activity_id),
                });
            }
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

        let control = self
            .queue
            .lock()
            .unwrap()
            .active
            .get_mut(&activity_id)
            .and_then(|active| active.control.take());
        if let Some(control) = control {
            let _ = control.send(TransferStop::Cancel);
            true
        } else {
            false
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

        let control = self
            .queue
            .lock()
            .unwrap()
            .active
            .get_mut(&activity_id)
            .and_then(|active| active.control.take());
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
            let controls = queue
                .active
                .values_mut()
                .filter_map(|active| active.control.take())
                .collect::<Vec<_>>();
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
            finish_transfer_activity(&activity_id, &queue_for_task);
            if let Some(job) = paused_job {
                let observer = job.observer.clone();
                let activity = job.activity.clone();
                queue_for_task
                    .lock()
                    .unwrap()
                    .paused
                    .insert(activity_id.clone(), job);
                observer.on_transfer_activity(activity);
                observer.on_status("paused".to_string());
            }
            drain_transfer_queue(queue_for_task, settings_for_task, handle_for_task);
        });
    }
}

fn finish_transfer_activity(activity_id: &str, queue: &Arc<Mutex<TransferQueueState>>) {
    queue.lock().unwrap().active.remove(activity_id);
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
    message: String,
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
        diagnostic_message: message,
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
                message: "transfer direction must be send or receive".to_string(),
            });
        }
    }
    build_client_for_request(settings, request)?;
    transfer_options_for_request(settings, request)?;
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
            message: "transfer direction must be send or receive".to_string(),
        }),
        (_, FfiTransferMode::Unknown) => Err(EnvoixError::Operation {
            message: "transfer mode must not be unknown".to_string(),
        }),
        (direction, mode) => Err(EnvoixError::Operation {
            message: format!("transfer mode {mode:?} is not supported for {direction:?}"),
        }),
    }
}

fn build_transfer_for_source(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    source: PeerSource,
    handle: &Handle,
) -> Result<Transfer, EnvoixError> {
    let client = build_client_for_request(settings, request)?;
    let options = transfer_options_for_request(settings, request)?;
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
            message: "transfer direction must be send or receive".to_string(),
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
            rendezvous: FfiRendezvousPlan::for_mode(mode),
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
            rendezvous: FfiRendezvousPlan::for_mode(mode),
        }
    }
}

impl FfiPairingInvite {
    fn from_invite(invite: &Invite) -> Self {
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
    fn from_request(request: &FfiTransferRequest, now_ms: u64) -> Self {
        Self {
            activity_id: request.activity_id.clone(),
            attempt_id: String::new(),
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
        self.failure_code = failure.code;
        self.failure_category = failure.category;
        self.failure_phase = failure.phase;
        self.failure_origin = failure.origin;
        self.user_message_key = failure.user_message_key.clone();
        self.retryable = failure.retryable;
        self.recovery_action = failure.recovery_action;
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

    fn apply_canceled(&mut self, ts_ms: u64) {
        self.updated_at_ms = ts_ms;
        self.completed_at_ms = ts_ms;
        self.state = FfiTransferActivityState::Canceled;
        self.diagnostic_message = "canceled".to_string();
        self.failure_code = FfiFailureCode::UserCanceled;
        self.failure_category = FfiFailureCategory::User;
        self.failure_phase = FfiFailurePhase::Transferring;
        self.failure_origin = FfiFailureOrigin::Local;
        self.user_message_key = "transfer.user_canceled".to_string();
        self.retryable = true;
        self.recovery_action = FfiRecoveryAction::Resume;
    }

    fn apply_paused(&mut self, ts_ms: u64) {
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

    fn apply_requeued(&mut self, ts_ms: u64) {
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

fn next_activity_id() -> String {
    let id = NEXT_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    format!("ffi-{id}")
}

fn next_attempt_id() -> String {
    let id = NEXT_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed);
    format!("attempt-{id}")
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
    if request.path_policy == FfiPathPolicy::RelayOnly && options.relay.is_none() {
        return Err(EnvoixError::Operation {
            message: "relay-only transfers require a relay URL".to_string(),
        });
    }
    options.path = path_policy(request.path_policy);
    options.resume = request.resume;
    options.listen_addrs = Some(receive_addrs());
    Ok(options)
}

fn receive_addrs() -> BindAddrs {
    BindAddrs::dual_stack(0)
}

fn peer_sources_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Vec<TransferAttemptSource>, EnvoixError> {
    let single = |mode, source| Ok(vec![TransferAttemptSource { mode, source }]);
    match request.mode {
        FfiTransferMode::Manual => single(
            FfiTransferMode::Manual,
            PeerSource::Manual {
                peer: required_peer_descriptor(&request.peer_descriptor)?,
                token: required_value(&request.token, "token")?,
            },
        ),
        FfiTransferMode::Invite => single(
            FfiTransferMode::Invite,
            PeerSource::Invite {
                invite: required_value(&request.invite, "invite")?,
            },
        ),
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
                sources.push(TransferAttemptSource {
                    mode: FfiTransferMode::Room,
                    source: PeerSource::Room {
                        code: code.clone(),
                        broker: rendezvous_broker_for_request(settings, request),
                    },
                });
            }
            if request.rendezvous.use_mdns {
                sources.push(TransferAttemptSource {
                    mode: FfiTransferMode::Mdns,
                    source: PeerSource::Mdns { token: Some(code) },
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
                    message: message.to_string(),
                })
            } else {
                Ok(sources)
            }
        }
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

fn broker_for_pairing_invite(broker: &str) -> String {
    let broker = broker.trim();
    if broker.is_empty() {
        DEFAULT_RENDEZVOUS_BROKER.to_string()
    } else {
        broker.to_string()
    }
}

fn relay_for_pairing_invite(broker: &str, relay: &str) -> Option<String> {
    let relay = relay.trim();
    if !relay.is_empty() {
        Some(relay.to_string())
    } else if broker.trim().is_empty() {
        Some(DEFAULT_RELAY_URL.to_string())
    } else {
        None
    }
}

fn core_invite_role(role: FfiInviteRole) -> Option<Role> {
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
            let failure = to_ffi_failure(&error, direction, activity);
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

async fn drive_transfer_request(
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
    let mut stop_requested = None;
    for (index, attempt) in attempts.into_iter().enumerate() {
        if index > 0 {
            activity.attempt_id = next_attempt_id();
            activity.updated_at_ms = now_ms();
            store_active_activity(&queue, &activity);
            observer.on_transfer_activity(activity.clone());
        }
        let transfer = match build_transfer_for_source(&settings, &request, attempt.source, &handle)
        {
            Ok(transfer) => transfer,
            Err(error) => {
                if let Some(next_mode) = modes.get(index + 1) {
                    observer.on_status(format!(
                        "{} setup failed; trying {}",
                        transfer_mode_label(attempt.mode),
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

        let outcome = drive_transfer_attempt(
            transfer,
            &mut activity,
            &*observer,
            &mut control,
            &mut stop_requested,
            &queue,
        )
        .await;
        let can_fallback = outcome.stop_requested.is_none()
            && !outcome.connected
            && modes.get(index + 1).is_some();
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
                observer.on_transfer_activity(activity.clone());
                observer.on_status("paused".to_string());
                return Some(QueuedTransfer {
                    request,
                    observer,
                    activity,
                });
            }
            Err(error) if can_fallback => {
                let next_mode = modes[index + 1];
                observer.on_status(format!(
                    "{} failed before connection ({}); trying {}",
                    transfer_mode_label(attempt.mode),
                    error,
                    transfer_mode_label(next_mode)
                ));
                continue;
            }
            Err(error) => {
                let canceled = outcome.stop_requested == Some(TransferStop::Cancel);
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

async fn drive_transfer_attempt(
    mut transfer: Transfer,
    activity: &mut FfiTransferActivityRecord,
    observer: &dyn TransferObserver,
    control: &mut oneshot::Receiver<TransferStop>,
    stop_requested: &mut Option<TransferStop>,
    queue: &Arc<Mutex<TransferQueueState>>,
) -> TransferAttemptOutcome {
    let mut direction = None;
    let mut connected = false;
    loop {
        tokio::select! {
            event = transfer.next_event() => {
                let Some(event) = event else { break };
                if direction.is_none() {
                    direction = event_direction(&event.event);
                }
                if matches!(event.event, TransferEvent::Connected { .. }) {
                    connected = true;
                }
                observe_transfer_event(observer, activity, event);
                store_active_activity(queue, activity);
            }
            stop = &mut *control, if stop_requested.is_none() => {
                let stop = stop.unwrap_or(TransferStop::Cancel);
                *stop_requested = Some(stop);
                transfer.cancel();
                observer.on_status(match stop {
                    TransferStop::Cancel => "cancelling".to_string(),
                    TransferStop::Pause => "pausing".to_string(),
                });
            }
        }
    }
    TransferAttemptOutcome {
        result: transfer.wait().await,
        direction,
        connected,
        stop_requested: *stop_requested,
    }
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

    fn recv_completed_activity(
        rx: &std::sync::mpsc::Receiver<Msg>,
        timeout: Duration,
    ) -> (u64, Vec<FfiTransferEvent>, FfiTransferActivityRecord) {
        let mut events = Vec::new();
        let mut completed_activity = None;
        loop {
            match rx.recv_timeout(timeout).unwrap() {
                Msg::Completed(bytes) => {
                    return (
                        bytes,
                        events,
                        completed_activity.expect("completed activity should precede callback"),
                    );
                }
                Msg::Failed(reason) => panic!("transfer failed: {reason}"),
                Msg::Invite(_) => {}
                Msg::Event(event) => events.push(event),
                Msg::Activity(record) => match record.state {
                    FfiTransferActivityState::Completed => completed_activity = Some(record),
                    FfiTransferActivityState::Failed => {
                        panic!("transfer failed: {}", record.diagnostic_message)
                    }
                    _ => {}
                },
            }
        }
    }

    fn recv_activity(
        rx: &std::sync::mpsc::Receiver<Msg>,
        activity_id: &str,
        timeout: Duration,
    ) -> FfiTransferActivityRecord {
        loop {
            match rx.recv_timeout(timeout).unwrap() {
                Msg::Activity(record) if record.activity_id == activity_id => return record,
                Msg::Failed(reason) => panic!("transfer failed: {reason}"),
                Msg::Invite(_) | Msg::Completed(_) | Msg::Event(_) | Msg::Activity(_) => {}
            }
        }
    }

    fn assert_no_nonqueued_activity(
        rx: &std::sync::mpsc::Receiver<Msg>,
        activity_id: &str,
        timeout: Duration,
    ) {
        let deadline = std::time::Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(Msg::Activity(record))
                    if record.activity_id == activity_id
                        && record.state != FfiTransferActivityState::Queued =>
                {
                    panic!("queued activity started unexpectedly: {:?}", record.state);
                }
                Ok(Msg::Failed(reason)) => panic!("transfer failed: {reason}"),
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn snapshot_record(
        session: &EnvoixSession,
        activity_id: &str,
    ) -> Option<FfiTransferActivityRecord> {
        session
            .list_transfer_activities()
            .into_iter()
            .find(|record| record.activity_id == activity_id)
    }

    #[test]
    fn pairing_invite_payload_round_trips_for_native_clients() {
        let invite = make_pairing_invite(
            FfiInviteRole::Receive,
            "id@127.0.0.1:8445".to_string(),
            "https://relay.example".to_string(),
        )
        .unwrap();

        assert!(invite.payload.starts_with("envoix://pair/"));
        assert_eq!(invite.role, FfiInviteRole::Receive);
        assert_eq!(invite.broker, "id@127.0.0.1:8445");
        assert_eq!(invite.relay, "https://relay.example");

        let parsed = parse_pairing_invite(invite.payload).unwrap();
        assert_eq!(parsed.code, invite.code);
        assert_eq!(parsed.broker, "id@127.0.0.1:8445");
        assert_eq!(parsed.relay, "https://relay.example");
        assert_eq!(parsed.role, FfiInviteRole::Receive);
    }

    #[test]
    fn pairing_invite_uses_hosted_defaults_when_settings_are_blank() {
        let invite =
            make_pairing_invite(FfiInviteRole::Send, String::new(), String::new()).unwrap();
        assert_eq!(invite.broker, DEFAULT_RENDEZVOUS_BROKER);
        assert_eq!(invite.relay, DEFAULT_RELAY_URL);

        let parsed = parse_pairing_invite(invite.code.clone()).unwrap();
        assert_eq!(parsed.code, invite.code);
        assert!(parsed.broker.is_empty());
        assert!(parsed.relay.is_empty());
        assert_eq!(parsed.role, FfiInviteRole::Unknown);
    }

    #[test]
    fn custom_pairing_broker_does_not_force_default_relay() {
        let invite = make_pairing_invite(
            FfiInviteRole::Unknown,
            "custom@10.0.0.1:8445".to_string(),
            String::new(),
        )
        .unwrap();
        assert_eq!(invite.broker, "custom@10.0.0.1:8445");
        assert!(invite.relay.is_empty());
        assert_eq!(invite.role, FfiInviteRole::Unknown);
    }

    #[test]
    fn pairing_invite_rejects_legacy_direct_invites() {
        let err = parse_pairing_invite("envoix:legacy-direct-payload".to_string()).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported Envoix pairing invite scheme")
        );
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
            rendezvous: FfiRendezvousPlan::default(),
        };
        let mut record = make_transfer_activity_record(request);
        assert_eq!(record.activity_id, "activity-1");
        assert!(record.attempt_id.is_empty());
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

    #[test]
    fn activity_record_keeps_structured_failure_metadata() {
        let request = FfiTransferRequest {
            activity_id: "activity-fail".to_string(),
            direction: FfiTransferDirection::Receive,
            mode: FfiTransferMode::Room,
            file_path: String::new(),
            output_dir: "/tmp/envoix".to_string(),
            peer_descriptor: String::new(),
            invite: String::new(),
            code: "135790-amber-comet".to_string(),
            token: String::new(),
            broker: String::new(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            resume: true,
            limits: FfiTransferLimits::default(),
            rendezvous: FfiRendezvousPlan::default(),
        };
        let mut record = make_transfer_activity_record(request);
        let failure = FfiTransferFailure {
            code: FfiFailureCode::PermissionDenied,
            category: FfiFailureCategory::Permission,
            phase: FfiFailurePhase::Committing,
            origin: FfiFailureOrigin::Local,
            direction: FfiTransferDirection::Receive,
            transfer_id: "tx-fail".to_string(),
            attempt_id: "attempt-1".to_string(),
            retryable: true,
            recovery_action: FfiRecoveryAction::ChooseFolder,
            user_message_key: "transfer.permission_denied".to_string(),
            diagnostic_message: "permission denied opening destination folder".to_string(),
        };

        record.apply_failure(&failure, 42);

        assert_eq!(record.state, FfiTransferActivityState::Failed);
        assert_eq!(record.failure_code, FfiFailureCode::PermissionDenied);
        assert_eq!(record.failure_category, FfiFailureCategory::Permission);
        assert_eq!(record.failure_phase, FfiFailurePhase::Committing);
        assert_eq!(record.failure_origin, FfiFailureOrigin::Local);
        assert_eq!(record.user_message_key, "transfer.permission_denied");
        assert_eq!(record.recovery_action, FfiRecoveryAction::ChooseFolder);
        assert!(record.retryable);
    }

    #[test]
    fn ffi_failure_keeps_current_attempt_identity() {
        let request = FfiTransferRequest {
            activity_id: "activity-attempt".to_string(),
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
            limits: FfiTransferLimits::default(),
            rendezvous: FfiRendezvousPlan::default(),
        };
        let mut record = make_transfer_activity_record(request);
        record.attempt_id = "attempt-1".to_string();
        record.transfer_id = "tx-1".to_string();

        let error = TransferError::input("unsupported transfer mode");
        let failure = to_ffi_failure(&error, Some(TransferDirection::Send), &record);

        assert_eq!(failure.transfer_id, "tx-1");
        assert_eq!(failure.attempt_id, "attempt-1");
        assert_eq!(failure.direction, FfiTransferDirection::Send);
        assert_eq!(failure.code, FfiFailureCode::UnsupportedFeature);
    }

    #[test]
    fn runtime_settings_normalize_parallel_transfer_limit() {
        let mut limits = FfiTransferLimits {
            max_parallel_transfers: 4,
            ..FfiTransferLimits::default()
        };
        normalize_transfer_limits(
            &EnvoixRuntimeSettings {
                concurrent_transfers: false,
                ..EnvoixRuntimeSettings::default()
            },
            &mut limits,
        );
        assert_eq!(limits.max_parallel_transfers, 1);

        let mut limits = FfiTransferLimits {
            max_parallel_transfers: 4,
            ..FfiTransferLimits::default()
        };
        normalize_transfer_limits(&EnvoixRuntimeSettings::default(), &mut limits);
        assert_eq!(limits.max_parallel_transfers, 4);

        let mut limits = FfiTransferLimits {
            max_parallel_transfers: 0,
            ..FfiTransferLimits::default()
        };
        normalize_transfer_limits(&EnvoixRuntimeSettings::default(), &mut limits);
        assert_eq!(limits.max_parallel_transfers, 1);
    }

    #[test]
    fn room_rendezvous_plan_uses_room_then_mdns() {
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        request.code = "135790-amber-comet".to_string();

        let sources = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("room request should build rendezvous sources");

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].mode, FfiTransferMode::Room);
        assert_eq!(sources[1].mode, FfiTransferMode::Mdns);
        match &sources[1].source {
            PeerSource::Mdns { token } => assert_eq!(token.as_deref(), Some("135790-amber-comet")),
            other => panic!("expected mDNS fallback source, got {other:?}"),
        }
    }

    #[test]
    fn room_rendezvous_plan_skips_room_without_internet() {
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        request.code = "135790-amber-comet".to_string();
        request.rendezvous = FfiRendezvousPlan {
            use_room: true,
            use_mdns: true,
            internet_available: false,
        };

        let sources = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("mDNS fallback should remain available without internet");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].mode, FfiTransferMode::Mdns);
        match &sources[0].source {
            PeerSource::Mdns { token } => assert_eq!(token.as_deref(), Some("135790-amber-comet")),
            other => panic!("expected mDNS source, got {other:?}"),
        }
    }

    #[test]
    fn room_rendezvous_plan_rejects_disabled_routes() {
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        request.code = "135790-amber-comet".to_string();
        request.rendezvous = FfiRendezvousPlan {
            use_room: true,
            use_mdns: false,
            internet_available: false,
        };

        let error = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect_err("room without internet and without mDNS must be rejected");

        assert!(error.to_string().contains("internet is unavailable"));
    }

    #[test]
    fn ffi_queue_respects_serial_runtime_setting() {
        let dir = tempfile::tempdir().unwrap();
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings {
            concurrent_transfers: false,
            ..EnvoixRuntimeSettings::default()
        });

        let (first_tx, _first_rx) = channel();
        let mut first = FfiTransferRequest::receive(
            first_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        first.activity_id = "serial-first".to_string();
        first.limits.max_parallel_transfers = 2;
        session
            .start_transfer(first, Arc::new(TestObserver(first_tx)))
            .unwrap();

        let (second_tx, second_rx) = channel();
        let mut second = FfiTransferRequest::receive(
            second_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        second.activity_id = "serial-second".to_string();
        second.limits.max_parallel_transfers = 2;
        session
            .start_transfer(second, Arc::new(TestObserver(second_tx)))
            .unwrap();

        let queued = recv_activity(&second_rx, "serial-second", Duration::from_secs(2));
        assert_eq!(queued.state, FfiTransferActivityState::Queued);
        assert_eq!(queued.limits.max_parallel_transfers, 1);
        assert_eq!(
            snapshot_record(&session, "serial-second").map(|record| record.state),
            Some(FfiTransferActivityState::Queued)
        );
        assert_no_nonqueued_activity(&second_rx, "serial-second", Duration::from_millis(200));

        assert!(session.cancel_activity("serial-second".to_string()));
        assert!(session.cancel_activity("serial-first".to_string()));
    }

    #[test]
    fn ffi_queue_holds_and_cancels_pending_activity() {
        let dir = tempfile::tempdir().unwrap();
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let session = EnvoixSession::new();

        let (first_tx, _first_rx) = channel();
        let mut first = FfiTransferRequest::receive(
            first_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        first.activity_id = "queue-first".to_string();
        first.limits.max_parallel_transfers = 1;
        session
            .start_transfer(first, Arc::new(TestObserver(first_tx)))
            .unwrap();

        let (second_tx, second_rx) = channel();
        let mut second = FfiTransferRequest::receive(
            second_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        second.activity_id = "queue-second".to_string();
        second.limits.max_parallel_transfers = 1;
        session
            .start_transfer(second, Arc::new(TestObserver(second_tx)))
            .unwrap();

        let queued = recv_activity(&second_rx, "queue-second", Duration::from_secs(2));
        assert_eq!(queued.state, FfiTransferActivityState::Queued);
        assert!(snapshot_record(&session, "queue-first").is_some());
        assert_eq!(
            snapshot_record(&session, "queue-second").map(|record| record.state),
            Some(FfiTransferActivityState::Queued)
        );
        assert_no_nonqueued_activity(&second_rx, "queue-second", Duration::from_millis(200));

        assert!(session.cancel_activity("queue-second".to_string()));
        let canceled = recv_activity(&second_rx, "queue-second", Duration::from_secs(2));
        assert_eq!(canceled.state, FfiTransferActivityState::Canceled);
        assert_eq!(
            snapshot_record(&session, "queue-second").map(|record| record.state),
            Some(FfiTransferActivityState::Canceled)
        );
        assert_eq!(
            session
                .get_transfer_activity("queue-second".to_string())
                .map(|record| record.state),
            Some(FfiTransferActivityState::Canceled)
        );
        assert_eq!(session.clear_transfer_history(), 1);
        assert!(snapshot_record(&session, "queue-second").is_none());

        assert!(session.cancel_activity("queue-first".to_string()));
    }

    #[test]
    fn ffi_queue_discards_pending_activity() {
        let dir = tempfile::tempdir().unwrap();
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let session = EnvoixSession::new();

        let (first_tx, _first_rx) = channel();
        let mut first = FfiTransferRequest::receive(
            first_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        first.activity_id = "discard-first".to_string();
        first.limits.max_parallel_transfers = 1;
        session
            .start_transfer(first, Arc::new(TestObserver(first_tx)))
            .unwrap();

        let (second_tx, second_rx) = channel();
        let mut second = FfiTransferRequest::receive(
            second_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        second.activity_id = "discard-second".to_string();
        second.limits.max_parallel_transfers = 1;
        session
            .start_transfer(second, Arc::new(TestObserver(second_tx)))
            .unwrap();

        let queued = recv_activity(&second_rx, "discard-second", Duration::from_secs(2));
        assert_eq!(queued.state, FfiTransferActivityState::Queued);
        assert!(snapshot_record(&session, "discard-second").is_some());

        assert!(session.discard_transfer_activity("discard-second".to_string()));
        let canceled = recv_activity(&second_rx, "discard-second", Duration::from_secs(2));
        assert_eq!(canceled.state, FfiTransferActivityState::Canceled);
        assert!(snapshot_record(&session, "discard-second").is_none());
        assert!(
            session
                .get_transfer_activity("discard-second".to_string())
                .is_none()
        );
        assert!(!session.discard_transfer_activity("discard-second".to_string()));

        assert!(session.cancel_activity("discard-first".to_string()));
    }

    #[test]
    fn ffi_queue_pauses_and_resumes_pending_activity() {
        let dir = tempfile::tempdir().unwrap();
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let session = EnvoixSession::new();

        let (first_tx, _first_rx) = channel();
        let mut first = FfiTransferRequest::receive(
            first_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        first.activity_id = "pause-first".to_string();
        first.limits.max_parallel_transfers = 1;
        session
            .start_transfer(first, Arc::new(TestObserver(first_tx)))
            .unwrap();

        let (second_tx, second_rx) = channel();
        let mut second = FfiTransferRequest::receive(
            second_dir.to_str().unwrap().to_string(),
            FfiTransferMode::ShowInvite,
        );
        second.activity_id = "pause-second".to_string();
        second.limits.max_parallel_transfers = 1;
        session
            .start_transfer(second, Arc::new(TestObserver(second_tx)))
            .unwrap();

        let queued = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
        assert_eq!(queued.state, FfiTransferActivityState::Queued);

        assert!(session.pause_activity("pause-second".to_string()));
        let paused = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
        assert_eq!(paused.state, FfiTransferActivityState::Paused);
        assert_eq!(paused.recovery_action, FfiRecoveryAction::Resume);
        assert_eq!(
            snapshot_record(&session, "pause-second").map(|record| record.state),
            Some(FfiTransferActivityState::Paused)
        );

        assert!(session.resume_activity("pause-second".to_string()));
        let requeued = recv_activity(&second_rx, "pause-second", Duration::from_secs(2));
        assert_eq!(requeued.state, FfiTransferActivityState::Queued);
        assert!(requeued.attempt_id.is_empty());
        assert_eq!(
            snapshot_record(&session, "pause-second").map(|record| record.state),
            Some(FfiTransferActivityState::Queued)
        );
        assert_no_nonqueued_activity(&second_rx, "pause-second", Duration::from_millis(200));

        assert!(session.cancel_activity("pause-second".to_string()));
        assert!(session.cancel_activity("pause-first".to_string()));
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
        let (bytes, receiver_events, receiver_activity) =
            recv_completed_activity(&rrx, Duration::from_secs(15));

        let completed_path = output_dir.join("hello.txt");
        assert_eq!(bytes, text.len() as u64);
        assert_eq!(
            receiver_activity.completed_file_path,
            completed_path.to_string_lossy()
        );
        assert_eq!(std::fs::read(completed_path).unwrap(), text);
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
        let (bytes, receiver_events, receiver_activity) =
            recv_completed_activity(&rrx, Duration::from_secs(20));

        let completed_path = output_dir.join("room.txt");
        assert_eq!(bytes, text.len() as u64);
        assert_eq!(
            receiver_activity.completed_file_path,
            completed_path.to_string_lossy()
        );
        assert_eq!(std::fs::read(completed_path).unwrap(), text);
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
