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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use envoix_client::TransferDirection;
use envoix_client::api::driver::{ClientContext, SessionContext, SessionParams, TransferSession};
use envoix_client::api::{Invite, PeerSource, Role, TransferOptions};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};

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

/// Monotone pump generation: every createSession/restoreSession claims one
/// and stamps it into all of that session's notices. Kotlin gates per card,
/// so a stale pump (a torn-down session for the same id) can never mutate
/// the current card - the fence is explicit, not an artifact of flow
/// mechanics.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

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
        "pause" => {
            session.pause();
        }
        "resume" => {
            session.resume();
        }
        "cancel" => {
            session.cancel();
        }
        "reverify" => {
            session.serve_reverify();
        }
        "receipt_posted" => {
            session.receipt_posted();
        }
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
fn notice_json(notice: envoix_client::api::driver::SessionNotice, generation: u64) -> String {
    use base64::Engine;
    use envoix_client::api::driver::SessionNotice as N;
    match notice {
        N::Snapshot(snapshot) => {
            let mut value = serde_json::to_value(&snapshot).unwrap_or_default();
            if let Some(map) = value.as_object_mut() {
                map.insert("notice".into(), "snapshot".into());
                map.insert("gen".into(), generation.into());
                // `path` stays the TYPED DataPath encoding ({type, addr|url|
                // description}); the frontend reads those fields directly, never
                // a Display string it then has to re-parse (which lost the type
                // for DataPath::Other and truncated relay URLs with spaces).
            }
            value.to_string()
        }
        N::Event(event) => {
            let mut value = serde_json::to_value(&event).unwrap_or_default();
            if let Some(map) = value.as_object_mut() {
                map.insert("notice".into(), "event".into());
                map.insert("gen".into(), generation.into());
            }
            value.to_string()
        }
        N::FetchReceipt { key, server } => format!(
            r#"{{"notice":"fetch_receipt","gen":{generation},"key":{},"server":{}}}"#,
            json_str(&key),
            json_str(&server.unwrap_or_default()),
        ),
        N::PostReceipt { key, blob, server } => format!(
            r#"{{"notice":"post_receipt","gen":{generation},"key":{},"blob":{},"server":{}}}"#,
            json_str(&key),
            json_str(&base64::engine::general_purpose::STANDARD.encode(blob)),
            json_str(&server.unwrap_or_default()),
        ),
    }
}

mod logging;
mod session;
