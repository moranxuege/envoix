//! JNI bridge that runs an Envoix transfer **in the Android app's process** and
//! streams its event JSON to a Kotlin callback.
//!
//! Running in-process (rather than exec'ing the CLI as a subprocess) is required
//! on Android: the network stack reaches the platform through the JVM - DNS
//! (`hickory` → `ConnectivityManager`), interface enumeration (`netdev`), and the
//! TLS trust store (`rustls-platform-verifier`) all read the Android context via
//! `ndk_context`. [`initContext`] wires the VM + app context in once; without it
//! those crates panic ("android context was not initialized").

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use envoix_client::TransferDirection;
use envoix_client::api::driver::{ClientContext, SessionContext, SessionParams, TransferSession};
use envoix_client::api::{Invite, PeerSource, Role, TransferOptions};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The durable record store (roadmap #5), set once by `initRecords`.
static RECORDS: OnceLock<envoix_client::api::record::RecordStore> = OnceLock::new();

fn record_for(id: i64) -> Option<(envoix_client::api::record::RecordStore, u64)> {
    RECORDS.get().map(|s| (s.clone(), id as u64))
}

/// Live transfer sessions (the state-machine driver), keyed by the Kotlin id.
type SessionMap = HashMap<i64, envoix_client::api::driver::TransferSession>;
static SESSIONS: OnceLock<Mutex<SessionMap>> = OnceLock::new();

fn sessions() -> &'static Mutex<SessionMap> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Intents addressed to a record id whose session is not registered yet (the
/// restore-then-intent race: Kotlin fires "resume" right after asking for a
/// restore, and the registration may not have happened). Queued here, drained
/// in order on registration, cleared on destroy. Lock order is always
/// sessions() before pending_intents().
static PENDING_INTENTS: OnceLock<Mutex<HashMap<i64, Vec<String>>>> = OnceLock::new();
/// Per-id bound; an id nobody registers must not accumulate garbage.
const MAX_PENDING_INTENTS: usize = 8;

fn pending_intents() -> &'static Mutex<HashMap<i64, Vec<String>>> {
    PENDING_INTENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Dispatch one intent string to a live session (shared by the direct path
/// and the pending-queue drain).
fn route_intent(id: i64, session: &envoix_client::api::driver::TransferSession, intent: &str) {
    match intent {
        "pause" => session.pause(),
        "resume" => session.resume(),
        "cancel" => session.cancel(),
        "reverify" => session.serve_reverify(),
        "receipt_posted" => session.receipt_posted(),
        _ => tracing::warn!(id, intent, "sessionIntent: unknown intent"),
    }
}

/// Register a freshly created/restored session, then drain any intents that
/// arrived before it existed. Refuses to replace a live session: silently
/// swapping the map entry would orphan a running driver (the caller's new
/// session is dropped, which detaches without touching the wire).
fn register_session(id: i64, session: envoix_client::api::driver::TransferSession) -> bool {
    let Ok(mut map) = sessions().lock() else {
        return false;
    };
    if map.contains_key(&id) {
        tracing::warn!(id, "session already live; duplicate start/restore ignored");
        return false;
    }
    map.insert(id, session);
    let queued = pending_intents()
        .lock()
        .ok()
        .and_then(|mut p| p.remove(&id))
        .unwrap_or_default();
    if let Some(session) = map.get(&id) {
        for intent in queued {
            tracing::info!(id, intent, "applying queued intent");
            route_intent(id, session, &intent);
        }
    }
    true
}

/// VM + Kotlin log sink for [`Java_dev_envoix_app_Native_initLogging`]. The
/// `tracing` subscriber below forwards every formatted line to `sink.log(String)`.
static LOG_VM: OnceLock<JavaVM> = OnceLock::new();
static LOG_SINK: OnceLock<GlobalRef> = OnceLock::new();

/// The always-on baseline: envoix internals + iroh's connection story.
const DEFAULT_LOG: &str = "envoix=debug,iroh=info,warn";

/// Handle to the reloadable log filter, so the app can raise/lower verbosity at
/// runtime (the `-vv` dev toggle) without restarting.
type LogReload =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;
static LOG_RELOAD: OnceLock<LogReload> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
    })
}

/// Wire the Android VM + application context into `ndk_context`. Must be called
/// once (e.g. from `Application.onCreate`) before any transfer runs.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_initContext(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    let Ok(vm) = env.get_java_vm() else {
        tracing::warn!("initContext: failed to get JavaVM");
        return;
    };
    let Ok(ctx) = env.new_global_ref(&context) else {
        tracing::warn!("initContext: failed to create global context ref");
        return;
    };
    unsafe {
        ndk_context::initialize_android_context(
            vm.get_java_vm_pointer() as *mut _,
            ctx.as_obj().as_raw() as *mut _,
        );
    }
    // Keep the global ref alive for the whole process, since ndk_context holds a
    // raw pointer to it.
    std::mem::forget(ctx);
}

