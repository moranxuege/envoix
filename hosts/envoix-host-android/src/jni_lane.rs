//! The JNI lane the Kotlin service drives. One global `Host` per process.
//!
//! Every entry point is a thin translation: bytes in, bytes out, no logic.
//! No unsafe BLOCK appears here; the module-level allow exists only for the
//! edition-2024 `#[unsafe(no_mangle)]` export attributes.
//!
//! The slot is an `RwLock`: lane calls take it SHARED, so an intent awaiting
//! the runtime never blocks the frame/work polls or another intent. Only boot
//! and shutdown take it exclusively.

use std::path::Path;
use std::sync::{OnceLock, RwLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jlong, jstring};

use crate::host::{AttachmentToken, FramePoll, Host, IntentRejection};

static HOST: OnceLock<RwLock<Option<Host>>> = OnceLock::new();

/// The Kotlin exception a superseded poll raises. A refusal that reads as
/// "nothing queued" would leave the replaced pump spinning forever, so the
/// frontend is told in the one way a JNI method can say something typed.
const SUPERSEDED: &str = "app/envoix/host/SupersededAttachment";
/// A frontend frame reached the authority boundary and was refused there.
const REJECTED_INTENT: &str = "app/envoix/host/RejectedIntent";

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

/// `NativeHost.attach(): Long` — opens a fresh frontend attachment and returns
/// its token; 0 means no host is running.
///
/// The one verb an observer needs and the only one it gets: it starts and
/// stops nothing, and the lane has no detach counterpart, so a frontend that
/// goes away cannot spell anything a transfer could notice. The token is what
/// makes the attachment an identity rather than an anonymous consumer.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_attach(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jlong {
    with_host(Host::open_lane).map_or(0, |token| token.raw() as jlong)
}

/// `NativeHost.pollFrame(token): ByteArray?` — one read/command contract frame
/// for THAT attachment. A superseded token consumes nothing and raises
/// `SupersededAttachment`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_pollFrame(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    token: jlong,
) -> jbyteArray {
    let token = AttachmentToken::from_raw(token as u64);
    match with_host(|host| host.poll_frame(token)) {
        Some(FramePoll::Frame(bytes)) => bytes_out(&mut env, Some(bytes)),
        Some(FramePoll::Superseded) => {
            let _ = env.throw_new(SUPERSEDED, "a newer attachment holds the frame lane");
            std::ptr::null_mut()
        }
        Some(FramePoll::Drained) | None => std::ptr::null_mut(),
    }
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

/// `NativeHost.pollSourceRelease(): String?` — one durably removed card whose
/// platform-owned persistable source grant must be released.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_pollSourceRelease(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let card = with_host(Host::poll_source_release).flatten();
    card.and_then(|card| env.new_string(format!("{:016x}", card.get())).ok())
        .map(|text| text.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `NativeHost.intent(frame): ByteArray` — one frontend-originated intent
/// frame in, the authority's encoded answer out: an acceptance for a command
/// on an existing card, or a create result for a request that one be made.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_intent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    frame: JByteArray<'_>,
) -> jbyteArray {
    let Ok(bytes) = env.convert_byte_array(&frame) else {
        return std::ptr::null_mut();
    };
    match with_host(|host| host.intent(&bytes)) {
        Some(Ok(answer)) => bytes_out(&mut env, Some(answer)),
        Some(Err(IntentRejection::Contract)) => {
            let _ = env.throw_new(REJECTED_INTENT, "the authority refused the intent frame");
            std::ptr::null_mut()
        }
        None => std::ptr::null_mut(),
    }
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

    /// `E2eBridge.liveCards(): String` — one debug report, in two sections
    /// separated by `;durable=`: the cards this process generation actually
    /// brought back as comma-separated 16-digit hex ids, then each card's
    /// latest COMMITTED state read back off disk as `id:state`. One JNI symbol
    /// answers both, because BN5 pins the dynamic symbol table exactly.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_envoix_host_E2eBridge_liveCards(
        env: JNIEnv<'_>,
        _class: JClass<'_>,
    ) -> jstring {
        let report = with_host(|host| {
            let mut cards = host.live_cards();
            cards.sort();
            let ids = cards
                .iter()
                .map(|card| format!("{:016x}", card.get()))
                .collect::<Vec<_>>()
                .join(",");
            let durable = cards
                .into_iter()
                .map(|card| format!("{:016x}:{}", card.get(), host.durable_state_for_e2e(card)))
                .collect::<Vec<_>>()
                .join(",");
            format!("{ids};durable={durable}")
        })
        .unwrap_or_else(|| ";durable=".to_owned());
        env.new_string(report)
            .map(|text| text.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
}
