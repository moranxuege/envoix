use super::*;
use std::io::Write;

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

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