#[allow(clippy::too_many_arguments)]
/// Generate a room invite for `role` ("send"/"receive"). Returns JSON
/// `{"code":..,"payload":..}` (the payload is the QR string), or `{"error":..}`.
/// `relay` may be empty.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_generateInvite(
    mut env: JNIEnv,
    _class: JClass,
    role: JString,
    broker: JString,
    relay: JString,
) -> jni::sys::jstring {
    let role = if jstr(&mut env, &role) == "send" {
        Role::Send
    } else {
        Role::Receive
    };
    let broker = jstr(&mut env, &broker);
    let relay = jstr(&mut env, &relay);
    let relay = (!relay.is_empty()).then_some(relay);
    let json = match Invite::room(broker, relay) {
        Ok(inv) => {
            let inv = inv.with_role(role);
            format!(
                r#"{{"code":{},"payload":{}}}"#,
                json_str(inv.code()),
                json_str(&inv.payload())
            )
        }
        Err(e) => format!(r#"{{"error":{}}}"#, json_str(&e.to_string())),
    };
    to_jstring(&mut env, &json)
}

/// Parse a typed code or a scanned `envoix://` payload. Returns JSON
/// `{"code":..,"broker":..,"relay":..,"role":..}`, or `{"error":..}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_parseInvite(
    mut env: JNIEnv,
    _class: JClass,
    input: JString,
) -> jni::sys::jstring {
    let input = jstr(&mut env, &input);
    let json = match Invite::parse(&input) {
        Ok(inv) => {
            let role = match inv.role() {
                Some(Role::Send) => "\"send\"",
                Some(Role::Receive) => "\"receive\"",
                None => "null",
            };
            format!(
                r#"{{"code":{},"broker":{},"relay":{},"role":{}}}"#,
                json_str(inv.code()),
                opt_json(inv.broker()),
                opt_json(inv.relay()),
                role,
            )
        }
        Err(e) => format!(r#"{{"error":{}}}"#, json_str(&e.to_string())),
    };
    to_jstring(&mut env, &json)
}

fn to_jstring(env: &mut JNIEnv, s: &str) -> jni::sys::jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to allocate Java string");
            std::ptr::null_mut()
        })
}

fn error_jstring(
    env: &mut JNIEnv,
    context: &str,
    error: impl std::fmt::Display,
) -> jni::sys::jstring {
    let message = format!("{context}: {error}");
    tracing::warn!(%message);
    to_jstring(env, &format!(r#"{{"error":{}}}"#, json_str(&message)))
}

fn opt_json(s: Option<&str>) -> String {
    s.map(json_str).unwrap_or_else(|| "null".to_string())
}

/// Everything one native transfer needs, bundled so the JNI entry point and
/// Split a comma-joined FFI config field into trimmed, non-empty entries.
/// Commas never appear in CIDR prefixes, so this round-trips the Kotlin lists.
fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|s| s.into()).unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to read Java string");
        String::new()
    })
}

/// Call `callback.onEvent(json)`, attaching this thread to the JVM as needed.
fn emit(vm: &JavaVM, cb: &GlobalRef, json: &str) {
    let Ok(mut env) = vm.attach_current_thread() else {
        tracing::warn!("failed to attach thread to JVM for callback");
        return;
    };
    if let Ok(js) = env.new_string(json) {
        let _ = env.call_method(
            cb,
            "onEvent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&js)],
        );
    } else {
        tracing::warn!("failed to allocate callback JSON string");
    }
}

/// Install a `tracing` subscriber that forwards every formatted log line to the
/// Kotlin `sink.log(String)`, so the app can show/copy the core's logs - the same
/// stream the CLI prints with `-v`. Safe to call once; later calls no-op. The
/// filter defaults to `envoix=debug,iroh=info,warn` (captures iroh's connection
/// story, not just warnings); override with the `ENVOIX_LOG` env.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_initLogging(
    env: JNIEnv,
    _class: JClass,
    sink: JObject,
) {
    let Ok(vm) = env.get_java_vm() else {
        tracing::warn!("initLogging: failed to get JavaVM");
        return;
    };
    let Ok(sink) = env.new_global_ref(&sink) else {
        tracing::warn!("initLogging: failed to create global log sink ref");
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
        .with(RoomTag) // must precede fmt: it hands the room to the writer
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
        // Route Rust panics into the tracing sink so the message reaches the app
        // log (and its on-disk copy) — a native abort otherwise leaves the panic
        // text only in logcat / the tombstone's "Abort message". Chain the default
        // hook so the native tombstone is still produced.
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(target: "envoix", "core panic: {info}");
            default(info);
        }));
    }
}

