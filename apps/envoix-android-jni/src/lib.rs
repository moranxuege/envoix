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
use std::sync::atomic::{AtomicU64, Ordering};
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

/// The always-on baseline: envoix internals + iroh's connection story. The
/// `envoix::timeline=off` directive keeps the structured timeline tier OUT of
/// the raw fmt trace (it has its own unfiltered layer) — no duplication.
const DEFAULT_LOG: &str = "envoix=debug,envoix::timeline=off,iroh=info,warn";
/// Appended to every runtime -vv spec so a reload can't re-admit timeline
/// events into the raw tier.
const TIMELINE_OFF: &str = ",envoix::timeline=off";

/// The tracing target that classifies structured authority events (the
/// transfer timeline, docs/design/diagnostics.md v2). A dedicated always-on
/// layer serializes these into the delimited envelope and routes them by
/// `session_id`, independent of the reloadable raw-trace filter (P7). Kept in
/// sync with `envoix_client`'s emitter const by value.
const TIMELINE_TARGET: &str = "envoix::timeline";
/// Envelope schema version — leads the line so a parser version-dispatches.
const TIMELINE_SCHEMA: u32 = 1;

/// Handle to the reloadable log filter, so the app can raise/lower verbosity at
/// runtime (the `-vv` dev toggle) without restarting.
type LogReload = tracing_subscriber::reload::Handle<
    tracing_subscriber::EnvFilter,
    tracing_subscriber::layer::Layered<RoomTag, tracing_subscriber::Registry>,
>;
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
    let (raw_filter, handle) = tracing_subscriber::reload::Layer::new(filter);
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    // Two tiers. The RAW trace passes the reloadable EnvFilter (the -vv knob);
    // the `envoix::timeline=off` directive in the spec keeps timeline events out
    // of it (so they aren't duplicated in the appendix). The TIMELINE tier is
    // UNFILTERED — it must see every span to read `session_id`, and it is
    // always-on regardless of the -vv knob (P7); an in-code target guard in
    // `TimelineLayer::on_event` restricts it to authority events. RoomTag stays
    // unfiltered so it stashes room + session_id into span extensions.
    let raw = tracing_subscriber::fmt::layer()
        .with_writer(JniLogWriter)
        .with_ansi(false)
        .with_target(false)
        .with_filter(raw_filter);
    let installed = tracing_subscriber::registry()
        .with(RoomTag)
        .with(raw)
        .with(TimelineLayer)
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
    let spec = format!("{}{}", String::from(spec), TIMELINE_OFF);
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

#[derive(Default)]
struct RoomVisitor {
    room: Option<String>,
    session_id: Option<u64>,
}

