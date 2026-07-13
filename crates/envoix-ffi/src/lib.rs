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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use envoix_client::{
    BindAddrs, PeerDescriptor, TransferDirection, TransferSummary,
    api::{
        Client, DataPath, ErrorKind, FailureCategory, FailureCode, FailureOrigin, FailurePhase,
        Invite, PairingStep, PathPolicy, PeerSource, Phase, RecoveryAction, Role,
        SessionFailureCode, StampedEvent, Transfer, TransferError, TransferEvent, TransferMode,
        TransferOptions,
        driver::{
            ClientContext, SessionContext, SessionNotice, SessionParams, SessionSnapshot,
            TransferSession as CanonicalTransferSession,
        },
        machine::{PauseOrigin, State as CanonicalState},
        record::{RecordStore, TransferRecord},
    },
};
use envoix_qr::QrInvitePayload;
use envoix_rendezvous_iroh::generate_code;
use envoix_storage::LocalFileStorage;
use envoix_types::TransferId;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{mpsc, oneshot};

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
static DURABLE_RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
const TRANSFER_ACTIVITY_HISTORY_CAP: usize = 50;
const ROOM_SEND_FALLBACK_TIMEOUT: Duration = Duration::from_secs(60);
/// Native UIs do not need one callback per network chunk. A bounded cadence
/// prevents large transfers from flooding the Swift/Kotlin main thread.
const NATIVE_PROGRESS_INTERVAL_MS: u64 = 500;
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
            reason: "unsupported Envoix pairing invite scheme".to_string(),
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
    /// An operation failed; `reason` is a human-readable reason.
    #[error("{reason}")]
    Operation {
        /// Human-readable operation failure reason.
        reason: String,
    },
}

fn op_err(error: impl std::fmt::Display) -> EnvoixError {
    EnvoixError::Operation {
        reason: error.to_string(),
    }
}

#[cfg(not(target_os = "android"))]
fn init_env_logging() {
    use std::sync::Once;

    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let Ok(spec) = std::env::var("ENVOIX_LOG") else {
            return;
        };
        let Ok(filter) = tracing_subscriber::EnvFilter::try_new(spec) else {
            return;
        };
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_target(false)
            .try_init();
    });
}

#[cfg(target_os = "android")]
mod android_bootstrap {
    use std::io::Write;
    use std::sync::OnceLock;

