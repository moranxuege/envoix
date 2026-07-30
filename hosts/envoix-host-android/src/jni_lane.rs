//! The JNI lane the Kotlin service drives. One global `Host` per process.
//!
//! Every entry point is a thin translation: bytes in, bytes out, no logic.
//!
//! ONE unsafe block appears here, and only here: adopting the file descriptor
//! the platform detaches for a source acquisition. Turning a raw descriptor into
//! an owned one asserts that nothing else will close it, and this is the only
//! place that assertion is checkable — Kotlin's `detachFd()` is in the same
//! call. Everything downstream takes the safe `OwnedFd`, so no other caller can
//! get the ownership question wrong. Otherwise the module-level allow exists
//! only for the edition-2024 `#[unsafe(no_mangle)]` export attributes.
//!
//! The slot is an `RwLock`: lane calls take it SHARED, so an intent awaiting
//! the runtime never blocks the frame/work polls or another intent. Only boot
//! and shutdown take it exclusively.

use std::path::Path;
use std::sync::{OnceLock, RwLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong, jstring};

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

/// `NativeHost.bindSourceDescriptor(card, generation, request, fd): Boolean`
///
/// The platform lends an open descriptor for one acquisition; this process
/// DUPLICATES it and owns the duplicate.
///
/// **Kotlin keeps its descriptor and closes it.** `fd` is borrowed for the
/// duration of this call only — the caller holds the `ParcelFileDescriptor`
/// across it — and every path here either duplicates it or touches it not at
/// all. A handover (`detachFd`) would make Rust the sole owner, which reads
/// tidier until the call does not happen: a renamed symbol raises
/// `UnsatisfiedLinkError` AFTER the detach, and the file is then open with no
/// owner in either language. Lending has no such path, because the caller's
/// `use` block closes whatever this does.
///
/// The duplicate shares the file offset with the original, which is why the
/// staging worker reads positionally.
///
/// A separate crossing from `reportDuty`, correlated by the acquisition key.
/// Folding the descriptor into the report would need a sentinel on every duty
/// that has none, and a sentinel is exactly the "absent looks like a value"
/// shape this contract vocabulary refuses everywhere else. Registering first and
/// reporting second is also the safe order: a report admitted with no descriptor
/// makes staging answer `Failed` and the card re-pick, whereas a descriptor with
/// no admitted report is discarded when the acquisition is superseded.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_envoix_host_NativeHost_bindSourceDescriptor(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    card: JString<'_>,
    generation: jint,
    request: JString<'_>,
    fd: jint,
) -> jboolean {
    // A negative descriptor is Android saying it had none to lend; borrowing it
    // would turn a refusal into a file this process believes it can read.
    if fd < 0 {
        return 0;
    }
    let (Ok(card), Ok(request)) = (env.get_string(&card), env.get_string(&request)) else {
        return 0;
    };
    let Some(acquisition) = crate::host::acquisition_from_hex(
        &String::from(card),
        u32::from_ne_bytes(generation.to_ne_bytes()),
        &String::from(request),
    ) else {
        return 0;
    };
    // SAFETY: `fd` is open for the whole of this call — the caller holds the
    // `ParcelFileDescriptor` it came from across it — and the borrow is consumed
    // here, never stored. Only the duplicate outlives the call. Asserted here
    // because here is the only place the calling contract is visible.
    #[allow(unsafe_code)]
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let Ok(descriptor) = borrowed.try_clone_to_owned() else {
        return 0;
    };
    u8::from(with_host(|host| host.bind_source_descriptor(acquisition, descriptor)).is_some())
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
    use jni::objects::JClass;
    use jni::sys::{jlong, jstring};

    use super::with_host;

    /// `E2eBridge.createForE2e(): Long` — gives the packaged instrumentation
    /// real durable state. Returns 0 on failure (a real RecordId is never
    /// zero).
    ///
    /// It takes no name or size any more: a card has neither until a document
    /// is chosen for it, so instrumentation that supplied them was stating
    /// facts the record cannot hold.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_envoix_host_E2eBridge_createForE2e(
        _env: JNIEnv<'_>,
        _class: JClass<'_>,
    ) -> jlong {
        with_host(|host| host.create_for_e2e().map_or(0, |card| card.get() as jlong)).unwrap_or(0)
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