impl tracing::field::Visit for RoomVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "room" {
            self.room = Some(value.trim_matches('"').to_string());
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "session_id" {
            self.session_id = Some(value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "room" {
            self.room = Some(format!("{value:?}").trim_matches('"').to_string());
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
        let mut visitor = RoomVisitor::default();
        attrs.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            if let Some(room) = visitor.room {
                span.extensions_mut().replace(RoomField(room));
            }
            if let Some(sid) = visitor.session_id {
                span.extensions_mut().replace(SessionField(sid));
            }
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = RoomVisitor::default();
        values.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            if let Some(room) = visitor.room {
                span.extensions_mut().replace(RoomField(room));
            }
            if let Some(sid) = visitor.session_id {
                span.extensions_mut().replace(SessionField(sid));
            }
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

// ─────────────────────── transfer timeline (v2) ───────────────────────
//
// A second, always-on tier: structured authority events at `TIMELINE_TARGET`.
// Routed by `session_id` (the durable card id, carried on the session span) —
// NOT by room, so two live cards sharing a room stay in distinct files. The
// Kotlin writer stamps `source_seq`; Rust never assigns it.

/// The durable card id (`session_id`) recorded on the session span, stashed in
/// span extensions so a timeline event can find the nearest one.
struct SessionField(u64);

/// Percent-encode ONLY the three octets that would break the TAB-delimited
/// grammar: `%`, TAB, LF. URIs, spaces, `=`, `:` pass through literally — the
/// line stays greppable (docs/design/diagnostics.md, "Escaping grammar").
fn tl_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            c => out.push(c),
        }
    }
    out
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Collects a timeline event's fields: the fixed columns are pulled out by name,
/// everything else becomes the ordered `k=v` tail.
#[derive(Default)]
struct TimelineVisitor {
    attempt: String,
    side: String,
    layer: String,
    event: String,
    outcome: String,
    tail: Vec<(String, String)>,
}

impl TimelineVisitor {
    fn put(&mut self, name: &str, value: String) {
        match name {
            "attempt" => self.attempt = value,
            "side" => self.side = value,
            "layer" => self.layer = value,
            "event" => self.event = value,
            "outcome" => self.outcome = value,
            // room / session_id ride on the span, not the event
            "room" | "session_id" => {}
            other => self.tail.push((other.to_string(), value)),
        }
    }
}

impl tracing::field::Visit for TimelineVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.put(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.put(field.name(), value.to_string());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.put(field.name(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.put(field.name(), value.to_string());
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.put(
            field.name(),
            format!("{value:?}").trim_matches('"').to_string(),
        );
    }
}

/// Build the delimited envelope MINUS `source_seq` (the Kotlin writer prepends
/// that). Fixed leading columns are safe by construction (digits / controlled
/// enums); tail values are escaped.
fn build_timeline_line(
    schema: u32,
    epoch_ms: u64,
    run_id: u32,
    session_id: Option<u64>,
    v: &TimelineVisitor,
) -> String {
    let sid = session_id.map(|s| s.to_string()).unwrap_or_default();
    let mut line = format!(
        "{schema}\t{epoch_ms}\t{run_id}\t{sid}\t{}\t{}\t{}\t{}\t{}",
        v.attempt, v.side, v.layer, v.event, v.outcome,
    );
    for (k, val) in &v.tail {
        line.push('\t');
        line.push_str(k);
        line.push('=');
        line.push_str(&tl_escape(val));
    }
    line
}

/// The always-on timeline layer: on each `TIMELINE_TARGET` event, find the
/// nearest `session_id`, build the envelope, and hand `(session_id, line)` to
/// the Kotlin sink. Only `on_event` is filtered — the span that carries
/// `session_id` has a normal target, so `SessionField` is stashed by the
/// unfiltered [`RoomTag`] layer and read here from the span scope.
struct TimelineLayer;

impl<S> tracing_subscriber::layer::Layer<S> for TimelineLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        // In-code target guard, NOT a per-layer `.with_filter()`: a per-layer
        // filter would restrict this layer's SPAN visibility too, hiding the
        // `session` span (whose target is the driver module) so `SessionField`
        // could never be read (session_id came out empty on-device — see the
        // `perlayer_filter_hides_session_span` test). Unfiltered + guarded, the
        // layer sees every span but only ACTS on timeline events.
        if event.metadata().target() != TIMELINE_TARGET {
            return;
        }
        let session_id = ctx.event_scope(event).and_then(|scope| {
            scope
                .filter_map(|span| span.extensions().get::<SessionField>().map(|s| s.0))
                .next()
        });
        let mut v = TimelineVisitor::default();
        event.record(&mut v);
        let line = build_timeline_line(
            TIMELINE_SCHEMA,
            epoch_ms(),
            std::process::id(),
            session_id,
            &v,
        );
        timeline_line(session_id.unwrap_or(0), &line);
    }
}

/// Forward one built timeline line to `sink.timeline(sessionId, line)`.
fn timeline_line(session_id: u64, line: &str) {
    let (Some(vm), Some(sink)) = (LOG_VM.get(), LOG_SINK.get()) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    if let Ok(js) = env.new_string(line) {
        let _ = env.call_method(
            sink,
            "timeline",
            "(JLjava/lang/String;)V",
            &[JValue::Long(session_id as i64), JValue::Object(&js)],
        );
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    #[test]
    fn escape_touches_only_delimiter_octets() {
        // URIs, spaces, and `=` survive; only %, TAB, LF are encoded.
        assert_eq!(
            tl_escape("content://media/Download/x.bin?take=1"),
            "content://media/Download/x.bin?take=1"
        );
        assert_eq!(tl_escape("a\tb\nc%d"), "a%09b%0Ac%25d");
        // decode is unambiguous: %25 must come from a literal % only
        assert_eq!(tl_escape("100%"), "100%25");
    }

    #[test]
    fn envelope_columns_are_positional_then_tail() {
        let mut v = TimelineVisitor::default();
        v.put("layer", "session".into());
        v.put("event", "created".into());
        v.put("attempt", "0".into());
        v.put("cause", "disk full = bad".into()); // tail value with = and space
        let line = build_timeline_line(1, 1_720_000_000_000, 42, Some(7), &v);
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols[0], "1"); // schema
        assert_eq!(cols[1], "1720000000000"); // epoch_ms
        assert_eq!(cols[2], "42"); // run_id
        assert_eq!(cols[3], "7"); // session_id
        assert_eq!(cols[4], "0"); // attempt
        assert_eq!(cols[5], ""); // side (absent)
        assert_eq!(cols[6], "session"); // layer
        assert_eq!(cols[7], "created"); // event
        assert_eq!(cols[8], ""); // outcome (absent)
        assert_eq!(cols[9], "cause=disk full = bad"); // tail, first = splits k/v
    }

    #[test]
    fn absent_session_id_is_empty_not_zero() {
        let v = TimelineVisitor::default();
        let line = build_timeline_line(1, 0, 1, None, &v);
        assert_eq!(line.split('\t').nth(3), Some("")); // session_id column blank
    }

    use std::sync::{Arc, Mutex};

    // A capturing stand-in for TimelineLayer: same session_id lookup + target
    // guard, but records to a Vec instead of the JNI sink.
    struct Cap {
        guard: bool,
        out: Arc<Mutex<Vec<(Option<u64>, String)>>>,
    }
    impl<S> tracing_subscriber::layer::Layer<S> for Cap
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if self.guard && event.metadata().target() != TIMELINE_TARGET {
                return;
            }
            let sid = ctx.event_scope(event).and_then(|scope| {
                scope
                    .filter_map(|s| s.extensions().get::<SessionField>().map(|f| f.0))
                    .next()
            });
            self.out
                .lock()
                .unwrap()
                .push((sid, event.metadata().target().to_string()));
        }
    }

    fn run_capture(guard: bool, filtered: bool) -> Vec<(Option<u64>, String)> {
        use tracing_subscriber::Layer as _;
        use tracing_subscriber::layer::SubscriberExt;
        let out = Arc::new(Mutex::new(Vec::new()));
        let cap = Cap {
            guard,
            out: out.clone(),
        };
        let sub = if filtered {
            tracing_subscriber::registry().with(RoomTag).with(
                cap.with_filter(tracing_subscriber::filter::filter_fn(|m| {
                    m.target() == TIMELINE_TARGET
                }))
                .boxed(),
            )
        } else {
            tracing_subscriber::registry()
                .with(RoomTag)
                .with(cap.boxed())
        };
        tracing::subscriber::with_default(sub, || {
            let span = tracing::info_span!("session", room = "r", session_id = 7u64);
            span.in_scope(|| {
                tracing::info!(target: "envoix::timeline", layer = "session", event = "created");
                tracing::info!(target: "iroh_relay", "noise that must NOT reach the timeline");
            });
        });
        let v = out.lock().unwrap().clone();
        v
    }

    // WHY TimelineLayer must NOT use a per-layer filter (the a1 bug): a
    // per-layer `target` filter restricts the layer's SPAN visibility, so the
    // session span (targeted at the driver module, not the timeline target) is
    // hidden and `session_id` can never be read.
    #[test]
    fn perlayer_filter_hides_session_span() {
        let got = run_capture(false, true);
        assert_eq!(
            got[0].0, None,
            "the per-layer filter hides the session span → session_id lost"
        );
    }

    // The FIX: no per-layer filter (so the session span is visible → session_id
    // resolves), an explicit target guard (so non-timeline events are dropped).
    #[test]
    fn guard_without_filter_resolves_session_id_and_drops_noise() {
        let got = run_capture(true, false);
        assert_eq!(got.len(), 1, "only the timeline event survives the guard");
        assert_eq!(
            got[0].0,
            Some(7),
            "session_id resolves from the visible span"
        );
        assert_eq!(got[0].1, TIMELINE_TARGET);
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

    let (context, extras) = match parse_create_params(&json, CreateMode::Normal) {
        Ok(parsed) => parsed,
        Err(e) => return emit_failed_snapshot(&vm, &cb, "invalid session params", e),
    };
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

/// Direction as the frontend sends it (lowercase). A typed enum so a typo like
/// `"recieve"` is a loud deserialize error, not a silent fall-through to
/// Receive. JNI-local, so the wire-adjacent `TransferDirection` serde repr
/// (PascalCase, read by the snapshot) stays untouched.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreateDirection {
    Send,
    Receive,
}

