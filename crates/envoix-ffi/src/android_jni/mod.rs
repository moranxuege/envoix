//! Exceptional JNI bridge compiled into the same library as the typed UniFFI
//! API. It runs an Envoix transfer **in the Android app's process** and
//! streams its event JSON to a Kotlin callback.
//!
//! Running in-process (rather than exec'ing the CLI as a subprocess) is required
//! on Android: the network stack reaches the platform through the JVM - DNS
//! (`hickory` → `ConnectivityManager`), interface enumeration (`netdev`), and the
//! TLS trust store (`rustls-platform-verifier`) all read the Android context via
//! `ndk_context`. [`initContext`] wires the VM + app context in once; without it
//! those crates panic ("android context was not initialized").

use std::sync::OnceLock;

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
    // SAFETY: ndk-context only accepts the raw VM and application-object
    // pointers supplied by JNI. The global reference is deliberately retained
    // for the process lifetime immediately below, so both pointers stay valid.
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

fn to_jstring(env: &mut JNIEnv, s: &str) -> jni::sys::jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to allocate Java string");
            std::ptr::null_mut()
        })
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
mod nearby_invite;