/// Change the log filter at runtime (the dev-mode `-vv` toggle). `spec` is an
/// env-filter directive, e.g. `envoix=trace,iroh=debug` for verbose or
/// `envoix=debug,iroh=info,warn` for the baseline. Invalid specs are ignored.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_setLogLevel(
    mut env: JNIEnv,
    _class: JClass,
    spec: JString,
) {
    let Ok(spec) = env.get_string(&spec) else {
        tracing::warn!("setLogLevel: failed to read log filter string");
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

/// Forward one formatted `tracing` line to `sink.log(...)`.
fn log_line(line: &str) {
    let (Some(vm), Some(sink)) = (LOG_VM.get(), LOG_SINK.get()) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    // The room captured by RoomTag for THIS event (same thread, synchronous).
    let room = CURRENT_ROOM.with(|r| r.borrow_mut().take());
    let room_obj = match room.as_deref().map(|r| env.new_string(r)) {
        Some(Ok(js)) => js,
        _ => jni::objects::JString::default(),
    };
    if let Ok(js) = env.new_string(line) {
        let _ = env.call_method(
            sink,
            "log",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&room_obj), JValue::Object(&js)],
        );
    }
}

thread_local! {
    /// Handoff from [`RoomTag`] to [`log_line`]: the `room` span field of the
    /// event currently being formatted (fmt writes synchronously after us).
    static CURRENT_ROOM: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// The `room` value recorded on a span (see docs/observability.md: room and
/// transfer_id are span fields on every line — this extracts them
/// STRUCTURALLY, replacing the Kotlin-side regex on formatted text).
struct RoomField(String);

/// Captures `room` at span creation AND on later `Span::record` calls (the
/// transfer span records it once known), then tags each event with the
/// nearest enclosing room.
struct RoomTag;

struct RoomVisitor(Option<String>);

impl tracing::field::Visit for RoomVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "room" {
            self.0 = Some(value.trim_matches('"').to_string());
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "room" {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }
}

impl<S> tracing_subscriber::layer::Layer<S> for RoomTag
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = RoomVisitor(None);
        attrs.record(&mut visitor);
        if let (Some(room), Some(span)) = (visitor.0, ctx.span(id)) {
            span.extensions_mut().replace(RoomField(room));
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = RoomVisitor(None);
        values.record(&mut visitor);
        if let (Some(room), Some(span)) = (visitor.0, ctx.span(id)) {
            span.extensions_mut().replace(RoomField(room));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let room = ctx.event_scope(event).and_then(|scope| {
            // innermost-first iteration: the first hit is the nearest room
            scope
                .filter_map(|span| span.extensions().get::<RoomField>().map(|r| r.0.clone()))
                .next()
        });
        CURRENT_ROOM.with(|r| *r.borrow_mut() = room);
    }
}

/// A `MakeWriter` whose per-event buffer ships its line to the Kotlin sink on drop.
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

/// Minimal JSON string encoding for the synthetic error event.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn failed_snapshot(reason: &str) -> String {
    format!(
        r#"{{"notice":"snapshot","seq":1,"state":"failed","reason":{}}}"#,
        json_str(reason)
    )
}

fn emit_failed_snapshot(vm: &JavaVM, cb: &GlobalRef, context: &str, error: impl std::fmt::Display) {
    let reason = format!("{context}: {error}");
    tracing::warn!(%reason);
    emit(vm, cb, &failed_snapshot(&reason));
}

fn java_vm_or_log(env: &JNIEnv, context: &str) -> Option<JavaVM> {
    match env.get_java_vm() {
        Ok(vm) => Some(vm),
        Err(error) => {
            tracing::warn!(%error, "{context}: failed to get JavaVM");
            None
        }
    }
}

