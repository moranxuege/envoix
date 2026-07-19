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
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Cap on a single uploaded log body. Generous for the debug era (full,
/// untrimmed uploads — server space is not a concern pre-release); revisit
/// with a retention/rotation policy before release.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Cap on the rdz's own captured lines per room.
const MAX_RDZ_LINES: usize = 2000;
/// Reject absurd room keys.
const MAX_ROOM_KEY: usize = 64;
/// Memory bounds — the log store accepts UNAUTHENTICATED POSTs, so it must cap
/// its own footprint against room-id spraying / side flooding (the sibling
/// receipts + broker stores already cap this way; this store was the gap).
/// Over-cap uploads are refused with 507.
const MAX_ROOMS: usize = 1024;
const MAX_CLIENTS_PER_ROOM: usize = 4;
const MAX_CLIENT_BYTES: usize = 256 * 1024 * 1024;

/// Collected logs for one room.
#[derive(Default)]
struct RoomEntry {
    updated: Option<Instant>,
    /// The rdz's own events for this room (captured from its tracing), each with
    /// the wall-clock `epoch_ms` it was captured at — the broker's lane in the
    /// time-merge (docs/design/diagnostics.md v2, P5).
    rdz: Vec<(u64, String)>,
    /// Each peer's uploaded log, keyed by side ("send"/"receive"/…).
    clients: Vec<(String, String)>,
}

/// Wall-clock milliseconds since the epoch (the broker's clock — one lane in the
/// skew-sensitive time-merge).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    pub fn push_rdz(&self, room: &str, epoch_ms: u64, line: String) {
        if room.len() > MAX_ROOM_KEY {
            return;
        }
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        if !rooms.contains_key(room) && rooms.len() >= MAX_ROOMS {
            return;
        }
        let entry = rooms.entry(room.to_string()).or_default();
        entry.rdz.push((epoch_ms, line));
        if entry.rdz.len() > MAX_RDZ_LINES {
            entry.rdz.drain(0..entry.rdz.len() - MAX_RDZ_LINES);
        }
        entry.updated = Some(Instant::now());
    }

    /// Store a peer's uploaded log, replacing any prior upload for the same side.
    /// Returns false when a memory cap (room count, sides/room, or aggregate
    /// bytes) would be exceeded — the store is open to unauthenticated POSTs, so
    /// it bounds its own memory.
    fn upload(&self, room: &str, side: &str, body: String) -> bool {
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        if !rooms.contains_key(room) && rooms.len() >= MAX_ROOMS {
            return false;
        }
        // Aggregate byte cap: current total, minus this side's prior body that
        // this upload replaces, plus the new body, must stay under the cap.
        // `replaced` is always a subset of `stored`, so the subtraction is safe.
        let replaced = rooms.get(room).map_or(0, |e| {
            e.clients
                .iter()
                .find(|(s, _)| s == side)
                .map_or(0, |(_, b)| b.len())
        });
        let stored: usize = rooms
            .values()
            .flat_map(|e| e.clients.iter())
            .map(|(_, b)| b.len())
            .sum();
        if stored.saturating_sub(replaced) + body.len() > MAX_CLIENT_BYTES {
            return false;
        }
        // Per-room side-count cap — only a genuinely new side counts (a re-upload
        // for an existing side replaces, it does not grow cardinality).
        let new_side = rooms
            .get(room)
            .is_none_or(|e| !e.clients.iter().any(|(s, _)| s == side));
        if new_side
            && rooms
                .get(room)
                .is_some_and(|e| e.clients.len() >= MAX_CLIENTS_PER_ROOM)
        {
            return false;
        }
        let entry = rooms.entry(room.to_string()).or_default();
        entry.clients.retain(|(s, _)| s != side);
        entry.clients.push((side.to_string(), body));
        entry.updated = Some(Instant::now());
        true
    }

    /// The canonical view: one ordered lane per source (rdz, then each peer's
    /// upload verbatim). Authoritative — no cross-source clock comparison (P5).
    fn view(&self, room: &str) -> Option<String> {
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        let entry = rooms.get(room)?;
        let mut out = String::new();
        if !entry.rdz.is_empty() {
            out.push_str(&format!("═════ rdz · room {room} ═════\n"));
            for (_, line) in &entry.rdz {
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

    /// The SECONDARY time-merged view (`?merge=time`): every timeline line from
    /// every source, interleaved by `epoch_ms`, side-tagged. Explicitly labelled
    /// skew-sensitive — sender/receiver/broker clocks are independent, so this
    /// can invent a plausible-but-false causal order; the per-source lanes above
    /// are the truth. Raw/non-timeline lines (no epoch) are omitted here.
    fn merge_view(&self, room: &str) -> Option<String> {
        let mut rooms = self.rooms.lock().unwrap();
        evict(&mut rooms, self.ttl);
        let entry = rooms.get(room)?;
        let mut rows: Vec<(u64, &str, &str)> = Vec::new();
        for (epoch, line) in &entry.rdz {
            rows.push((*epoch, "rdz", line.as_str()));
        }
        for (side, body) in &entry.clients {
            for line in body.lines() {
                if let Some(epoch) = timeline_epoch(line) {
                    rows.push((epoch, side.as_str(), line));
                }
            }
        }
        // Stable sort: within one clock tick, insertion (per-lane) order holds.
        rows.sort_by_key(|(epoch, _, _)| *epoch);
        let mut out = String::new();
        out.push_str(&format!(
            "═════ time-merged · room {room} · ⚠ SKEW-SENSITIVE ═════\n\
             (sender / receiver / broker clocks are independent — this interleave\n\
              can imply a false causal order. The per-source lanes (default view)\n\
              are authoritative. Columns: epoch_ms  [side]  line)\n\n"
        ));
        for (epoch, side, line) in rows {
            out.push_str(&format!("{epoch}  [{side:<7}]  {line}\n"));
        }
        Some(out)
    }
}

/// The `epoch_ms` (column 2) of a timeline-envelope line — `seq⇥schema⇥epoch⇥…`
/// with the three leading columns all digits and epoch exactly 13 wide — or None
/// for a raw / header / non-timeline line.
fn timeline_epoch(line: &str) -> Option<u64> {
    let mut cols = line.split('\t');
    let seq = cols.next()?;
    let schema = cols.next()?;
    let epoch = cols.next()?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if digits(seq) && digits(schema) && epoch.len() == 13 && digits(epoch) {
        epoch.parse().ok()
    } else {
        None
    }
}

fn evict(rooms: &mut HashMap<String, RoomEntry>, ttl: Duration) {
    let now = Instant::now();
    rooms.retain(|_, e| e.updated.is_some_and(|u| now.duration_since(u) < ttl));
}

/// Length-checked constant-time byte compare for the operator token (no early
/// return on the first mismatched byte → no timing side-channel; the token
/// length itself is not sensitive).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Router state: the store, plus the optional operator token that gates report
/// RETRIEVAL (GET). Uploads (POST) stay open — peers cannot hold an operator
/// secret and must still be able to upload.
#[derive(Clone)]
pub struct LogState {
    store: Arc<RoomLogs>,
    view_auth: ViewAuth,
}

/// How report RETRIEVAL (GET) is gated. Default is fail-CLOSED — an open
/// endpoint must be a deliberate, visible choice, not a silent default, so
/// security does not depend on remembering to configure a token.
#[derive(Clone)]
pub enum ViewAuth {
    /// A bearer token is required (from `--log-view-token-file`).
    Token(Arc<str>),
    /// Explicitly opened via `--unsafe-open-log-view` (anonymous reads).
    Open,
    /// Default: reads are refused (403) until a token or the unsafe flag is set.
    Closed,
}

/// The axum router: `POST /logs/{room}?side=…` ingests, `GET /logs/{room}` views.
pub fn router(store: Arc<RoomLogs>, view_auth: ViewAuth) -> Router {
    Router::new()
        .route("/logs/{room}", post(upload).get(view))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(LogState { store, view_auth })
}

async fn upload(
    Path(room): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<LogState>,
    body: String,
) -> StatusCode {
    if room.len() > MAX_ROOM_KEY {
        return StatusCode::BAD_REQUEST;
    }
    if state.store.upload(&room, &side_of(query.as_deref()), body) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::INSUFFICIENT_STORAGE
    }
}

async fn view(
    Path(room): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    State(state): State<LogState>,
) -> (StatusCode, String) {
    // A room id is a low-entropy correlation key, NOT authorization.
    match &state.view_auth {
        ViewAuth::Token(expected) => {
            let ok = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()));
            if !ok {
                return (
                    StatusCode::UNAUTHORIZED,
                    "operator token required\n".to_string(),
                );
            }
        }
        ViewAuth::Open => {}
        ViewAuth::Closed => {
            return (
                StatusCode::FORBIDDEN,
                "report retrieval disabled; set --log-view-token-file or \
                 --unsafe-open-log-view on the server\n"
                    .to_string(),
            );
        }
    }
    // Default is the canonical per-source lanes; `?merge=time` is the secondary
    // skew-sensitive interleave.
    let merged = query
        .as_deref()
        .is_some_and(|q| q.split('&').any(|kv| kv == "merge=time"));
    let result = if merged {
        state.store.merge_view(&room)
    } else {
        state.store.view(&room)
    };
    match result {
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
        self.store.push_rdz(&room, now_ms(), line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_epoch_parses_envelope_and_rejects_raw() {
        assert_eq!(
            timeline_epoch("12\t1\t1783875675799\t8045\t3\t\tprotocol\tcomplete_ack\tsent"),
            Some(1783875675799),
        );
        // raw iroh line, header line, and a too-short epoch all yield None.
        assert_eq!(timeline_epoch("13:00:47  DEBUG  data path: direct"), None);
        assert_eq!(timeline_epoch("═════ send ═════"), None);
        assert_eq!(timeline_epoch("1\t1\t123\tx"), None);
    }

    #[test]
    fn merge_interleaves_sources_by_epoch() {
        let store = RoomLogs::new(Duration::from_secs(60));
        store.push_rdz("r", 100, "INFO  paired".to_string());
        // 13-digit epochs; receive (200) predates send (300).
        assert!(store.upload(
            "r",
            "send",
            "0\t1\t0000000000300\t9\t3\t\tmachine\ttransition\t\n".to_string()
        ));
        assert!(store.upload(
            "r",
            "receive",
            "0\t1\t0000000000200\t9\t5\t\tsession\tcreated\t\n".to_string(),
        ));
        let merged = store.merge_view("r").unwrap();
        let rdz = merged.find("[rdz").unwrap();
        let recv = merged.find("[receive").unwrap();
        let send = merged.find("[send").unwrap();
        assert!(
            rdz < recv && recv < send,
            "interleaved by epoch across sources"
        );
    }

    #[test]
    fn per_room_side_cap_rejects_new_but_allows_replace() {
        let store = RoomLogs::new(Duration::from_secs(60));
        for i in 0..MAX_CLIENTS_PER_ROOM {
            assert!(
                store.upload("r", &format!("s{i}"), "x".to_string()),
                "side {i} within cap"
            );
        }
        assert!(
            !store.upload("r", "overflow", "x".to_string()),
            "a new side past the cap is refused"
        );
        assert!(
            store.upload("r", "s0", "y".to_string()),
            "re-upload of an existing side still accepted"
        );
    }

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq(b"operator-token", b"operator-token"));
        assert!(!constant_time_eq(b"operator-token", b"operator-toke")); // length differs
        assert!(!constant_time_eq(b"operator-token", b"operator-tokeX")); // last byte differs
        assert!(!constant_time_eq(b"", b"x"));
    }
}
