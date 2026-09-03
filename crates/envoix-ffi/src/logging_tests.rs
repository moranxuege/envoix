use super::*;

#[test]
fn core_info_advertises_the_typed_log_sink() {
    let info = crate::envoix_core_info();
    assert_eq!(info.ffi_api_version, 25);
    assert!(
        info.capabilities
            .iter()
            .any(|capability| capability == "typed_log_sink_v1")
    );
}

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

/// The exact envelope a fixed input produces — the cross-language golden.
/// `TransferTimeline.buildLine` (Kotlin) asserts byte-identical output for
/// the same inputs (`TimelineEnvelopeTest`), pinning column order + escaping
/// across the boundary. Change one side and one of the two tests fails.
#[test]
fn golden_line_matches_the_kotlin_builder() {
    let mut v = TimelineVisitor::default();
    v.put("attempt", "1".into());
    v.put("side", "sender".into());
    v.put("layer", "machine".into());
    v.put("event", "transition".into());
    v.put("outcome", "ok".into());
    v.put("cause", "a%b\tc\nd".into()); // exercises all three escaped octets
    let line = build_timeline_line(1, 1_720_000_000_000, 42, Some(7), &v);
    assert_eq!(
        line,
        "1\t1720000000000\t42\t7\t1\tsender\tmachine\ttransition\tok\tcause=a%25b%09c%0Ad"
    );
}

#[test]
fn absent_session_id_is_empty_not_zero() {
    let v = TimelineVisitor::default();
    let line = build_timeline_line(1, 0, 1, None, &v);
    assert_eq!(line.split('\t').nth(3), Some("")); // session_id column blank
}

use std::sync::{Arc, Mutex};

// Captured (session_id, target) pairs — a named type keeps the Arc<Mutex<…>>
// below out of clippy::type_complexity.
type Captured = Vec<(Option<u64>, String)>;

// A capturing stand-in for TimelineLayer: same session_id lookup + target
// guard, but records to a Vec instead of the typed sink.
struct Cap {
    guard: bool,
    out: Arc<Mutex<Captured>>,
}
impl<S> tracing_subscriber::layer::Layer<S> for Cap
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
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

fn run_capture(guard: bool, filtered: bool) -> Captured {
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
    out.lock().unwrap().clone()
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
