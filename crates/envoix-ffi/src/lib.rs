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
        record::{EXTERNAL_RECORD_ID_KEY, RecordStore, TransferRecord, stable_record_id},
    },
};
use envoix_qr::QrInvitePayload;
use envoix_rendezvous_iroh::generate_code;
use envoix_storage::LocalFileStorage;
use envoix_types::TransferId;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{mpsc, oneshot};

uniffi::setup_scaffolding!();

mod manifest;
pub use manifest::*;

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
/// Version of the additive native API contract exposed by this crate.
const ENVOIX_FFI_API_VERSION: u32 = 3;
const NATIVE_PUBLICATION_EXTRAS_KEY: &str = "native_publication";
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

/// Reports the native bridge version and optional capabilities at runtime.
#[uniffi::export]
pub fn envoix_core_info() -> FfiCoreInfo {
    FfiCoreInfo {
        ffi_api_version: ENVOIX_FFI_API_VERSION,
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![
            "activity_actions_v1".to_string(),
            "durable_activity_sequence_v1".to_string(),
            "native_publication_v1".to_string(),
            "durable_publication_recovery_v1".to_string(),
            "per_session_receipt_endpoint_v1".to_string(),
            "manifest_activity_v1".to_string(),
            "manifest_selection_builder_v1".to_string(),
            "manifest_diagnostic_events_v1".to_string(),
        ],
    }
}

/// Projects canonical lifecycle state into native UI action availability.
#[uniffi::export]
pub fn transfer_activity_actions(record: FfiTransferActivityRecord) -> FfiTransferActivityActions {
    FfiTransferActivityActions::for_record(&record)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
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

/// Runtime identity used to detect a stale but otherwise loadable native core.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiCoreInfo {
    pub ffi_api_version: u32,
    pub core_version: String,
    pub capabilities: Vec<String>,
}

/// Canonical action policy for an Activity card.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityActions {
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub can_delete: bool,
    pub is_finalizing: bool,
}

impl FfiTransferActivityActions {
    fn for_record(activity: &FfiTransferActivityRecord) -> Self {
        let is_finalizing = is_finalizing_activity(activity);
        let can_pause = can_pause_durable_activity(activity);
        let can_resume = matches!(
            activity.state,
            FfiTransferActivityState::Paused | FfiTransferActivityState::Unconfirmed
        ) || matches!(activity.state, FfiTransferActivityState::Failed)
            && activity.retryable
            || matches!(activity.state, FfiTransferActivityState::Publishing) && activity.retryable;
        let can_cancel = matches!(
            activity.state,
            FfiTransferActivityState::Queued
                | FfiTransferActivityState::Binding
                | FfiTransferActivityState::WaitingForPeer
                | FfiTransferActivityState::Pairing
                | FfiTransferActivityState::Connecting
                | FfiTransferActivityState::Transferring
                | FfiTransferActivityState::Verifying
                | FfiTransferActivityState::Unconfirmed
                | FfiTransferActivityState::Paused
        ) && !is_finalizing
            || matches!(activity.state, FfiTransferActivityState::Publishing) && activity.retryable;
        let can_delete = matches!(
            activity.state,
            FfiTransferActivityState::Completed
                | FfiTransferActivityState::Failed
                | FfiTransferActivityState::Canceled
        );

        Self {
            can_pause,
            can_resume,
            can_cancel,
            can_delete,
            is_finalizing,
        }
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiFailureOrigin {
    Local,
    Peer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
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

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Record)]
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

/// Frontend-owned destination for publishing a staged receive.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Record)]
pub struct FfiNativePublicationTarget {
    pub destination_path: String,
    pub bookmark: Vec<u8>,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
struct PersistedNativePublication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<FfiNativePublicationTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FfiTransferFailure>,
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

/// Versioned native receipt courier that receives the endpoint frozen in the
/// durable session. `None` is reserved for records created before that field
/// existed, allowing the frontend to use its current configured endpoint.
#[uniffi::export(with_foreign)]
pub trait MailboxObserverV2: Send + Sync {
    fn on_fetch_receipt(&self, activity_id: String, key: String, server: Option<String>);
    fn on_post_receipt(
        &self,
        activity_id: String,
        key: String,
        blob: Vec<u8>,
        server: Option<String>,
    );
}

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

mod durable;
pub use durable::*;
pub(crate) use durable::{can_pause_durable_activity, durable_runtime};
#[cfg(test)]
use durable::{can_cancel_durable_activity, can_resume_durable_activity};

mod session;
pub use session::EnvoixSession;
pub(crate) use session::*;