fn callback_or_log(env: &JNIEnv, callback: &JObject, context: &str) -> Option<GlobalRef> {
    match env.new_global_ref(callback) {
        Ok(callback) => Some(callback),
        Err(error) => {
            tracing::warn!(%error, "{context}: failed to create callback global ref");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Session API (the state-machine driver; replaces runTransfer in step C).
// ---------------------------------------------------------------------------

/// One notice as JSON for the Kotlin side.
fn notice_json(notice: envoix_client::api::driver::SessionNotice) -> String {
    use base64::Engine;
    use envoix_client::api::driver::SessionNotice as N;
    match notice {
        N::Snapshot(snapshot) => {
            let path = snapshot.session.path.as_ref().map(|p| p.to_string());
            let mut value = serde_json::to_value(&snapshot).unwrap_or_default();
            if let Some(map) = value.as_object_mut() {
                map.insert("notice".into(), "snapshot".into());
                // The frontend wants a display string, not the enum encoding.
                map.insert(
                    "path".into(),
                    path.map(Into::into).unwrap_or(serde_json::Value::Null),
                );
            }
            value.to_string()
        }
        N::FetchReceipt { key, server } => format!(
            r#"{{"notice":"fetch_receipt","key":{},"server":{}}}"#,
            json_str(&key),
            json_str(&server.unwrap_or_default()),
        ),
        N::PostReceipt { key, blob, server } => format!(
            r#"{{"notice":"post_receipt","key":{},"blob":{},"server":{}}}"#,
            json_str(&key),
            json_str(&base64::engine::general_purpose::STANDARD.encode(blob)),
            json_str(&server.unwrap_or_default()),
        ),
    }
}

/// Create and start a transfer session. `params_json` carries the same fields
/// as `runTransfer` (direction/code/broker/relay/path/chunk_size/candidates/
/// use_room/use_mdns/resume). Notices (snapshots + mailbox courier requests)
/// are delivered to `callback.onEvent` as JSON; returns immediately.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_createSession(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "createSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "createSession") else {
        return;
    };

    let v: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => return emit_failed_snapshot(&vm, &cb, "invalid session params JSON", e),
    };
    let get = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let allow = split_csv(&get("candidates_allow"));
    let deny = split_csv(&get("candidates_deny"));
    let chunk = get("chunk_size");
    let chunk = (!chunk.is_empty()).then_some(chunk);
    let receipt_server = get("receipt_server");
    let client_context = ClientContext {
        chunk_size: chunk,
        candidates_allow: allow,
        candidates_deny: deny,
        receipt_server: (!receipt_server.is_empty()).then_some(receipt_server),
    };

    let code = get("code");
    let mut sources: Vec<PeerSource> = Vec::new();
    if v["use_room"].as_bool().unwrap_or(false) {
        sources.push(PeerSource::Room {
            code: code.clone(),
            broker: get("broker"),
        });
    }
    if v["use_mdns"].as_bool().unwrap_or(false) {
        sources.push(PeerSource::Mdns { token: Some(code) });
    }
    let mut options = TransferOptions::default();
    let relay = get("relay");
    options.relay = (!relay.is_empty()).then_some(relay); // empty = default, like the CLI
    options.resume = v["resume"].as_bool().unwrap_or(false);
    let direction = match get("direction").as_str() {
        "send" => TransferDirection::Send,
        _ => TransferDirection::Receive,
    };
    let context = SessionContext {
        client: client_context,
        params: SessionParams {
            direction,
            path: std::path::PathBuf::from(get("path")),
            sources,
            options,
        },
    };

    let extras = v.get("platform_extras").filter(|e| e.is_object()).cloned();
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::start(context, record_for(id), extras) {
        Ok(session) => session,
        Err(error) => return emit_failed_snapshot(&vm, &cb, "invalid session context", error),
    };
    if !register_session(id, session) {
        return emit_failed_snapshot(&vm, &cb, "session already live or registry unavailable", id);
    }
    spawn_pump(vm, cb, notices);
}

fn spawn_pump(
    vm: JavaVM,
    cb: GlobalRef,
    mut notices: tokio::sync::mpsc::UnboundedReceiver<envoix_client::api::driver::SessionNotice>,
) {
    runtime().spawn(async move {
        while let Some(notice) = notices.recv().await {
            emit(&vm, &cb, &notice_json(notice));
        }
    });
}

/// Set the durable record directory. Call once at app start, before sessions.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_initRecords(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
) {
    let dir = jstr(&mut env, &dir);
    if RECORDS
        .set(envoix_client::api::record::RecordStore::new(dir))
        .is_err()
    {
        tracing::warn!("initRecords: record store already initialized");
    }
}

/// All persisted transfer records as a JSON array (for restoring cards).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_listRecords<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
) -> jni::sys::jstring {
    let json = match RECORDS.get() {
        Some(store) => {
            let records = runtime().block_on(store.load_all());
            match serde_json::to_string(&records) {
                Ok(json) => json,
                Err(error) => {
                    return error_jstring(&mut env, "failed to serialize transfer records", error);
                }
            }
        }
        None => "[]".into(),
    };
    to_jstring(&mut env, &json)
}

