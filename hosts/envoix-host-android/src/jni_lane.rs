//! The JNI lane the Kotlin service drives. One global `Host` per process.
//!
//! Every entry point is a thin translation: bytes in, bytes out, no logic.
//! No unsafe BLOCK appears here; the module-level allow exists only for the
//! edition-2024 `#[unsafe(no_mangle)]` export attributes.
//!
//! The slot is an `RwLock`: lane calls take it SHARED, so a submit awaiting the
//! runtime never blocks the frame/work polls or another submit. Only boot and
//! shutdown take it exclusively.

use std::path::Path;
use std::sync::{OnceLock, RwLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray};

use crate::host::Host;

static HOST: OnceLock<RwLock<Option<Host>>> = OnceLock::new();

fn host_slot() -> &'static RwLock<Option<Host>> {
    HOST.get_or_init(|| RwLock::new(None))
}

/// Runs `call` against the live host, holding only a SHARED slot guard.
fn with_host<T>(call: impl FnOnce(&Host) -> T) -> Option<T> {
    let slot = host_slot()
        .read()
        .unwrap_or_else(|poison| poison.into_inner());
    slot.as_ref().map(call)
}

fn bytes_out(env: &mut JNIEnv<'_>, bytes: Option<Vec<u8>>) -> jbyteArray {
    match bytes {
        Some(bytes) => env
            .byte_array_from_slice(&bytes)
            .map(|array| array.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `NativeHost.boot(storageRoot): Boolean`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_boot(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    storage_root: JString<'_>,
) -> jboolean {
    let Ok(root) = env.get_string(&storage_root) else {
        return 0;
    };
    let root: String = root.into();
    let mut slot = host_slot()
        .write()
        .unwrap_or_else(|poison| poison.into_inner());
    if slot.is_some() {
        return 1;
    }
    match Host::boot(Path::new(&root)) {
        Ok(host) => {
            *slot = Some(host);
            1
        }
        Err(_) => 0,
    }
}

/// `NativeHost.pollFrame(): ByteArray?` — one read/command contract frame.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_pollFrame(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jbyteArray {
    let frame = with_host(Host::poll_frame).flatten();
    bytes_out(&mut env, frame)
}

/// `NativeHost.pollWork(): ByteArray?` — one platform work order.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_pollWork(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jbyteArray {
    let order = with_host(Host::poll_work).flatten();
    bytes_out(&mut env, order)
}

/// `NativeHost.submit(frame): ByteArray` — the encoded acceptance frame.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_submit(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    frame: JByteArray<'_>,
) -> jbyteArray {
    let Ok(bytes) = env.convert_byte_array(&frame) else {
        return std::ptr::null_mut();
    };
    let acceptance = with_host(|host| host.submit(&bytes));
    bytes_out(&mut env, acceptance)
}

/// `NativeHost.reportDuty(report): Boolean`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_reportDuty(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    report: JByteArray<'_>,
) -> jboolean {
    let Ok(bytes) = env.convert_byte_array(&report) else {
        return 0;
    };
    u8::from(with_host(|host| host.report_duty(&bytes)).unwrap_or(false))
}

/// `NativeHost.shutdown()`
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_shutdown(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    let host = host_slot()
        .write()
        .unwrap_or_else(|poison| poison.into_inner())
        .take();
    if let Some(host) = host {
        host.shutdown();
    }
}

/// The packaged process-death instrumentation lane, compiled ONLY under the
/// `e2e-instrumentation` feature.
///
/// The feature is off by default, so these exported symbols do not exist in a
/// release-shaped cdylib — not stripped afterwards, never compiled. The Kotlin
/// `E2eBridge` that binds them likewise lives in the debug source set only.
#[cfg(feature = "e2e-instrumentation")]
mod e2e {
    use jni::JNIEnv;
    use jni::objects::{JClass, JString};
    use jni::sys::{jlong, jstring};

    use super::with_host;
    use crate::host::Host;

    /// `E2eBridge.createForE2e(name, totalBytes): Long` — gives the packaged
    /// instrumentation real durable state. Returns 0 on failure (a real
    /// RecordId is never zero).
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_envoix_host_E2eBridge_createForE2e(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        name: JString<'_>,
        total_bytes: jlong,
    ) -> jlong {
        let Ok(name) = env.get_string(&name) else {
            return 0;
        };
        let name: String = name.into();
        let Ok(total) = u64::try_from(total_bytes) else {
            return 0;
        };
        with_host(|host| {
            host.create_for_e2e(&name, total)
                .map_or(0, |card| card.get() as jlong)
        })
        .unwrap_or(0)
    }

    /// `E2eBridge.liveCards(): String` — the restore probe: the cards this
    /// process generation actually brought back, as comma-separated 16-digit
    /// hex ids (empty when none).
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_envoix_host_E2eBridge_liveCards(
        env: JNIEnv<'_>,
        _class: JClass<'_>,
    ) -> jstring {
        let mut cards: Vec<String> = with_host(Host::live_cards)
            .unwrap_or_default()
            .into_iter()
            .map(|card| format!("{:016x}", card.get()))
            .collect();
        cards.sort();
        env.new_string(cards.join(","))
            .map(|text| text.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
}
