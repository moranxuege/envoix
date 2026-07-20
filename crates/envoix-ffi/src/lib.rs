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

mod session;
pub(crate) use session::*;