    use jni::JNIEnv;
    use jni::JavaVM;
    use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};

    static LOG_VM: OnceLock<JavaVM> = OnceLock::new();
    static LOG_SINK: OnceLock<GlobalRef> = OnceLock::new();
    type LogReload = tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >;
    static LOG_RELOAD: OnceLock<LogReload> = OnceLock::new();

    const DEFAULT_LOG: &str = "envoix=debug,iroh=info,warn";

    /// Wire the Android VM + app context into dependencies that use ndk_context.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_envoix_app_NativeBootstrap_initContext(
        env: JNIEnv,
        _class: JClass,
        context: JObject,
    ) {
        let Ok(vm) = env.get_java_vm() else { return };
        let Ok(ctx) = env.new_global_ref(&context) else {
            return;
        };
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer() as *mut _,
                ctx.as_obj().as_raw() as *mut _,
            );
        }
        // ndk_context stores raw pointers, so the Java context must live for the
        // process lifetime.
        std::mem::forget(ctx);
    }

    /// Forward Rust tracing output into the app's LogStore.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_envoix_app_NativeBootstrap_initLogging(
        env: JNIEnv,
        _class: JClass,
        sink: JObject,
    ) {
        let Ok(vm) = env.get_java_vm() else { return };
        let Ok(sink) = env.new_global_ref(&sink) else {
            return;
        };
        let _ = LOG_VM.set(vm);
        let _ = LOG_SINK.set(sink);

        let spec = std::env::var("ENVOIX_LOG").unwrap_or_else(|_| DEFAULT_LOG.to_string());
        let filter = tracing_subscriber::EnvFilter::try_new(&spec)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG));
        let (filter, handle) = tracing_subscriber::reload::Layer::new(filter);
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let installed = tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(JniLogWriter)
                    .with_ansi(false)
                    .with_target(false),
            )
            .try_init()
            .is_ok();
        if installed {
            let _ = LOG_RELOAD.set(handle);
        }
    }

    /// Change the log filter used by the Android developer verbosity toggle.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_envoix_app_NativeBootstrap_setLogLevel(
        mut env: JNIEnv,
        _class: JClass,
        spec: JString,
    ) {
        let Ok(spec) = env.get_string(&spec) else {
            return;
        };
        let spec: String = spec.into();
        if let (Some(handle), Ok(filter)) = (
            LOG_RELOAD.get(),
            tracing_subscriber::EnvFilter::try_new(&spec),
        ) {
            let _ = handle.reload(filter);
        }
    }

    fn log_line(line: &str) {
        let (Some(vm), Some(sink)) = (LOG_VM.get(), LOG_SINK.get()) else {
            return;
        };
        let Ok(mut env) = vm.attach_current_thread() else {
            return;
        };
        if let Ok(js) = env.new_string(line) {
            let _ = env.call_method(sink, "log", "(Ljava/lang/String;)V", &[JValue::Object(&js)]);
        }
    }

    #[derive(Clone)]
    struct JniLogWriter;

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for JniLogWriter {
        type Writer = LineBuf;

        fn make_writer(&'a self) -> Self::Writer {
            LineBuf(Vec::new())
        }
    }

    struct LineBuf(Vec<u8>);

    impl Write for LineBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if !self.0.is_empty() {
                if let Ok(s) = std::str::from_utf8(&self.0) {
                    log_line(s.trim_end());
                }
                self.0.clear();
            }
            Ok(())
        }
    }

    impl Drop for LineBuf {
        fn drop(&mut self) {
            let _ = self.flush();
        }
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
    /// Receive into staging, then wait for the native shell to publish to the
    /// user-selected Files/MediaStore destination.
    pub publication_required: bool,
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
    Unconfirmed,
    Publishing,
    Completed,
    Failed,
    Paused,
    Canceled,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityRecord {
    pub activity_id: String,
    /// Monotonic canonical snapshot sequence; native clients discard older
    /// deliveries when platform callback scheduling reorders them.
    pub sequence: u64,
    pub attempt_id: String,
    pub state: FfiTransferActivityState,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub transfer_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub bytes_resumed: u64,
    pub speed_bps: u64,
    pub average_speed_bps: u64,
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

/// Platform courier for the opaque completion-receipt mailbox. The Rust
/// driver owns keys, sealing, verification, polling, and state transitions;
/// native code only performs HTTPS GET/POST and reports the result back.
#[uniffi::export(with_foreign)]
pub trait MailboxObserver: Send + Sync {
    fn on_fetch_receipt(&self, activity_id: String, key: String);
    fn on_post_receipt(&self, activity_id: String, key: String, blob: Vec<u8>);
}

/// One durable transfer card driven by the canonical Rust state machine.
#[derive(uniffi::Object)]
pub struct DurableEnvoixSession {
    driver: Mutex<Option<CanonicalTransferSession>>,
    activity: Arc<Mutex<FfiTransferActivityRecord>>,
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
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.receipt_response((!blob.is_empty()).then_some(blob))
    }

    pub fn receipt_posted(&self) -> bool {
        let driver = self.driver.lock().unwrap();
        let Some(driver) = driver.as_ref() else {
            return false;
        };
        driver.receipt_posted()
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
    mut request: FfiTransferRequest,
    records_dir: String,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserver>,
) -> Result<Arc<DurableEnvoixSession>, EnvoixError> {
    if request.activity_id.trim().is_empty() {
        request.activity_id = next_activity_id();
    }
    normalize_transfer_limits(&settings, &mut request.limits);
    validate_transfer_request(&settings, &request)?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = RecordStore::new(records_dir);
    let mut context = canonical_context_for_request(&settings, &request)?;
    if context.requires_stable_listener_identity() {
        context.client.identity_file = Some(store.identity_path(&request.activity_id));
    }
    let activity = Arc::new(Mutex::new(FfiTransferActivityRecord::from_request(
        &request,
        now_ms(),
    )));
    let runtime = durable_runtime()?;
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalTransferSession::start(context.clone(), Some((store, request.activity_id.clone())))
            .map_err(op_err)?
    };
    let session = Arc::new(DurableEnvoixSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
    });
    runtime.handle().spawn(drive_durable_notices(
        request.activity_id,
        context,
        notices,
        activity,
        observer,
        mailbox,
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
    let activity_id = required_value(&activity_id, "activity_id")?;
    let records_dir = required_value(&records_dir, "records_dir")?;
    let store = RecordStore::new(records_dir);
    let runtime = durable_runtime()?;
    let mut record = runtime
        .block_on(store.load_all())
        .into_iter()
        .find(|record| record.id == activity_id)
        .ok_or_else(|| EnvoixError::Operation {
            reason: format!("transfer record not found: {activity_id}"),
        })?;
    if record.context.requires_stable_listener_identity()
        && record.context.client.identity_file.is_none()
    {
        record.context.client.identity_file = Some(store.identity_path(&activity_id));
    }
    let context = record.context.clone();
    let activity = Arc::new(Mutex::new(activity_from_canonical_record(&record)));
    let (driver, notices) = {
        let _guard = runtime.enter();
        CanonicalTransferSession::restore(record, Some((store, activity_id.clone())))
            .map_err(op_err)?
    };
    let session = Arc::new(DurableEnvoixSession {
        driver: Mutex::new(Some(driver)),
        activity: activity.clone(),
    });
    runtime.handle().spawn(drive_durable_notices(
        activity_id,
        context,
        notices,
        activity,
        observer,
        mailbox,
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
        .get_or_init(|| Runtime::new().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|reason| EnvoixError::Operation {
            reason: reason.clone(),
        })
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
            publication_required: false,
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
            publication_required: false,
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

    fn apply_event(&mut self, event: &FfiTransferEvent) {
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
    fn apply_observation(&mut self, event: &FfiTransferEvent) {
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
        self.retryable = false;
        self.recovery_action = FfiRecoveryAction::None;
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

    fn apply_peer_paused(&mut self, ts_ms: u64) {
        self.apply_paused(ts_ms);
        self.diagnostic_message = "peer paused; waiting for resume".to_string();
        self.failure_origin = FfiFailureOrigin::Peer;
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

async fn drive_durable_notices(
    activity_id: String,
    context: SessionContext,
    mut notices: mpsc::UnboundedReceiver<SessionNotice>,
    activity: Arc<Mutex<FfiTransferActivityRecord>>,
    observer: Arc<dyn TransferObserver>,
    mailbox: Arc<dyn MailboxObserver>,
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
            SessionNotice::FetchReceipt { key } => {
                mailbox.on_fetch_receipt(activity_id.clone(), key);
            }
            SessionNotice::PostReceipt { key, blob } => {
                mailbox.on_post_receipt(activity_id.clone(), key, blob);
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

fn canonical_context_for_request(
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

fn activity_from_canonical_record(record: &TransferRecord) -> FfiTransferActivityRecord {
    let mut request = request_from_canonical_context(&record.id, &record.context);
    request.activity_id = record.id.clone();
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
    activity
}

fn request_from_canonical_context(
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

fn apply_canonical_snapshot(
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
    if matches!(
        session.state,
        CanonicalState::Transferring | CanonicalState::Confirming
    ) && activity.started_at_ms == 0
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
        reason: error.to_string(),
    })
}

fn transfer_options_for_request(
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

fn peer_sources_for_request(
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

fn required_path(value: &str, field: &str) -> Result<String, EnvoixError> {
    required_value(value, field)
}

fn required_value(value: &str, field: &str) -> Result<String, EnvoixError> {
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

fn should_emit_native_event(event: &FfiTransferEvent, last_progress_ms: &mut u64) -> bool {
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

fn to_ffi_event(event: &StampedEvent, activity_id: &str) -> FfiTransferEvent {
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
        }
        TransferEvent::Confirming { transfer_id } => {
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
    use std::sync::{Mutex, OnceLock, Weak};
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

    struct NoopMailbox;

    impl MailboxObserver for NoopMailbox {
        fn on_fetch_receipt(&self, _activity_id: String, _key: String) {}

        fn on_post_receipt(&self, _activity_id: String, _key: String, _blob: Vec<u8>) {}
    }

    enum MailboxMsg {
        Fetch {
            activity_id: String,
            key: String,
        },
        Post {
            activity_id: String,
            key: String,
            blob: Vec<u8>,
        },
    }

    struct TestMailbox(Sender<MailboxMsg>);

    impl MailboxObserver for TestMailbox {
        fn on_fetch_receipt(&self, activity_id: String, key: String) {
            let _ = self.0.send(MailboxMsg::Fetch { activity_id, key });
        }

        fn on_post_receipt(&self, activity_id: String, key: String, blob: Vec<u8>) {
            let _ = self.0.send(MailboxMsg::Post {
                activity_id,
                key,
                blob,
            });
        }
    }

    struct PauseOnProgressObserver {
        messages: Sender<Msg>,
        session: Weak<EnvoixSession>,
        activity_id: String,
        pause_result: Sender<bool>,
        requested: std::sync::atomic::AtomicBool,
    }

    impl TransferObserver for PauseOnProgressObserver {
        fn on_invite_ready(&self, invite: String) {
            let _ = self.messages.send(Msg::Invite(invite));
        }
        fn on_started(&self, _file_name: String, _total_bytes: u64) {}
        fn on_progress(&self, _transferred: u64, _total: u64) {
            if !self.requested.swap(true, Ordering::SeqCst) {
                let accepted = self
                    .session
                    .upgrade()
                    .is_some_and(|session| session.pause_activity(self.activity_id.clone()));
                let _ = self.pause_result.send(accepted);
            }
        }
        fn on_completed(&self, bytes: u64) {
            let _ = self.messages.send(Msg::Completed(bytes));
        }
        fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}
        fn on_failed(&self, reason: String) {
            let _ = self.messages.send(Msg::Failed(reason));
        }
        fn on_transfer_event(&self, event: FfiTransferEvent) {
            let _ = self.messages.send(Msg::Event(event));
        }
        fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
            let _ = self.messages.send(Msg::Activity(record));
        }
        fn on_status(&self, _message: String) {}
    }

    struct DurablePauseOnProgressObserver {
        messages: Sender<Msg>,
        session: Mutex<Option<Weak<DurableEnvoixSession>>>,
        result: Sender<bool>,
        requested: std::sync::atomic::AtomicBool,
    }

    impl TransferObserver for DurablePauseOnProgressObserver {
        fn on_invite_ready(&self, invite: String) {
            let _ = self.messages.send(Msg::Invite(invite));
        }

        fn on_started(&self, _file_name: String, _total_bytes: u64) {}

        fn on_progress(&self, _transferred: u64, _total: u64) {
            if self.requested.load(Ordering::SeqCst) {
                return;
            }
            let accepted = self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|session| session.pause());
            if accepted && !self.requested.swap(true, Ordering::SeqCst) {
                let _ = self.result.send(true);
            }
        }

        fn on_completed(&self, bytes: u64) {
            let _ = self.messages.send(Msg::Completed(bytes));
        }

        fn on_transfer_failed(&self, _failure: FfiTransferFailure) {}

        fn on_failed(&self, reason: String) {
            let _ = self.messages.send(Msg::Failed(reason));
        }

        fn on_transfer_event(&self, event: FfiTransferEvent) {
            let _ = self.messages.send(Msg::Event(event));
        }

        fn on_transfer_activity(&self, record: FfiTransferActivityRecord) {
            let _ = self.messages.send(Msg::Activity(record));
        }

        fn on_status(&self, _message: String) {}
    }

    static LOOPBACK_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_loopback_tests() -> std::sync::MutexGuard<'static, ()> {
        LOOPBACK_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap()
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
        let mut completed_activity_count = 0;
        loop {
            match rx.recv_timeout(timeout).unwrap() {
                Msg::Completed(bytes) => {
                    assert_eq!(
                        completed_activity_count, 1,
                        "receive should publish exactly one terminal completed activity"
                    );
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
                    FfiTransferActivityState::Completed => {
                        assert!(
                            !record.completed_file_path.is_empty(),
                            "completed receive activity must include its committed file path"
                        );
                        completed_activity_count += 1;
                        completed_activity = Some(record);
                    }
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

    fn recv_activity_state(
        rx: &std::sync::mpsc::Receiver<Msg>,
        activity_id: &str,
        state: FfiTransferActivityState,
        timeout: Duration,
    ) -> FfiTransferActivityRecord {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("timed out waiting for activity state");
            match rx.recv_timeout(remaining).unwrap() {
                Msg::Activity(record)
                    if record.activity_id == activity_id && record.state == state =>
                {
                    return record;
                }
                Msg::Failed(reason) => panic!("transfer failed: {reason}"),
                Msg::Invite(_) | Msg::Completed(_) | Msg::Event(_) | Msg::Activity(_) => {}
            }
        }
    }

    fn start_test_broker() -> String {
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
                broker_tx
                    .send(format!("{server_id}@{server_addr}"))
                    .unwrap();
                serve_endpoint(server, Arc::new(RoomRegistry::new()), None)
                    .await
                    .unwrap();
            });
        });
        broker_rx.recv_timeout(Duration::from_secs(10)).unwrap()
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
    fn native_progress_is_rate_limited_but_final_progress_is_delivered() {
        let mut last_progress_ms = 0;
        let mut progress = FfiTransferEvent::empty("activity-1", 1_000);
        progress.kind = FfiTransferEventKind::Progress;
        progress.bytes_transferred = 10;
        progress.total_bytes = 100;
        assert!(should_emit_native_event(&progress, &mut last_progress_ms));

        progress.ts_ms = 1_200;
        progress.bytes_transferred = 20;
        assert!(!should_emit_native_event(&progress, &mut last_progress_ms));

        progress.ts_ms = 1_500;
        progress.bytes_transferred = 30;
        assert!(should_emit_native_event(&progress, &mut last_progress_ms));

        progress.ts_ms = 1_510;
        progress.bytes_transferred = 100;
        assert!(should_emit_native_event(&progress, &mut last_progress_ms));

        let completed = FfiTransferEvent::empty("activity-1", 1_511);
        assert!(should_emit_native_event(&completed, &mut last_progress_ms));
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
            publication_required: false,
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
        assert_eq!(record.state, FfiTransferActivityState::Verifying);
        assert_eq!(record.bytes_transferred, 100);
        assert_eq!(record.completed_at_ms, 0);

        record.apply_completed(
            &TransferSummary {
                bytes_transferred: 100,
                transfer_id: TransferId::new("tx1"),
                file_name: "report.pdf".to_string(),
            },
            21,
            "/tmp/report.pdf".to_string(),
        );
        assert_eq!(record.state, FfiTransferActivityState::Completed);
        assert_eq!(record.completed_at_ms, 21);
        assert_eq!(record.completed_file_path, "/tmp/report.pdf");
    }

    #[test]
    fn confirming_activity_is_finalizing_and_rejects_stop_requests() {
        let activity_id = "confirming-activity".to_string();
        let mut request =
            FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Room);
        request.activity_id = activity_id.clone();
        let mut activity = make_transfer_activity_record(request);
        activity.state = FfiTransferActivityState::Verifying;
        activity.diagnostic_message = "confirming".to_string();
        assert!(is_finalizing_activity(&activity));
        assert!(!can_pause_durable_activity(&activity));
        assert!(!can_cancel_durable_activity(&activity));

        let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
        let (control, _receiver) = oneshot::channel();
        session.queue.lock().unwrap().active.insert(
            activity_id.clone(),
            ActiveTransfer {
                control: Some(control),
                limit: 1,
                activity,
            },
        );

        assert!(!session.cancel_activity(activity_id.clone()));
        assert!(!session.pause_activity(activity_id));
    }

    #[test]
    fn durable_controls_only_accept_legal_lifecycle_states() {
        let request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
        let mut activity = make_transfer_activity_record(request);
        assert!(can_pause_durable_activity(&activity));
        assert!(can_cancel_durable_activity(&activity));
        assert!(!can_resume_durable_activity(&activity));

        activity.apply_paused(now_ms());
        assert!(!can_pause_durable_activity(&activity));
        assert!(can_cancel_durable_activity(&activity));
        assert!(can_resume_durable_activity(&activity));

        activity.state = FfiTransferActivityState::Completed;
        assert!(!can_pause_durable_activity(&activity));
        assert!(!can_cancel_durable_activity(&activity));
        assert!(!can_resume_durable_activity(&activity));
    }

    #[test]
    fn canonical_activity_preserves_structured_network_failure() {
        let request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
        let context =
            canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
        let mut activity = make_transfer_activity_record(request);
        let mut session = envoix_client::api::machine::Session::new(TransferDirection::Receive);
        session.state = CanonicalState::Failed;
        session.reason_code = Some(SessionFailureCode::Other);
        session.reason = Some("network connection timed out".to_string());
        let mut failure = TransferError::transport(Phase::Transfer, "network connection timed out")
            .to_failure(Some(TransferDirection::Receive));
        failure.attempt_id = Some("attempt-1".to_string());
        session.failure = Some(failure);

        apply_canonical_snapshot(
            &mut activity,
            &SessionSnapshot {
                seq: 1,
                speed_bps: 0.0,
                avg_bps: 0.0,
                session,
            },
            &context,
            now_ms(),
        );

        assert_eq!(activity.state, FfiTransferActivityState::Failed);
        assert_eq!(activity.failure_code, FfiFailureCode::Timeout);
        assert_eq!(activity.failure_category, FfiFailureCategory::Network);
        assert_eq!(activity.failure_phase, FfiFailurePhase::Transferring);
        assert_eq!(activity.attempt_id, "attempt-1");
        assert!(activity.diagnostic_message.contains("timed out"));
    }

    #[test]
    fn resume_during_pause_transition_is_not_lost() {
        let activity_id = "pause-resume-race".to_string();
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
        request.activity_id = activity_id.clone();
        let mut activity = make_transfer_activity_record(request.clone());
        activity.state = FfiTransferActivityState::Transferring;

        let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
        let (messages, _rx) = channel();
        let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
        let (control, _control_receiver) = oneshot::channel();
        session.queue.lock().unwrap().active.insert(
            activity_id.clone(),
            ActiveTransfer {
                control: Some(control),
                limit: 1,
                activity: activity.clone(),
            },
        );

        assert!(session.pause_activity(activity_id.clone()));
        assert!(session.resume_activity(activity_id.clone()));

        activity.apply_paused(now_ms());
        let notice = finish_transfer_activity(
            &activity_id,
            Some(QueuedTransfer {
                request,
                observer,
                activity,
            }),
            &session.queue,
        )
        .expect("paused activity should be requeued");

        assert_eq!(notice.activity.state, FfiTransferActivityState::Queued);
        assert_eq!(notice.status, "resuming");
        let queue = session.queue.lock().unwrap();
        assert_eq!(queue.pending.len(), 1);
        assert!(!queue.paused.contains_key(&activity_id));
        assert!(!queue.active.contains_key(&activity_id));
    }

    #[test]
    fn cancel_during_pause_transition_overrides_resume() {
        let activity_id = "pause-cancel-race".to_string();
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::ShowInvite);
        request.activity_id = activity_id.clone();
        let mut activity = make_transfer_activity_record(request.clone());
        activity.state = FfiTransferActivityState::Transferring;

        let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
        let (messages, _rx) = channel();
        let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
        let (control, _control_receiver) = oneshot::channel();
        session.queue.lock().unwrap().active.insert(
            activity_id.clone(),
            ActiveTransfer {
                control: Some(control),
                limit: 1,
                activity: activity.clone(),
            },
        );

        assert!(session.pause_activity(activity_id.clone()));
        assert!(session.resume_activity(activity_id.clone()));
        assert!(session.cancel_activity(activity_id.clone()));

        activity.apply_paused(now_ms());
        let notice = finish_transfer_activity(
            &activity_id,
            Some(QueuedTransfer {
                request,
                observer,
                activity,
            }),
            &session.queue,
        )
        .expect("paused activity should be canceled");

        assert_eq!(notice.activity.state, FfiTransferActivityState::Canceled);
        assert!(!notice.activity.retryable);
        assert_eq!(notice.activity.recovery_action, FfiRecoveryAction::None);
        assert_eq!(notice.status, "canceled");
        let queue = session.queue.lock().unwrap();
        assert!(queue.pending.is_empty());
        assert!(!queue.paused.contains_key(&activity_id));
        assert_eq!(
            queue.history.front().map(|record| record.state),
            Some(FfiTransferActivityState::Canceled)
        );
    }

    #[test]
    fn peer_pause_requeues_while_concurrent_cancel_still_wins() {
        let activity_id = "peer-pause-race".to_string();
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        request.activity_id = activity_id.clone();
        let mut activity = make_transfer_activity_record(request.clone());
        activity.state = FfiTransferActivityState::Transferring;
        activity.apply_peer_paused(now_ms());
        assert_eq!(activity.failure_origin, FfiFailureOrigin::Peer);

        let session = EnvoixSession::new_with_settings(EnvoixRuntimeSettings::default());
        let (messages, _rx) = channel();
        let observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
        let (control, _control_receiver) = oneshot::channel();
        session.queue.lock().unwrap().active.insert(
            activity_id.clone(),
            ActiveTransfer {
                control: Some(control),
                limit: 1,
                activity: activity.clone(),
            },
        );

        assert!(session.cancel_activity(activity_id.clone()));
        schedule_peer_pause_resume(&session.queue, &activity_id);
        let notice = finish_transfer_activity(
            &activity_id,
            Some(QueuedTransfer {
                request,
                observer,
                activity,
            }),
            &session.queue,
        )
        .expect("peer-paused activity should resolve its pending action");

        assert_eq!(notice.activity.state, FfiTransferActivityState::Canceled);
        assert_eq!(notice.status, "canceled");
        let queue = session.queue.lock().unwrap();
        assert!(queue.pending.is_empty());
        assert!(!queue.paused.contains_key(&activity_id));
        drop(queue);

        let resume_id = "peer-pause-resume".to_string();
        let mut resume_request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        resume_request.activity_id = resume_id.clone();
        let mut resume_activity = make_transfer_activity_record(resume_request.clone());
        resume_activity.apply_peer_paused(now_ms());
        let (messages, _rx) = channel();
        let resume_observer: Arc<dyn TransferObserver> = Arc::new(TestObserver(messages));
        let (control, _control_receiver) = oneshot::channel();
        session.queue.lock().unwrap().active.insert(
            resume_id.clone(),
            ActiveTransfer {
                control: Some(control),
                limit: 1,
                activity: resume_activity.clone(),
            },
        );
        schedule_peer_pause_resume(&session.queue, &resume_id);
        let notice = finish_transfer_activity(
            &resume_id,
            Some(QueuedTransfer {
                request: resume_request,
                observer: resume_observer,
                activity: resume_activity,
            }),
            &session.queue,
        )
        .expect("peer pause should automatically queue a resumed attempt");

        assert_eq!(notice.activity.state, FfiTransferActivityState::Queued);
        assert_eq!(notice.status, "resuming");
        assert_eq!(session.queue.lock().unwrap().pending.len(), 1);
    }

    #[test]
    fn canceled_receive_cleanup_deletes_only_its_exact_partial() {
        let runtime = Runtime::new().unwrap();
        runtime.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let transfer_id = TransferId::new("cleanup-transfer");
            let other_id = TransferId::new("other-transfer");
            let state = envoix_storage::TransferResumeState {
                transfer_id: transfer_id.clone(),
                file_name: "movie.mkv".to_string(),
                file_size: 8,
                chunk_size: 4,
                bytes_received: 4,
                next_chunk_index: 1,
                hash_bytes: 4,
                hash_checkpoint: None,
            };
            let other_state = envoix_storage::TransferResumeState {
                transfer_id: other_id.clone(),
                ..state.clone()
            };
            LocalFileStorage::write_resume_state(dir.path(), &state)
                .await
                .unwrap();
            LocalFileStorage::write_resume_state(dir.path(), &other_state)
                .await
                .unwrap();
            let target_temp =
                LocalFileStorage::resumable_temp_path(dir.path(), "movie.mkv", &transfer_id)
                    .unwrap();
            let other_temp =
                LocalFileStorage::resumable_temp_path(dir.path(), "movie.mkv", &other_id).unwrap();
            std::fs::write(&target_temp, b"abcd").unwrap();
            std::fs::write(&other_temp, b"wxyz").unwrap();

            let request = FfiTransferRequest::receive(
                dir.path().to_string_lossy().into_owned(),
                FfiTransferMode::ShowInvite,
            );
            let mut activity = FfiTransferActivityRecord::from_request(&request, now_ms());
            activity.transfer_id = transfer_id.to_string();
            activity.file_name = "movie.mkv".to_string();
            cleanup_canceled_receive(&request, &activity).await;

            assert!(!target_temp.exists());
            assert!(
                LocalFileStorage::read_resume_state(dir.path(), "movie.mkv", &transfer_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(other_temp.exists());
            assert!(
                LocalFileStorage::read_resume_state(dir.path(), "movie.mkv", &other_id)
                    .await
                    .unwrap()
                    .is_some()
            );
        });
    }

    #[test]
    fn discarding_failed_receive_deletes_retained_partial() {
        let dir = tempfile::tempdir().unwrap();
        let transfer_id = TransferId::new("discard-failed-transfer");
        let state = envoix_storage::TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "failed.bin".to_string(),
            file_size: 4,
            chunk_size: 4,
            bytes_received: 4,
            next_chunk_index: 1,
            hash_bytes: 4,
            hash_checkpoint: None,
        };
        let runtime = Runtime::new().unwrap();
        runtime
            .block_on(LocalFileStorage::write_resume_state(dir.path(), &state))
            .unwrap();
        let temp =
            LocalFileStorage::resumable_temp_path(dir.path(), &state.file_name, &transfer_id)
                .unwrap();
        std::fs::write(&temp, b"data").unwrap();

        let mut request = FfiTransferRequest::receive(
            dir.path().to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        request.activity_id = "discard-failed".to_string();
        let mut record = FfiTransferActivityRecord::from_request(&request, now_ms());
        record.state = FfiTransferActivityState::Failed;
        record.transfer_id = transfer_id.to_string();
        record.file_name = state.file_name.clone();
        let session = EnvoixSession::new();
        {
            let mut queue = session.queue.lock().unwrap();
            queue.requests.insert(request.activity_id.clone(), request);
            queue.push_history(record);
        }

        assert!(session.discard_transfer_activity("discard-failed".to_string()));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while temp.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!temp.exists());
        assert!(
            runtime
                .block_on(LocalFileStorage::read_resume_state(
                    dir.path(),
                    &state.file_name,
                    &transfer_id,
                ))
                .unwrap()
                .is_none()
        );
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
            publication_required: false,
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
            publication_required: false,
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
    fn room_rendezvous_plan_retries_through_relay_before_mdns() {
        let mut request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        request.code = "135790-amber-comet".to_string();

        let sources = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("room request should build rendezvous sources");

        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].mode, FfiTransferMode::Room);
        assert_eq!(sources[0].path_policy_override, None);
        assert_eq!(sources[1].mode, FfiTransferMode::Room);
        assert_eq!(
            sources[1].path_policy_override,
            Some(FfiPathPolicy::RelayOnly)
        );
        assert_eq!(sources[2].mode, FfiTransferMode::Mdns);
        match &sources[2].source {
            PeerSource::Mdns { token } => assert_eq!(token.as_deref(), Some("135790-amber-comet")),
            other => panic!("expected mDNS fallback source, got {other:?}"),
        }
    }

    #[test]
    fn canonical_room_context_uses_auto_path_then_mdns_without_duplicate_sources() {
        let mut request =
            FfiTransferRequest::send("/tmp/envoix.txt".to_string(), FfiTransferMode::Room);
        request.code = "135790-amber-comet".to_string();

        let context = canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("canonical room context");

        assert_eq!(context.params.sources.len(), 2);
        assert!(matches!(context.params.sources[0], PeerSource::Room { .. }));
        assert!(matches!(context.params.sources[1], PeerSource::Mdns { .. }));
        assert_eq!(context.params.options.path, PathPolicy::Auto);
        assert!(context.params.options.relay.is_some());
    }

    #[test]
    fn fallback_is_allowed_after_connection_but_before_transfer_starts() {
        assert!(can_fallback_after_error(None, false, true));
        assert!(!can_fallback_after_error(None, true, true));
        assert!(!can_fallback_after_error(
            Some(TransferStop::Cancel),
            false,
            true
        ));
        assert!(!can_fallback_after_error(None, false, false));
    }

    #[test]
    fn room_fallback_timeout_only_applies_to_senders() {
        let mut send_request =
            FfiTransferRequest::send("/tmp/envoix-room.txt".to_string(), FfiTransferMode::Room);
        send_request.code = "135790-amber-comet".to_string();
        let mut receive_request =
            FfiTransferRequest::receive("/tmp/envoix".to_string(), FfiTransferMode::Room);
        receive_request.code = "135790-amber-comet".to_string();

        assert_eq!(
            fallback_timeout_for_attempt(&send_request, FfiTransferMode::Room, true),
            Some(ROOM_SEND_FALLBACK_TIMEOUT),
        );
        assert_eq!(
            fallback_timeout_for_attempt(&receive_request, FfiTransferMode::Room, true),
            None,
        );
        assert_eq!(
            fallback_timeout_for_attempt(&send_request, FfiTransferMode::Room, false),
            None,
        );
        assert_eq!(
            fallback_timeout_for_attempt(&send_request, FfiTransferMode::Mdns, true),
            None,
        );
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
    fn debug_summary_redacts_room_password() {
        let mut request =
            FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Room);
        request.code = "123456-amber-comet".to_string();
        let attempts = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("room request should build rendezvous sources");

        let summary = request_debug_summary(&EnvoixRuntimeSettings::default(), &request, &attempts);
        let attempt = attempt_debug_summary(0, attempts.len(), &attempts[0]);

        assert!(summary.contains("room=123456"));
        assert!(attempt.contains("room=123456"));
        assert!(!summary.contains("amber-comet"));
        assert!(!attempt.contains("amber-comet"));
    }

    #[test]
    fn invite_debug_summary_reports_endpoint_shape_without_token() {
        let peer = PeerDescriptor::new(
            SecretKey::generate().public().to_string(),
            vec!["127.0.0.1:9000".parse().unwrap()],
        )
        .unwrap();
        let invite = QrInvitePayload::new_with_relay_urls(
            "135790-amber-comet".to_string(),
            peer.clone(),
            vec!["https://relay.example:8444".to_string()],
            999,
        )
        .encode();

        let source = invite_source_debug(&invite);
        let advertised = advertised_endpoint_debug(&peer, Some(&invite));

        assert!(source.contains("source=invite"));
        assert!(source.contains("direct=1"));
        assert!(source.contains("relay=1"));
        assert!(!source.contains("amber-comet"));
        assert!(advertised.contains("direct=1"));
        assert!(advertised.contains("relay=1"));
    }

    #[test]
    fn invite_send_auto_adds_relay_only_retry_when_invite_has_relay() {
        let peer = PeerDescriptor::new(
            SecretKey::generate().public().to_string(),
            vec!["127.0.0.1:9000".parse().unwrap()],
        )
        .unwrap();
        let invite = QrInvitePayload::new_with_relay_urls(
            "135790-amber-comet".to_string(),
            peer,
            vec!["https://relay.example:8444".to_string()],
            999,
        )
        .encode();
        let mut request =
            FfiTransferRequest::send("/tmp/report.pdf".to_string(), FfiTransferMode::Invite);
        request.invite = invite;

        let attempts = peer_sources_for_request(&EnvoixRuntimeSettings::default(), &request)
            .expect("invite request should build attempts");

        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].path_policy_override, None);
        assert_eq!(
            attempts[1].path_policy_override,
            Some(FfiPathPolicy::RelayOnly)
        );
        assert!(attempt_debug_summary(1, 2, &attempts[1]).contains("path=relay-only"));
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
        let _loopback_guard = lock_loopback_tests();
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
    fn durable_ffi_invite_loopback_persists_canonical_completion() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        let receive_records = dir.path().join("receive-records");
        let send_records = dir.path().join("send-records");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("durable.txt");
        let text = b"canonical durable ffi loopback";
        std::fs::write(&source, text).unwrap();

        let (rtx, rrx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        receive_request.activity_id = "durable-receive".to_string();
        let receiver = start_durable_transfer(
            EnvoixRuntimeSettings::default(),
            receive_request,
            receive_records.to_string_lossy().into_owned(),
            Arc::new(TestObserver(rtx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();

        let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));
        thread::sleep(Duration::from_millis(300));

        let (stx, srx) = channel();
        let mut send_request = FfiTransferRequest::send(
            source.to_string_lossy().into_owned(),
            FfiTransferMode::Invite,
        );
        send_request.activity_id = "durable-send".to_string();
        send_request.invite = invite;
        let sender = start_durable_transfer(
            EnvoixRuntimeSettings::default(),
            send_request,
            send_records.to_string_lossy().into_owned(),
            Arc::new(TestObserver(stx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();

        let (sent, _) = recv_completed(&srx, Duration::from_secs(20));
        let (received, _, receive_activity) =
            recv_completed_activity(&rrx, Duration::from_secs(20));
        let completed_path = output_dir.join("durable.txt");
        assert_eq!(sent, text.len() as u64);
        assert_eq!(received, text.len() as u64);
        assert_eq!(std::fs::read(&completed_path).unwrap(), text);
        assert_eq!(
            receive_activity.completed_file_path,
            completed_path.to_string_lossy()
        );

        let receive_history =
            list_durable_transfer_records(receive_records.to_string_lossy().into_owned()).unwrap();
        let send_history =
            list_durable_transfer_records(send_records.to_string_lossy().into_owned()).unwrap();
        assert_eq!(receive_history.len(), 1);
        assert_eq!(receive_history[0].activity_id, "durable-receive");
        assert_eq!(
            receive_history[0].state,
            FfiTransferActivityState::Completed
        );
        assert_eq!(send_history.len(), 1);
        assert_eq!(send_history[0].activity_id, "durable-send");
        assert_eq!(send_history[0].state, FfiTransferActivityState::Completed);

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn durable_invite_pause_reuses_the_scanned_endpoint_and_token() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        let receive_records = dir.path().join("receive-records");
        let send_records = dir.path().join("send-records");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("invite-pause.bin");
        let payload = vec![0x4a; 16 * 1024 * 1024];
        std::fs::write(&source, &payload).unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "chunk_size = \"256K\"\n").unwrap();
        // A non-empty broker setting suppresses the hosted relay default. This
        // keeps the regression on the original QR's direct endpoint/port.
        let settings = EnvoixRuntimeSettings {
            server_url: "unused-local-broker".to_string(),
            relay_url: String::new(),
            config_path: config.to_string_lossy().into_owned(),
            ..EnvoixRuntimeSettings::default()
        };

        let (rtx, rrx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        receive_request.activity_id = "invite-pause-receive".to_string();
        let receiver = start_durable_transfer(
            settings.clone(),
            receive_request,
            receive_records.to_string_lossy().into_owned(),
            Arc::new(TestObserver(rtx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        let invite = loopback_invite(&recv_invite(&rrx, Duration::from_secs(10)));
        thread::sleep(Duration::from_millis(300));

        let (stx, srx) = channel();
        let (pause_tx, pause_rx) = channel();
        let send_observer = Arc::new(DurablePauseOnProgressObserver {
            messages: stx,
            session: Mutex::new(None),
            result: pause_tx,
            requested: std::sync::atomic::AtomicBool::new(false),
        });
        let mut send_request = FfiTransferRequest::send(
            source.to_string_lossy().into_owned(),
            FfiTransferMode::Invite,
        );
        send_request.activity_id = "invite-pause-send".to_string();
        send_request.invite = invite;
        let sender = start_durable_transfer(
            settings,
            send_request,
            send_records.to_string_lossy().into_owned(),
            send_observer.clone(),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        *send_observer.session.lock().unwrap() = Some(Arc::downgrade(&sender));

        assert!(
            pause_rx
                .recv_timeout(Duration::from_secs(20))
                .expect("progress should trigger an invite pause")
        );
        recv_activity_state(
            &srx,
            "invite-pause-send",
            FfiTransferActivityState::Paused,
            Duration::from_secs(20),
        );
        assert!(sender.resume());

        let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
        let (received, _, activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
        assert_eq!(sent, payload.len() as u64);
        assert_eq!(received, payload.len() as u64);
        assert!(activity.bytes_resumed > 0);
        assert_eq!(
            std::fs::read(output_dir.join("invite-pause.bin")).unwrap(),
            payload
        );

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn durable_restore_marks_interrupted_attempt_lost_and_remove_discards_exact_state() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        let records_dir = dir.path().join("records");
        std::fs::create_dir_all(&output_dir).unwrap();

        let mut request = FfiTransferRequest::receive(
            output_dir.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        request.activity_id = "restore-interrupted".to_string();
        let context =
            canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
        let transfer_id = TransferId::new("restore-transfer");
        let other_id = TransferId::new("other-transfer");
        let mut session = envoix_client::api::machine::Session::new(TransferDirection::Receive);
        session.state = CanonicalState::Transferring;
        session.transfer_id = Some(transfer_id.to_string());
        session.file_name = Some("resume.bin".to_string());
        session.bytes = 4;
        session.total = 8;
        let record = TransferRecord {
            id: request.activity_id.clone(),
            created_ms: now_ms(),
            updated_ms: now_ms(),
            context,
            session,
        };
        let resume_state = envoix_storage::TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "resume.bin".to_string(),
            file_size: 8,
            chunk_size: 4,
            bytes_received: 4,
            next_chunk_index: 1,
            hash_bytes: 4,
            hash_checkpoint: None,
        };
        let other_state = envoix_storage::TransferResumeState {
            transfer_id: other_id.clone(),
            ..resume_state.clone()
        };
        durable_runtime().unwrap().block_on(async {
            RecordStore::new(&records_dir).save(&record).await.unwrap();
            LocalFileStorage::write_resume_state(&output_dir, &resume_state)
                .await
                .unwrap();
            LocalFileStorage::write_resume_state(&output_dir, &other_state)
                .await
                .unwrap();
        });
        let partial =
            LocalFileStorage::resumable_temp_path(&output_dir, "resume.bin", &transfer_id).unwrap();
        let other_partial =
            LocalFileStorage::resumable_temp_path(&output_dir, "resume.bin", &other_id).unwrap();
        std::fs::write(&partial, b"abcd").unwrap();
        std::fs::write(&other_partial, b"wxyz").unwrap();

        let (tx, rx) = channel();
        let restored = restore_durable_transfer(
            request.activity_id.clone(),
            records_dir.to_string_lossy().into_owned(),
            Arc::new(TestObserver(tx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        let paused = recv_activity_state(
            &rx,
            &request.activity_id,
            FfiTransferActivityState::Paused,
            Duration::from_secs(5),
        );
        assert_eq!(paused.failure_code, FfiFailureCode::NetworkLost);
        assert_eq!(paused.recovery_action, FfiRecoveryAction::Resume);
        assert!(paused.diagnostic_message.contains("app restart"));

        assert!(restored.remove());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let records =
                list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
            if records.is_empty() && !partial.exists() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "remove did not delete the durable record and exact partial"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(other_partial.exists());
        assert!(
            durable_runtime()
                .unwrap()
                .block_on(LocalFileStorage::read_resume_state(
                    &output_dir,
                    "resume.bin",
                    &other_id,
                ))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn durable_room_pause_resumes_from_initiating_side_only() {
        let _loopback_guard = lock_loopback_tests();
        let broker = start_test_broker();
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("durable-pause.bin");
        let payload = vec![0x6d; 32 * 1024 * 1024];
        std::fs::write(&source, &payload).unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "chunk_size = \"256K\"\n").unwrap();

        let settings = EnvoixRuntimeSettings {
            server_url: broker,
            relay_url: String::new(),
            config_path: config.to_string_lossy().into_owned(),
            ..EnvoixRuntimeSettings::default()
        };
        let code = "975310-durable-pause".to_string();
        let receive_records = dir.path().join("receive-records");
        let send_records = dir.path().join("send-records");

        let (rtx, rrx) = channel();
        let (mailbox_tx, mailbox_rx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_string_lossy().into_owned(),
            FfiTransferMode::Room,
        );
        receive_request.activity_id = "durable-pause-receive".to_string();
        receive_request.code = code.clone();
        let receiver = start_durable_transfer(
            settings.clone(),
            receive_request,
            receive_records.to_string_lossy().into_owned(),
            Arc::new(TestObserver(rtx)),
            Arc::new(TestMailbox(mailbox_tx)),
        )
        .unwrap();

        thread::sleep(Duration::from_millis(200));

        let (stx, srx) = channel();
        let (pause_tx, pause_rx) = channel();
        let send_observer = Arc::new(DurablePauseOnProgressObserver {
            messages: stx,
            session: Mutex::new(None),
            result: pause_tx,
            requested: std::sync::atomic::AtomicBool::new(false),
        });
        let mut send_request =
            FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
        send_request.activity_id = "durable-pause-send".to_string();
        send_request.code = code;
        let sender = start_durable_transfer(
            settings,
            send_request,
            send_records.to_string_lossy().into_owned(),
            send_observer.clone(),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        *send_observer.session.lock().unwrap() = Some(Arc::downgrade(&sender));

        assert!(
            pause_rx
                .recv_timeout(Duration::from_secs(20))
                .expect("progress should trigger a durable pause")
        );
        recv_activity_state(
            &srx,
            "durable-pause-send",
            FfiTransferActivityState::Paused,
            Duration::from_secs(20),
        );
        assert!(sender.resume());

        let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
        let (received, _, activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
        assert_eq!(sent, payload.len() as u64);
        assert_eq!(received, payload.len() as u64);
        assert_eq!(
            std::fs::read(output_dir.join("durable-pause.bin")).unwrap(),
            payload
        );
        assert!(
            activity.bytes_resumed > 0,
            "resumed transfer should report a retained prefix"
        );
        match mailbox_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("completed room receive should post a sealed receipt")
        {
            MailboxMsg::Post {
                activity_id,
                key,
                blob,
            } => {
                assert_eq!(activity_id, "durable-pause-receive");
                assert_eq!(key.len(), 64);
                assert!(!blob.is_empty());
            }
            MailboxMsg::Fetch { .. } => panic!("receiver should post, not fetch, a receipt"),
        }
        assert!(receiver.receipt_posted());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let records = durable_runtime()
                .unwrap()
                .block_on(RecordStore::new(&receive_records).load_all());
            if records
                .first()
                .is_some_and(|record| record.session.facts.proof_delivered)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "receipt POST acknowledgement was not persisted"
            );
            thread::sleep(Duration::from_millis(20));
        }

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn durable_unconfirmed_restore_completes_from_verified_mailbox_receipt() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let records_dir = dir.path().join("records");
        let source = dir.path().join("mailbox.bin");
        let payload = b"mailbox completion proof";
        std::fs::write(&source, payload).unwrap();
        let transfer_id = "transfer-mailbox-proof";
        let code = "864209-mailbox-proof";

        let mut request =
            FfiTransferRequest::send(source.to_string_lossy().into_owned(), FfiTransferMode::Room);
        request.activity_id = "mailbox-unconfirmed".to_string();
        request.code = code.to_string();
        request.broker = "ignored@127.0.0.1:9".to_string();
        let context =
            canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
        let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Send);
        machine.state = CanonicalState::Unconfirmed;
        machine.transfer_id = Some(transfer_id.to_string());
        machine.file_name = Some("mailbox.bin".to_string());
        machine.bytes = payload.len() as u64;
        machine.total = payload.len() as u64;
        let created_ms = now_ms();
        durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
                id: request.activity_id.clone(),
                created_ms,
                updated_ms: created_ms,
                context,
                session: machine,
            }))
            .unwrap();
        let receipt = envoix_storage::TransferReceipt {
            transfer_id: TransferId::new(transfer_id),
            file_name: "mailbox.bin".to_string(),
            file_size: payload.len() as u64,
            file_hash: blake3::hash(payload).to_hex().to_string(),
        };
        let blob = envoix_client::api::receipt::seal_receipt(transfer_id, code, &receipt).unwrap();

        let (activity_tx, activity_rx) = channel();
        let (mailbox_tx, mailbox_rx) = channel();
        let restored = restore_durable_transfer(
            request.activity_id.clone(),
            records_dir.to_string_lossy().into_owned(),
            Arc::new(TestObserver(activity_tx)),
            Arc::new(TestMailbox(mailbox_tx)),
        )
        .unwrap();
        let fetch = mailbox_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("restored unconfirmed transfer should poll the mailbox");
        match fetch {
            MailboxMsg::Fetch { activity_id, key } => {
                assert_eq!(activity_id, request.activity_id);
                assert_eq!(
                    key,
                    envoix_client::api::receipt::receipt_mailbox_key(transfer_id)
                );
            }
            MailboxMsg::Post {
                activity_id,
                key,
                blob,
            } => panic!(
                "send should fetch, not post, a receipt: {activity_id} {key} {} bytes",
                blob.len()
            ),
        }
        assert!(restored.receipt_response(blob));

        let completed = recv_activity_state(
            &activity_rx,
            &request.activity_id,
            FfiTransferActivityState::Completed,
            Duration::from_secs(5),
        );
        assert_eq!(completed.bytes_transferred, payload.len() as u64);
        let history =
            list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].state, FfiTransferActivityState::Completed);
        assert_eq!(history[0].created_at_ms, created_ms);
    }

    #[test]
    fn durable_staged_receive_is_not_completed_until_native_publication() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let records_dir = dir.path().join("records");
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let staged_file = staging.join("published.bin");
        std::fs::write(&staged_file, b"published bytes").unwrap();

        let mut request = FfiTransferRequest::receive(
            staging.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        request.activity_id = "awaiting-publication".to_string();
        request.publication_required = true;
        let context =
            canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
        let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Receive);
        machine.state = CanonicalState::AwaitingPublication;
        machine.publication_required = true;
        machine.transfer_id = Some("transfer-publication".to_string());
        machine.file_name = Some("published.bin".to_string());
        machine.bytes = 15;
        machine.total = 15;
        machine.completed_file_path = Some(staged_file.to_string_lossy().into_owned());
        let timestamp = now_ms();
        durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
                id: request.activity_id.clone(),
                created_ms: timestamp,
                updated_ms: timestamp,
                context,
                session: machine,
            }))
            .unwrap();

        let (tx, rx) = channel();
        let restored = restore_durable_transfer(
            request.activity_id.clone(),
            records_dir.to_string_lossy().into_owned(),
            Arc::new(TestObserver(tx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        let publishing = recv_activity_state(
            &rx,
            &request.activity_id,
            FfiTransferActivityState::Publishing,
            Duration::from_secs(5),
        );
        assert_eq!(
            publishing.completed_file_path,
            staged_file.to_string_lossy()
        );
        assert_eq!(publishing.completed_at_ms, 0);

        let final_uri = "content://downloads/envoix/published.bin";
        assert!(restored.publication_succeeded(final_uri.to_string()));
        let completed = recv_activity_state(
            &rx,
            &request.activity_id,
            FfiTransferActivityState::Completed,
            Duration::from_secs(5),
        );
        assert_eq!(completed.completed_file_path, final_uri);
        assert!(completed.completed_at_ms > 0);
        let history =
            list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
        assert_eq!(history[0].state, FfiTransferActivityState::Completed);
        assert_eq!(history[0].completed_file_path, final_uri);
    }

    #[test]
    fn canceling_durable_publication_discards_only_its_staged_artifacts() {
        let _loopback_guard = lock_loopback_tests();
        let dir = tempfile::tempdir().unwrap();
        let records_dir = dir.path().join("records");
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let staged_file = staging.join("cancel.bin");
        let unrelated_file = staging.join("keep.bin");
        let payload = b"cancel staged bytes";
        std::fs::write(&staged_file, payload).unwrap();
        std::fs::write(&unrelated_file, b"keep staged bytes").unwrap();

        let transfer_id = TransferId::new("transfer-cancel-publication");
        let unrelated_id = TransferId::new("transfer-unrelated-publication");
        durable_runtime().unwrap().block_on(async {
            LocalFileStorage::write_receipt(
                &staging,
                &envoix_storage::TransferReceipt {
                    transfer_id: transfer_id.clone(),
                    file_name: "cancel.bin".into(),
                    file_size: payload.len() as u64,
                    file_hash: "cancel-hash".into(),
                },
            )
            .await
            .unwrap();
            LocalFileStorage::write_receipt(
                &staging,
                &envoix_storage::TransferReceipt {
                    transfer_id: unrelated_id.clone(),
                    file_name: "keep.bin".into(),
                    file_size: 17,
                    file_hash: "keep-hash".into(),
                },
            )
            .await
            .unwrap();
        });

        let mut request = FfiTransferRequest::receive(
            staging.to_string_lossy().into_owned(),
            FfiTransferMode::ShowInvite,
        );
        request.activity_id = "cancel-awaiting-publication".to_string();
        request.publication_required = true;
        let context =
            canonical_context_for_request(&EnvoixRuntimeSettings::default(), &request).unwrap();
        let mut machine = envoix_client::api::machine::Session::new(TransferDirection::Receive);
        machine.state = CanonicalState::AwaitingPublication;
        machine.publication_required = true;
        machine.transfer_id = Some(transfer_id.to_string());
        machine.file_name = Some("cancel.bin".to_string());
        machine.bytes = payload.len() as u64;
        machine.total = payload.len() as u64;
        machine.completed_file_path = Some(staged_file.to_string_lossy().into_owned());
        let timestamp = now_ms();
        durable_runtime()
            .unwrap()
            .block_on(RecordStore::new(&records_dir).save(&TransferRecord {
                id: request.activity_id.clone(),
                created_ms: timestamp,
                updated_ms: timestamp,
                context,
                session: machine,
            }))
            .unwrap();

        let (tx, rx) = channel();
        let restored = restore_durable_transfer(
            request.activity_id.clone(),
            records_dir.to_string_lossy().into_owned(),
            Arc::new(TestObserver(tx)),
            Arc::new(NoopMailbox),
        )
        .unwrap();
        recv_activity_state(
            &rx,
            &request.activity_id,
            FfiTransferActivityState::Publishing,
            Duration::from_secs(5),
        );

        assert!(restored.cancel());
        let canceled = recv_activity_state(
            &rx,
            &request.activity_id,
            FfiTransferActivityState::Canceled,
            Duration::from_secs(5),
        );
        assert_eq!(canceled.failure_code, FfiFailureCode::UserCanceled);
        assert!(!staged_file.exists());
        assert!(unrelated_file.exists());
        durable_runtime().unwrap().block_on(async {
            assert!(
                LocalFileStorage::read_receipt(&staging, "cancel.bin")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                LocalFileStorage::read_receipt(&staging, "keep.bin")
                    .await
                    .unwrap()
                    .unwrap()
                    .transfer_id,
                unrelated_id
            );
        });
        let history =
            list_durable_transfer_records(records_dir.to_string_lossy().into_owned()).unwrap();
        assert_eq!(history[0].state, FfiTransferActivityState::Canceled);
    }

    #[test]
    fn ffi_room_loopback() {
        let _loopback_guard = lock_loopback_tests();
        let broker = start_test_broker();

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

    #[test]
    fn ffi_room_pause_resumes_from_one_side_and_preserves_file() {
        let _loopback_guard = lock_loopback_tests();
        let broker = start_test_broker();
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("received");
        std::fs::create_dir_all(&output_dir).unwrap();
        let source = dir.path().join("pause-resume.bin");
        let payload = vec![0x5a; 32 * 1024 * 1024];
        std::fs::write(&source, &payload).unwrap();

        let settings = EnvoixRuntimeSettings {
            server_url: broker,
            relay_url: String::new(),
            ..EnvoixRuntimeSettings::default()
        };
        let code = "246810-cobalt-bridge".to_string();
        let receiver_id = "pause-receiver".to_string();
        let sender_id = "pause-sender".to_string();

        let receiver = Arc::new(EnvoixSession::new_with_settings(settings.clone()));
        let (rtx, rrx) = channel();
        let mut receive_request = FfiTransferRequest::receive(
            output_dir.to_str().unwrap().to_string(),
            FfiTransferMode::Room,
        );
        receive_request.activity_id = receiver_id;
        receive_request.code = code.clone();
        receiver
            .start_transfer(receive_request, Arc::new(TestObserver(rtx)))
            .unwrap();

        thread::sleep(Duration::from_millis(200));

        let sender = Arc::new(EnvoixSession::new_with_settings(settings));
        let (stx, srx) = channel();
        let (pause_tx, pause_rx) = channel();
        let observer = PauseOnProgressObserver {
            messages: stx,
            session: Arc::downgrade(&sender),
            activity_id: sender_id.clone(),
            pause_result: pause_tx,
            requested: std::sync::atomic::AtomicBool::new(false),
        };
        let mut send_request =
            FfiTransferRequest::send(source.to_str().unwrap().to_string(), FfiTransferMode::Room);
        send_request.activity_id = sender_id.clone();
        send_request.code = code;
        sender
            .start_transfer(send_request, Arc::new(observer))
            .unwrap();

        assert!(
            pause_rx
                .recv_timeout(Duration::from_secs(20))
                .expect("progress should trigger a pause")
        );
        recv_activity_state(
            &srx,
            &sender_id,
            FfiTransferActivityState::Paused,
            Duration::from_secs(20),
        );
        assert!(sender.resume_activity(sender_id));

        let (sent, _) = recv_completed(&srx, Duration::from_secs(45));
        let (received, _, activity) = recv_completed_activity(&rrx, Duration::from_secs(45));
        assert_eq!(sent, payload.len() as u64);
        assert_eq!(received, payload.len() as u64);
        assert!(!activity.completed_file_path.is_empty());
        assert_eq!(
            std::fs::read(output_dir.join("pause-resume.bin")).unwrap(),
            payload
        );
    }
}