impl From<CreateDirection> for TransferDirection {
    fn from(d: CreateDirection) -> Self {
        match d {
            CreateDirection::Send => TransferDirection::Send,
            CreateDirection::Receive => TransferDirection::Receive,
        }
    }
}

/// Android platform extras. The JNI adapter knows these keys; the core keeps
/// them opaque (they round-trip back to an untyped `Value`). Typed here with
/// `deny_unknown_fields` only so a misspelled extras key fails loudly at the
/// boundary instead of silently vanishing.
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AndroidPlatformExtras {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    qr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_recoverable: Option<bool>,
}

/// Which create entry point is parsing — staging additionally requires a source.
#[derive(Clone, Copy)]
enum CreateMode {
    Normal,
    Staging,
}

/// The frontend's flat session-params JSON — the UI-shaped boundary DTO. Every
/// top-level field is REQUIRED: Kotlin's `paramsJson` always emits all of them
/// (an empty string means "use the core default"), so a *missing* field means
/// the two sides drifted and must fail loudly, not default silently.
/// `deny_unknown_fields` catches a renamed / typo'd / extra key the same way.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateParams {
    direction: CreateDirection,
    path: String,
    code: String,
    broker: String,
    relay: String,
    chunk_size: String,
    data_stream_window: String,
    candidates_allow: String,
    candidates_deny: String,
    receipt_server: String,
    use_room: bool,
    use_mdns: bool,
    resume: bool,
    /// Optional: Kotlin omits it when there are no extras (`if length > 0`).
    #[serde(default)]
    platform_extras: Option<AndroidPlatformExtras>,
}

