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
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use envoix_client::api::{Client, Invite, PeerSource, Role, TransferOptions};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jlong;

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Cancel closures for in-flight transfers, keyed by the Kotlin-side transfer id.
#[allow(clippy::type_complexity)]
static CANCELS: OnceLock<Mutex<HashMap<i64, Box<dyn Fn() + Send + Sync>>>> = OnceLock::new();

fn cancels() -> &'static Mutex<HashMap<i64, Box<dyn Fn() + Send + Sync>>> {
    CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// VM + Kotlin log sink for [`Java_dev_envoix_app_Native_initLogging`]. The
/// `tracing` subscriber below forwards every formatted line to `sink.log(String)`.
static LOG_VM: OnceLock<JavaVM> = OnceLock::new();
static LOG_SINK: OnceLock<GlobalRef> = OnceLock::new();

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
    // Keep the global ref alive for the whole process, since ndk_context holds a
    // raw pointer to it.
    std::mem::forget(ctx);
}

/// Run one transfer to completion, forwarding each event's JSON to
/// `callback.onEvent(String)`. Blocks the calling thread, so invoke it from a
/// background thread on the Kotlin side.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_runTransfer(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    direction: JString,
    code: JString,
    broker: JString,
    relay: JString,
    path: JString,
    config_path: JString,
    callback: JObject,
) {
    let direction = jstr(&mut env, &direction);
    let code = jstr(&mut env, &code);
    let broker = jstr(&mut env, &broker);
    let relay = jstr(&mut env, &relay);
    let path = jstr(&mut env, &path);
    let config_path = jstr(&mut env, &config_path);

    let vm = env.get_java_vm().expect("java vm");
    let cb = env.new_global_ref(&callback).expect("callback ref");

    runtime().block_on(async move {
        if let Err(err) = drive(id, &direction, code, broker, relay, path, config_path, &vm, &cb).await {
            emit(&vm, &cb, &format!(r#"{{"event":"failed","error":{}}}"#, json_str(&err)));
        }
    });
    if let Ok(mut map) = cancels().lock() {
        map.remove(&id);
    }
}

/// Cancel the in-flight transfer with the given id (no-op if it already ended).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_cancel(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    if let Ok(map) = cancels().lock()
        && let Some(cancel) = map.get(&id)
    {
        cancel();
    }
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
    let role = if jstr(&mut env, &role) == "send" { Role::Send } else { Role::Receive };
    let broker = jstr(&mut env, &broker);
    let relay = jstr(&mut env, &relay);
    let relay = (!relay.is_empty()).then_some(relay);
    let json = match Invite::room(broker, relay) {
        Ok(inv) => {
            let inv = inv.with_role(role);
            format!(r#"{{"code":{},"payload":{}}}"#, json_str(inv.code()), json_str(&inv.payload()))
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
    env.new_string(s).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

fn opt_json(s: Option<&str>) -> String {
    s.map(json_str).unwrap_or_else(|| "null".to_string())
}

async fn drive(
    id: i64,
    direction: &str,
    code: String,
    broker: String,
    relay: String,
    path: String,
    config_path: String,
    vm: &JavaVM,
    cb: &GlobalRef,
) -> Result<(), String> {
    let config = (!config_path.is_empty()).then(|| PathBuf::from(config_path));
    let client = Client::from_runtime_sources(config.as_deref()).map_err(|e| e.to_string())?;
    let source = PeerSource::Room { code, broker };
    let mut options = TransferOptions::default();
    options.relay = Some(relay);
    let into = PathBuf::from(path);
    let mut transfer = match direction {
        "send" => client.send(into, source, options),
        _ => client.receive(into, source, options),
    }
    .map_err(|e| e.to_string())?;

    // Register a cancel handle so `cancel(id)` can stop this transfer.
    let handle = transfer.cancel_handle();
    if let Ok(mut map) = cancels().lock() {
        map.insert(id, Box::new(move || handle.cancel()));
    }

    while let Some(event) = transfer.next_event().await {
        if let Ok(json) = serde_json::to_string(&event) {
            emit(vm, cb, &json);
        }
    }
    transfer.wait().await.map(|_| ()).map_err(|e| e.to_string())
}

/// Read a Java string into a Rust `String` (empty on any error).
fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|s| s.into()).unwrap_or_default()
}

/// Call `callback.onEvent(json)`, attaching this thread to the JVM as needed.
fn emit(vm: &JavaVM, cb: &GlobalRef, json: &str) {
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    if let Ok(js) = env.new_string(json) {
        let _ = env.call_method(cb, "onEvent", "(Ljava/lang/String;)V", &[JValue::Object(&js)]);
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
    let Ok(vm) = env.get_java_vm() else { return };
    let Ok(sink) = env.new_global_ref(&sink) else {
        return;
    };
    let _ = LOG_VM.set(vm);
    let _ = LOG_SINK.set(sink);

    let spec = std::env::var("ENVOIX_LOG").unwrap_or_else(|_| "envoix=debug,iroh=info,warn".to_string());
    let filter = tracing_subscriber::EnvFilter::try_new(&spec)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(JniLogWriter)
        .with_ansi(false)
        .with_target(false)
        .try_init();
}

/// Forward one formatted `tracing` line to `sink.log(...)`.
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
