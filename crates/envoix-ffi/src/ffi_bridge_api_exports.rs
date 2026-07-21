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