impl CreateParams {
    fn into_context(
        self,
        mode: CreateMode,
    ) -> Result<(SessionContext, Option<serde_json::Value>), String> {
        if self.path.trim().is_empty() {
            return Err("path must not be empty".into());
        }
        if let Some(x) = &self.platform_extras {
            // Kotlin writes the two together; reject a half-pair.
            if x.source_uri.is_some() != x.source_recoverable.is_some() {
                return Err("source_uri and source_recoverable must be set together".into());
            }
        }
        if matches!(mode, CreateMode::Staging) {
            let has_source = self
                .platform_extras
                .as_ref()
                .and_then(|x| x.source_uri.as_deref())
                .is_some_and(|s| !s.is_empty());
            if !has_source {
                return Err("staging session requires a source_uri in platform_extras".into());
            }
        }

        let opt = |s: String| (!s.is_empty()).then_some(s);
        let client = ClientContext {
            chunk_size: opt(self.chunk_size),
            data_stream_window: opt(self.data_stream_window),
            candidates_allow: split_csv(&self.candidates_allow),
            candidates_deny: split_csv(&self.candidates_deny),
            receipt_server: opt(self.receipt_server),
        };

        let mut sources: Vec<PeerSource> = Vec::new();
        if self.use_room {
            sources.push(PeerSource::Room {
                code: self.code.clone(),
                broker: self.broker,
            });
        }
        if self.use_mdns {
            sources.push(PeerSource::Mdns {
                token: Some(self.code),
            });
        }

        let mut options = TransferOptions::default();
        options.relay = opt(self.relay); // empty = default, like the CLI
        options.resume = self.resume;

        let context = SessionContext {
            client,
            params: SessionParams {
                direction: self.direction.into(),
                path: std::path::PathBuf::from(self.path),
                sources,
                options,
            },
        };

        // Hand the extras to the core as an opaque object (it never interprets
        // them); the typing above is purely boundary key-validation.
        let extras = match self.platform_extras {
            Some(x) => Some(serde_json::to_value(&x).map_err(|e| format!("platform_extras: {e}"))?),
            None => None,
        };
        Ok((context, extras))
    }
}

