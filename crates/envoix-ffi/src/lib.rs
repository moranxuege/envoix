//! UniFFI bridge for the canonical Manifest v2 job and session APIs.

use std::path::Path;

use envoix_client::PeerDescriptor;
use envoix_client::api::{Client, Invite, PathPolicy, PeerSource, Role, TransferOptions};
use envoix_rendezvous_iroh::generate_code;

uniffi::setup_scaffolding!();

mod manifest_v2_job;
pub use manifest_v2_job::*;
mod manifest_v2_session;
pub use manifest_v2_session::*;

const DEFAULT_RENDEZVOUS_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
const DEFAULT_RELAY_URL: &str = "https://envoix.chkxwlyh.us:8444";
const INVITE_TTL_SECS: u64 = 300;
const ENVOIX_FFI_API_VERSION: u32 = 4;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EnvoixError {
    #[error("{reason}")]
    Operation { reason: String },
}

pub(crate) fn op_err(error: impl std::fmt::Display) -> EnvoixError {
    EnvoixError::Operation {
        reason: error.to_string(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct EnvoixRuntimeSettings {
    pub concurrent_transfers: bool,
    pub language: String,
    pub server_url: String,
    pub relay_url: String,
    pub config_path: String,
    pub speed_limit_mbps: u64,
}

impl Default for EnvoixRuntimeSettings {
    fn default() -> Self {
        Self {
            concurrent_transfers: true,
            language: "en".into(),
            server_url: String::new(),
            relay_url: String::new(),
            config_path: String::new(),
            speed_limit_mbps: 0,
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
    pub code: String,
    pub payload: String,
    pub broker: String,
    pub relay: String,
    pub role: FfiInviteRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferDirection {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferMode {
    Manual,
    Invite,
    ShowManual,
    ShowInvite,
    Mdns,
    Room,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPathPolicy {
    Auto,
    RelayOnly,
    DirectOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRendezvousPlan {
    pub use_room: bool,
    pub use_mdns: bool,
    pub internet_available: bool,
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

/// Session-only inputs. Source paths and destination paths live in the job or
/// destination request, never in this rendezvous record.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferRequest {
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub peer_descriptor: String,
    pub invite: String,
    pub code: String,
    pub token: String,
    pub broker: String,
    pub relay: String,
    pub config_path: String,
    pub path_policy: FfiPathPolicy,
    pub rendezvous: FfiRendezvousPlan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestV2Phase {
    Pairing,
    Connecting,
    Transferring,
    Verifying,
    Saving,
    WaitingForReceiverSave,
    FinalizingDelivery,
    Delivered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCode {
    UserCanceled,
    NetworkLost,
    AuthenticationFailed,
    UnsupportedFeature,
    InternalError,
    SenderSourceUnavailable,
    SenderPermissionLost,
    SenderSourceChanged,
    SenderItemRemoved,
    SenderCanceled,
    ProtocolOrIntegrityFailure,
    ReceiverSpaceInsufficient,
    ReceiverDestinationDecisionRequired,
    ReceiverDestinationUnavailable,
    ReceiverSaveFailed,
    ReceiverReusedObjectLost,
    ReceiverFinalizationOutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Unsupported,
    Internal,
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
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferFailure {
    pub code: FfiFailureCode,
    pub category: FfiFailureCategory,
    pub phase: FfiFailurePhase,
    pub origin: FfiFailureOrigin,
    pub direction: FfiTransferDirection,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub user_message_key: String,
    pub diagnostic_message: String,
}

#[uniffi::export(with_foreign)]
pub trait TransferObserver: Send + Sync {
    fn on_invite_ready(&self, invite: String);
    fn on_started(&self, item_count: u32, total_bytes: u64);
    fn on_phase(&self, phase: FfiManifestV2Phase);
    fn on_progress(&self, transferred: u64, total: u64);
    fn on_completed(&self, bytes: u64);
    fn on_transfer_failed(&self, failure: FfiTransferFailure);
    fn on_diagnostic(&self, message: String);
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiCoreInfo {
    pub ffi_api_version: u32,
    pub core_version: String,
    pub capabilities: Vec<String>,
}

#[uniffi::export]
pub fn envoix_core_info() -> FfiCoreInfo {
    FfiCoreInfo {
        ffi_api_version: ENVOIX_FFI_API_VERSION,
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![
            "canonical_transfer_job_v2".into(),
            "manifest_v2_session".into(),
            "paged_transfer_inventory_v2".into(),
            "delivery_proof_v2".into(),
        ],
    }
}

#[uniffi::export]
pub fn generate_room_code() -> Result<String, EnvoixError> {
    generate_code(2).map_err(op_err)
}

#[uniffi::export]
pub fn make_pairing_invite(
    role: FfiInviteRole,
    broker: String,
    relay: String,
) -> Result<FfiPairingInvite, EnvoixError> {
    let broker = non_empty(&broker).unwrap_or(DEFAULT_RENDEZVOUS_BROKER);
    let relay = non_empty(&relay).or(Some(DEFAULT_RELAY_URL));
    let mut invite = Invite::room(broker.to_string(), relay.map(str::to_string)).map_err(op_err)?;
    invite = match role {
        FfiInviteRole::Send => invite.with_role(Role::Send),
        FfiInviteRole::Receive => invite.with_role(Role::Receive),
        FfiInviteRole::Unknown => invite,
    };
    Ok(project_invite(&invite))
}

#[uniffi::export]
pub fn parse_pairing_invite(input: String) -> Result<FfiPairingInvite, EnvoixError> {
    let invite = Invite::parse(&input).map_err(op_err)?;
    Ok(project_invite(&invite))
}

fn project_invite(invite: &Invite) -> FfiPairingInvite {
    FfiPairingInvite {
        code: invite.code().to_string(),
        payload: invite.payload(),
        broker: invite.broker().unwrap_or_default().to_string(),
        relay: invite.relay().unwrap_or_default().to_string(),
        role: match invite.role() {
            Some(Role::Send) => FfiInviteRole::Send,
            Some(Role::Receive) => FfiInviteRole::Receive,
            None => FfiInviteRole::Unknown,
        },
    }
}

pub(crate) struct RouteAttempt {
    pub source: PeerSource,
    pub path_policy_override: Option<PathPolicy>,
}

pub(crate) fn build_client_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Client, EnvoixError> {
    let path = non_empty(&request.config_path)
        .or_else(|| non_empty(&settings.config_path))
        .map(Path::new);
    Client::from_runtime_sources(path).map_err(op_err)
}

pub(crate) fn transfer_options_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    path_override: Option<PathPolicy>,
) -> Result<TransferOptions, EnvoixError> {
    let path = path_override.unwrap_or(match request.path_policy {
        FfiPathPolicy::Auto => PathPolicy::Auto,
        FfiPathPolicy::RelayOnly => PathPolicy::RelayOnly,
        FfiPathPolicy::DirectOnly => PathPolicy::DirectOnly,
    });
    let relay = non_empty(&request.relay)
        .or_else(|| non_empty(&settings.relay_url))
        .map(str::to_string);
    if path == PathPolicy::RelayOnly && relay.is_none() {
        return Err(EnvoixError::Operation {
            reason: "relay-only routing requires a relay URL".into(),
        });
    }
    Ok(TransferOptions {
        relay,
        path,
        listen_addrs: None,
    })
}

pub(crate) fn peer_sources_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Vec<RouteAttempt>, EnvoixError> {
    let single = |source| {
        Ok(vec![RouteAttempt {
            source,
            path_policy_override: None,
        }])
    };
    match request.mode {
        FfiTransferMode::Manual => single(PeerSource::Manual {
            peer: required(&request.peer_descriptor, "peer_descriptor")?
                .parse::<PeerDescriptor>()
                .map_err(op_err)?,
            token: required(&request.token, "token")?.to_string(),
        }),
        FfiTransferMode::Invite => single(PeerSource::Invite {
            invite: required(&request.invite, "invite")?.to_string(),
        }),
        FfiTransferMode::ShowManual => single(PeerSource::ShowManual {
            token: non_empty(&request.token).map(str::to_string),
        }),
        FfiTransferMode::ShowInvite => single(PeerSource::ShowInvite {
            ttl_secs: INVITE_TTL_SECS,
            token: non_empty(&request.token).map(str::to_string),
        }),
        FfiTransferMode::Mdns => single(PeerSource::Mdns {
            token: non_empty(&request.token).map(str::to_string),
        }),
        FfiTransferMode::Room => {
            let code = required(&request.code, "code")?.to_string();
            let broker = non_empty(&request.broker)
                .or_else(|| non_empty(&settings.server_url))
                .unwrap_or(DEFAULT_RENDEZVOUS_BROKER)
                .to_string();
            let mut routes = Vec::new();
            if request.rendezvous.use_room && request.rendezvous.internet_available {
                routes.push(RouteAttempt {
                    source: PeerSource::Room {
                        code: code.clone(),
                        broker,
                    },
                    path_policy_override: None,
                });
            }
            if request.rendezvous.use_mdns {
                routes.push(RouteAttempt {
                    source: PeerSource::Mdns { token: Some(code) },
                    path_policy_override: Some(PathPolicy::Auto),
                });
            }
            if routes.is_empty() {
                return Err(EnvoixError::Operation {
                    reason: "room request has no enabled rendezvous route".into(),
                });
            }
            Ok(routes)
        }
    }
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, EnvoixError> {
    non_empty(value).ok_or_else(|| EnvoixError::Operation {
        reason: format!("{field} must not be empty"),
    })
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
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
        // SAFETY: ndk-context requires the JavaVM and application-context raw
        // pointers supplied by Android. `ctx` is intentionally retained for
        // the process lifetime immediately below, so both pointers stay valid.
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer() as *mut _,
                ctx.as_obj().as_raw() as *mut _,
            );
        }
        // ndk-context stores this pointer globally and exposes no ownership API.
        std::mem::forget(ctx);
    }

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
                if let Ok(line) = std::str::from_utf8(&self.0) {
                    log_line(line.trim_end());
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
