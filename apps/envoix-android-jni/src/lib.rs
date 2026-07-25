//! JNI bridge that runs an Envoix transfer **in the Android app's process** and
//! streams its event JSON to a Kotlin callback.
//!
//! Running in-process (rather than exec'ing the CLI as a subprocess) is required
//! on Android: the network stack reaches the platform through the JVM - DNS
//! (`hickory` → `ConnectivityManager`), interface enumeration (`netdev`), and the
//! TLS trust store (`rustls-platform-verifier`) all read the Android context via
//! `ndk_context`. [`initContext`] wires the VM + app context in once; without it
//! those crates panic ("android context was not initialized").

use std::sync::OnceLock;

use envoix_client::api::{Invite, Role};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

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

fn opt_json(s: Option<&str>) -> String {
    s.map(json_str).unwrap_or_else(|| "null".to_string())
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

mod logging;
mod manifest_v2;
mod room_control;