/// Rehydrate a persisted session (no attempt launched; a mid-flight record
/// restores as Paused(Lost); a restored Unconfirmed resumes its mailbox poll).
/// Notices flow to `callback` like `createSession`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_restoreSession(
    env: JNIEnv,
    _class: JClass,
    id: jlong,
    callback: JObject,
) {
    let Some(vm) = java_vm_or_log(&env, "restoreSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "restoreSession") else {
        return;
    };
    let Some(store) = RECORDS.get() else {
        return emit_failed_snapshot(&vm, &cb, "transfer record store is not initialized", "");
    };
    let record = runtime()
        .block_on(store.load_all())
        .into_iter()
        .find(|r| r.id == id as u64);
    let Some(record) = record else {
        return emit_failed_snapshot(&vm, &cb, "transfer record not found", id);
    };
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::restore(record, record_for(id)) {
        Ok(session) => session,
        Err(error) => {
            return emit_failed_snapshot(&vm, &cb, "invalid restored session context", error);
        }
    };
    if !register_session(id, session) {
        return emit_failed_snapshot(&vm, &cb, "session already live or registry unavailable", id);
    }
    spawn_pump(vm, cb, notices);
}

/// Route a user intent ("pause" / "resume" / "cancel") to a live session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_sessionIntent(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    intent: JString,
) {
    let intent = jstr(&mut env, &intent);
    let Ok(map) = sessions().lock() else {
        tracing::warn!(id, intent, "sessionIntent: session registry unavailable");
        return;
    };
    let Some(session) = map.get(&id) else {
        // Not registered (yet): queue by record id - registration drains the
        // queue in order, so an intent can never race a restore and be lost.
        if let Ok(mut pending) = pending_intents().lock() {
            let queue = pending.entry(id).or_default();
            if queue.len() < MAX_PENDING_INTENTS {
                tracing::info!(id, intent, "session not registered; intent queued");
                queue.push(intent);
            } else {
                tracing::warn!(id, intent, "pending intent queue full; dropped");
            }
        }
        return;
    };
    route_intent(id, session, &intent);
}

/// The courier's answer to a fetch_receipt notice: the blob (base64), or ""
/// when the mailbox slot was empty. `key` echoes the notice's mailbox key,
/// so the driver can drop answers from a superseded attempt.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_receiptResponse(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    key: JString,
    blob_b64: JString,
) {
    use base64::Engine;
    let key = jstr(&mut env, &key);
    let blob_b64 = jstr(&mut env, &blob_b64);
    let blob = if blob_b64.trim().is_empty() {
        None
    } else {
        match base64::engine::general_purpose::STANDARD.decode(blob_b64.trim()) {
            Ok(blob) => Some(blob),
            Err(error) => {
                tracing::warn!(id, %error, "receiptResponse: invalid base64 blob");
                return;
            }
        }
    };
    let Ok(map) = sessions().lock() else {
        tracing::warn!(id, "receiptResponse: session registry unavailable");
        return;
    };
    let Some(session) = map.get(&id) else {
        tracing::warn!(id, "receiptResponse: session not found");
        return;
    };
    session.receipt_response(key, blob);
}

/// Replace the frontend-owned card context (QR payload, saved URI, ...)
/// persisted with the transfer's record. Opaque to the core.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_setSessionExtras(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    extras_json: JString,
) {
    let raw = jstr(&mut env, &extras_json);
    let Ok(extras) = serde_json::from_str::<serde_json::Value>(&raw) else {
        tracing::warn!(id, "setSessionExtras: invalid JSON");
        return;
    };
    let Ok(map) = sessions().lock() else {
        return;
    };
    let Some(session) = map.get(&id) else {
        tracing::debug!(id, "setSessionExtras: session not live");
        return;
    };
    session.set_extras(extras);
}

/// Tear a session down. With `discard` (D2, Remove): delete the partial,
/// resume state, and receipt sidecars first. Idempotent.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_destroySession(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    discard: jboolean,
) {
    let session = sessions().lock().ok().and_then(|mut m| m.remove(&id));
    if let Ok(mut pending) = pending_intents().lock() {
        pending.remove(&id);
    }
    let Some(session) = session else {
        // No live handle - the record is still the authority for existence:
        // Remove must clean the durable artifacts anyway, or the record
        // resurrects the card on the next restore.
        if discard != 0
            && let Some((store, id)) = record_for(id)
        {
            runtime().block_on(envoix_client::api::record::discard_record(&store, id));
        }
        return;
    };
    if discard != 0 {
        session.discard();
    }
    // Dropping the handle closes the command channel; the actor drains the
    // queued discard first, then detaches any live attempt - a silent
    // teardown (the peer sees connection loss, never a cancel) - and exits.
}
