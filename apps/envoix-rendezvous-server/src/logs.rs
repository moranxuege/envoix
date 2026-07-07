//! Per-room log collection for holistic debugging: peers POST their transfer log,
//! an operator GETs the merged view — the rdz's own room events plus both peers'
//! logs, one page. Keyed by room id (the code's first segment, the same id the
//! broker matches on). In-memory with a TTL; auth is possession of the room id
//! (it's the URL). Runs on a separate HTTP port from the pairing endpoint, so it
//! never touches the SPAKE2 wire protocol.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::post;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Cap on a single uploaded log body.
const MAX_BODY: usize = 512 * 1024;
/// Cap on the rdz's own captured lines per room.
const MAX_RDZ_LINES: usize = 2000;
/// Reject absurd room keys.
const MAX_ROOM_KEY: usize = 64;

/// Collected logs for one room.
#[derive(Default)]
struct RoomEntry {
    updated: Option<Instant>,
    /// The rdz's own events for this room (captured from its tracing).
    rdz: Vec<String>,
    /// Each peer's uploaded log, keyed by side ("send"/"receive"/…).
    clients: Vec<(String, String)>,
}

/// Room -> collected logs, evicted after `ttl` of inactivity.
pub struct RoomLogs {
    ttl: Duration,
    rooms: Mutex<HashMap<String, RoomEntry>>,
}

impl RoomLogs {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            rooms: Mutex::new(HashMap::new()),
        }
    }

    /// Append the rdz's own log line for `room` (called from the tracing layer).
    pub fn push_rdz(&self, room: &str, line: String) {
        if room.len() > MAX_ROOM_KEY {
            return;
        }
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        let entry = rooms.entry(room.to_string()).or_default();
        entry.rdz.push(line);
        if entry.rdz.len() > MAX_RDZ_LINES {
            entry.rdz.drain(0..entry.rdz.len() - MAX_RDZ_LINES);
        }
        entry.updated = Some(Instant::now());
    }

    /// Store a peer's uploaded log, replacing any prior upload for the same side.
    fn upload(&self, room: &str, side: &str, body: String) {
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        let entry = rooms.entry(room.to_string()).or_default();
        entry.clients.retain(|(s, _)| s != side);
        entry.clients.push((side.to_string(), body));
        entry.updated = Some(Instant::now());
    }

    /// Render the merged view for `room`, or None if nothing collected.
    fn view(&self, room: &str) -> Option<String> {
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        let entry = rooms.get(room)?;
        let mut out = String::new();
        if !entry.rdz.is_empty() {
            out.push_str(&format!("═════ rdz · room {room} ═════\n"));
            for line in &entry.rdz {
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
        for (side, log) in &entry.clients {
            out.push_str(&format!("═════ {side} ═════\n"));
            out.push_str(log);
            if !log.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        Some(out)
    }
}

fn evict(rooms: &mut HashMap<String, RoomEntry>, ttl: Duration) {
    let now = Instant::now();
    rooms.retain(|_, e| e.updated.is_some_and(|u| now.duration_since(u) < ttl));
}

/// The axum router: `POST /logs/{room}?side=…` ingests, `GET /logs/{room}` views.
pub fn router(store: Arc<RoomLogs>) -> Router {
    Router::new()
        .route("/logs/{room}", post(upload).get(view))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(store)
}

async fn upload(
    Path(room): Path<String>,
    RawQuery(query): RawQuery,
    State(store): State<Arc<RoomLogs>>,
    body: String,
) -> StatusCode {
    if room.len() > MAX_ROOM_KEY {
        return StatusCode::BAD_REQUEST;
    }
    store.upload(&room, &side_of(query.as_deref()), body);
    StatusCode::NO_CONTENT
}

async fn view(
    Path(room): Path<String>,
    State(store): State<Arc<RoomLogs>>,
) -> (StatusCode, String) {
    match store.view(&room) {
        Some(text) => (StatusCode::OK, text),
        None => (StatusCode::NOT_FOUND, "no logs for this room\n".to_string()),
    }
}

/// Extract a sanitized `side` from the raw query string; default "peer".
fn side_of(query: Option<&str>) -> String {
    let side: String = query
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("side=")))
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    if side.is_empty() {
        "peer".to_string()
    } else {
        side
    }
}

// ───────── capturing the rdz's own per-room events ─────────

/// A span's `room` field, stored in span extensions once known.
struct RoomTag(String);

/// Pulls the `room` field out of span attributes / records.
struct FindRoom(Option<String>);
impl Visit for FindRoom {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "room" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "room" {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }
}

/// Renders an event into "message  k=v …".
struct FmtEvent {
    message: String,
    fields: String,
}
impl Visit for FmtEvent {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "room" => {}
            name => {
                if !self.fields.is_empty() {
                    self.fields.push(' ');
                }
                self.fields.push_str(&format!("{name}={value:?}"));
            }
        }
    }
}

/// Tracing layer that captures every event tagged (via its span) with a `room`
/// field into [`RoomLogs`], so the rdz's own pairing story lands in the room view
/// alongside both peers' uploaded logs.
pub struct RoomCapture {
    store: Arc<RoomLogs>,
}

impl RoomCapture {
    pub fn new(store: Arc<RoomLogs>) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for RoomCapture
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut find = FindRoom(None);
            attrs.record(&mut find);
            if let Some(room) = find.0 {
                span.extensions_mut().insert(RoomTag(room));
            }
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        // `room` is recorded lazily (Span::current().record("room", …)), so pick
        // it up here too.
        if let Some(span) = ctx.span(id) {
            let mut find = FindRoom(None);
            values.record(&mut find);
            if let Some(room) = find.0 {
                span.extensions_mut().insert(RoomTag(room));
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let room = ctx.event_scope(event).and_then(|scope| {
            scope
                .from_root()
                .find_map(|span| span.extensions().get::<RoomTag>().map(|t| t.0.clone()))
        });
        let Some(room) = room else { return };
        let mut fmt = FmtEvent {
            message: String::new(),
            fields: String::new(),
        };
        event.record(&mut fmt);
        let level = event.metadata().level();
        let line = if fmt.fields.is_empty() {
            format!("{level}  {}", fmt.message)
        } else {
            format!("{level}  {}  {}", fmt.message, fmt.fields)
        };
        self.store.push_rdz(&room, line);
    }
}