/// Parse + validate the frontend's create-session params (shared by the normal
/// and staging entry points), producing the durable [`SessionContext`] plus the
/// opaque platform extras. A single implementation of parse, conversion, and
/// mode validation.
fn parse_create_params(
    json: &str,
    mode: CreateMode,
) -> Result<(SessionContext, Option<serde_json::Value>), String> {
    let params: CreateParams =
        serde_json::from_str(json).map_err(|e| format!("params JSON: {e}"))?;
    params.into_context(mode)
}

#[cfg(test)]
mod create_params_tests {
    use super::*;

    /// A complete, valid params object (a joined room receive). Tests mutate a
    /// clone to exercise each rejection path.
    fn valid() -> serde_json::Value {
        serde_json::json!({
            "direction": "receive",
            "path": "/tmp/out",
            "code": "123456-cobalt-flint",
            "broker": "id@1.2.3.4:5",
            "relay": "",
            "chunk_size": "",
            "data_stream_window": "",
            "candidates_allow": "",
            "candidates_deny": "",
            "receipt_server": "",
            "use_room": true,
            "use_mdns": false,
            "resume": false
        })
    }

    fn parse(
        v: &serde_json::Value,
        mode: CreateMode,
    ) -> Result<(SessionContext, Option<serde_json::Value>), String> {
        parse_create_params(&v.to_string(), mode)
    }

    #[test]
    fn valid_params_build_a_context() {
        let (ctx, extras) = parse(&valid(), CreateMode::Normal).unwrap();
        assert!(matches!(ctx.params.direction, TransferDirection::Receive));
        assert_eq!(ctx.params.sources.len(), 1); // room only (use_mdns false)
        assert!(extras.is_none());
    }

    #[test]
    fn a_missing_field_is_rejected_not_defaulted() {
        // The whole point: deleting a `put(...)` on the Kotlin side must error,
        // not silently become "".
        let mut v = valid();
        v.as_object_mut().unwrap().remove("data_stream_window");
        assert!(parse(&v, CreateMode::Normal).is_err());
    }

    #[test]
    fn an_unknown_or_renamed_field_is_rejected() {
        let mut v = valid();
        v["chunkSize"] = serde_json::json!("64KB"); // camelCase typo of chunk_size
        assert!(parse(&v, CreateMode::Normal).is_err());
    }

    #[test]
    fn a_typo_direction_is_rejected() {
        let mut v = valid();
        v["direction"] = serde_json::json!("recieve");
        assert!(parse(&v, CreateMode::Normal).is_err());
    }

    #[test]
    fn an_empty_path_is_rejected() {
        let mut v = valid();
        v["path"] = serde_json::json!("");
        assert!(parse(&v, CreateMode::Normal).is_err());
    }

    #[test]
    fn staging_requires_a_source_but_normal_does_not() {
        assert!(parse(&valid(), CreateMode::Staging).is_err());
        assert!(parse(&valid(), CreateMode::Normal).is_ok());
    }

    #[test]
    fn staging_with_a_source_round_trips_extras() {
        let mut v = valid();
        v["direction"] = serde_json::json!("send");
        v["path"] = serde_json::json!("/tmp/staged");
        v["platform_extras"] = serde_json::json!({
            "source_uri": "content://x",
            "source_recoverable": true
        });
        let (_, extras) = parse(&v, CreateMode::Staging).unwrap();
        let extras = extras.unwrap();
        assert_eq!(extras["source_uri"], "content://x");
        assert_eq!(extras["source_recoverable"], true);
    }

    #[test]
    fn a_half_pair_of_source_fields_is_rejected() {
        let mut v = valid();
        v["platform_extras"] = serde_json::json!({ "source_uri": "content://x" });
        assert!(parse(&v, CreateMode::Normal).is_err());
    }

    #[test]
    fn an_unknown_extras_key_is_rejected() {
        let mut v = valid();
        v["platform_extras"] = serde_json::json!({ "qrr": "x" }); // typo of qr
        assert!(parse(&v, CreateMode::Normal).is_err());
    }
}

/// Create a SEND session that stages its `content://` source first: the
/// session starts in Preparing and the record is committed before Kotlin
/// copies a byte. Notices flow like `createSession`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_createStagingSession(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    params_json: JString,
    callback: JObject,
) {
    let json = jstr(&mut env, &params_json);
    let Some(vm) = java_vm_or_log(&env, "createStagingSession") else {
        return;
    };
    let Some(cb) = callback_or_log(&env, &callback, "createStagingSession") else {
        return;
    };
    let (context, extras) = match parse_create_params(&json, CreateMode::Staging) {
        Ok(parsed) => parsed,
        Err(e) => return emit_failed_snapshot(&vm, &cb, "invalid session params", e),
    };
    let _guard = runtime().enter();
    let (session, notices) = match TransferSession::start_staging(context, record_for(id), extras) {
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
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    runtime().spawn(async move {
        while let Some(notice) = notices.recv().await {
            emit(&vm, &cb, &notice_json(notice, generation));
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
pub extern "system" fn Java_dev_envoix_app_Native_listRestoreContexts<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass,
) -> jni::sys::jstring {
    let json = match RECORDS.get() {
        Some(store) => {
            let records = runtime().block_on(store.load_all());
            let dtos: Vec<serde_json::Value> = records
                .iter()
                .map(|record| {
                    let mut value = serde_json::to_value(record.restore_context())
                        .unwrap_or(serde_json::Value::Null);
                    // Android-specific card context lives in the opaque
                    // platform_extras (the core does not interpret it); the
                    // JNI glue, which knows the Android keys, flattens the two
                    // the frontend needs onto the DTO.
                    if let (Some(object), Some(extras)) =
                        (value.as_object_mut(), record.platform_extras.as_ref())
                    {
                        for key in ["qr", "saved_uri", "source_uri"] {
                            if let Some(text) = extras.get(key).and_then(|v| v.as_str()) {
                                object.insert(key.into(), text.into());
                            }
                        }
                        if let Some(ok) = extras.get("source_recoverable").and_then(|v| v.as_bool())
                        {
                            object.insert("source_recoverable".into(), ok.into());
                        }
                    }
                    value
                })
                .collect();
            match serde_json::to_string(&dtos) {
                Ok(json) => json,
                Err(error) => {
                    return error_jstring(&mut env, "failed to serialize restore contexts", error);
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

/// A Preparing send: report staging copy progress (moves the bar only).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageProgress(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    bytes: jlong,
) {
    let Ok(map) = sessions().lock() else {
        return;
    };
    if let Some(session) = map.get(&id) {
        session.stage_progress(bytes.max(0) as u64);
    }
}

/// A Preparing send: staging finished, launch the first attempt.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageComplete(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    let Ok(map) = sessions().lock() else {
        return;
    };
    match map.get(&id) {
        Some(session) => session.stage_complete(),
        None => tracing::debug!(id, "stageComplete: session not live"),
    }
}

/// A Preparing send: staging failed, fail the transfer with `reason`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_stageFailed(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    reason: JString,
) {
    let reason = jstr(&mut env, &reason);
    let Ok(map) = sessions().lock() else {
        return;
    };
    match map.get(&id) {
        Some(session) => session.stage_failed(reason),
        None => tracing::debug!(id, "stageFailed: session not live"),
    }
}

/// Replace the frontend-owned card context (QR payload, saved URI, ...)
/// persisted with the transfer's record. Opaque to the core.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_envoix_app_Native_setSessionExtras(
    mut env: JNIEnv,
    _class: JClass,
    id: jlong,
    extras_json: JString,
) -> jni::sys::jstring {
    let raw = jstr(&mut env, &extras_json);
    // Typed at the boundary (deny_unknown_fields): a misspelled/mistyped extras
    // key is a loud error, not a silently-stored one. Re-serialized to an opaque
    // object for the core (which never interprets it). Returns "" on success, an
    // error message otherwise, so the caller can surface a real boundary drift.
    let extras = match serde_json::from_str::<AndroidPlatformExtras>(&raw)
        .and_then(|x| serde_json::to_value(&x))
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("setSessionExtras: invalid extras: {e}");
            tracing::warn!(id, %msg);
            return to_jstring(&mut env, &msg);
        }
    };
    let Ok(map) = sessions().lock() else {
        return to_jstring(&mut env, "");
    };
    let Some(session) = map.get(&id) else {
        // A benign race (Kotlin syncs after teardown), not a drift — no error.
        tracing::debug!(id, "setSessionExtras: session not live");
        return to_jstring(&mut env, "");
    };
    session.set_extras(extras);
    to_jstring(&mut env, "")
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
