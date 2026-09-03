//! Per-room log collection for holistic debugging: peers POST their transfer log,
//! an operator GETs the merged view — the rdz's own room events plus both peers'
//! logs, one page. Keyed by room id (the code's first segment, the same id the
//! broker matches on). In-memory with a TTL; the room id is only a correlation
//! key and is never accepted as authorization. Runs on a separate HTTPS port
//! from the pairing endpoint, so it never touches the SPAKE2 wire protocol.
//! Loopback HTTP is reserved for local development or a TLS reverse proxy.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Cap shared by the native diagnostic clients and the collection endpoint.
const MAX_BODY: usize = 480 * 1024;
/// Cap on the rdz's own captured lines per room.
const MAX_RDZ_LINES: usize = 2000;
/// Reject absurd room keys.
const MAX_ROOM_KEY: usize = 64;
/// Memory bounds — authenticated clients can still be buggy or compromised, so
/// the store caps its own footprint against room-id spraying / side flooding.
/// Over-cap uploads are refused with 507.
const MAX_ROOMS: usize = 1024;
const MAX_CLIENTS_PER_ROOM: usize = 4;
const MAX_CLIENT_BYTES: usize = 256 * 1024 * 1024;
/// Per-source limits are intentionally separate: a diagnostic upload must not
/// consume an operator's report-view budget.
const UPLOAD_RATE_EVENTS: u32 = 3;
const UPLOAD_RATE_PERIOD: Duration = Duration::from_secs(60);
const UPLOAD_RATE_BURST: u32 = 5;
const VIEW_RATE_EVENTS: u32 = 60;
const VIEW_RATE_PERIOD: Duration = Duration::from_secs(60);
const VIEW_RATE_BURST: u32 = 120;
const MAX_RATE_LIMIT_STATES: usize = 4096;
const RATE_LIMIT_STATE_TTL: Duration = Duration::from_secs(600);
const RATE_LIMIT_STATE_CAP_RETRY_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum LogOperation {
    Upload,
    View,
}

#[derive(Clone, Copy)]
struct RatePolicy {
    events: u32,
    period: Duration,
    burst: u32,
}

impl LogOperation {
    const fn policy(self) -> RatePolicy {
        match self {
            Self::Upload => RatePolicy {
                events: UPLOAD_RATE_EVENTS,
                period: UPLOAD_RATE_PERIOD,
                burst: UPLOAD_RATE_BURST,
            },
            Self::View => RatePolicy {
                events: VIEW_RATE_EVENTS,
                period: VIEW_RATE_PERIOD,
                burst: VIEW_RATE_BURST,
            },
        }
    }
}

struct RateBucket {
    tokens: f64,
    updated: Instant,
    last_seen: Instant,
}

#[derive(Default)]
struct LogRateLimits {
    buckets: Mutex<HashMap<(IpAddr, LogOperation), RateBucket>>,
}

impl LogRateLimits {
    fn allow(&self, source: IpAddr, operation: LogOperation) -> Result<(), u64> {
        self.allow_at(source, operation, Instant::now())
    }

    fn allow_at(&self, source: IpAddr, operation: LogOperation, now: Instant) -> Result<(), u64> {
        let policy = operation.policy();
        let mut buckets = self.buckets.lock().unwrap();
        buckets.retain(|_, bucket| {
            now.saturating_duration_since(bucket.last_seen) < RATE_LIMIT_STATE_TTL
        });
        let key = (source, operation);
        if !buckets.contains_key(&key) && buckets.len() >= MAX_RATE_LIMIT_STATES {
            return Err(RATE_LIMIT_STATE_CAP_RETRY_SECS);
        }
        let bucket = buckets.entry(key).or_insert_with(|| RateBucket {
            tokens: f64::from(policy.burst),
            updated: now,
            last_seen: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        let refill_per_second = f64::from(policy.events) / policy.period.as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_second).min(f64::from(policy.burst));
        bucket.updated = now;
        bucket.last_seen = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        let retry_after = ((1.0 - bucket.tokens) / refill_per_second).ceil().max(1.0) as u64;
        Err(retry_after)
    }
}

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

/// Router state: bounded source limits plus separate upload and report-view
/// authorization policies. Neither operation treats a Room identifier as a
/// credential.
#[derive(Clone)]
pub struct LogState {
    store: Arc<RoomLogs>,
    rate_limits: Arc<LogRateLimits>,
    upload_auth: UploadAuth,
    view_auth: ViewAuth,
}

/// Uploads are disabled unless the operator configures a bearer token.
#[derive(Clone)]
pub enum UploadAuth {
    Token(Arc<str>),
    Closed,
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
pub fn router(store: Arc<RoomLogs>, upload_auth: UploadAuth, view_auth: ViewAuth) -> Router {
    Router::new()
        .route("/logs/{room}", post(upload).get(view))
        .with_state(LogState {
            store,
            rate_limits: Arc::new(LogRateLimits::default()),
            upload_auth,
            view_auth,
        })
}

async fn upload(
    Path(room): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<LogState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    if let Err(status) = authorize_upload(request.headers(), &state.upload_auth) {
        return status.into_response();
    }
    if let Err(retry_after) = state.rate_limits.allow(peer.ip(), LogOperation::Upload) {
        return rate_limited_response(retry_after);
    }
    if room.len() > MAX_ROOM_KEY {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let body = match to_bytes(request.into_body(), MAX_BODY).await {
        Ok(body) => match String::from_utf8(body.to_vec()) {
            Ok(body) => body,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let status = if state.store.upload(&room, &side_of(query.as_deref()), body) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::INSUFFICIENT_STORAGE
    };
    status.into_response()
}

fn authorize_upload(headers: &HeaderMap, auth: &UploadAuth) -> Result<(), StatusCode> {
    match auth {
        UploadAuth::Token(expected) if bearer_matches(headers, expected) => Ok(()),
        UploadAuth::Token(_) => Err(StatusCode::UNAUTHORIZED),
        UploadAuth::Closed => Err(StatusCode::FORBIDDEN),
    }
}

fn bearer_matches(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
}

async fn view(
    Path(room): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    State(state): State<LogState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    // A room id is a low-entropy correlation key, NOT authorization.
    match &state.view_auth {
        ViewAuth::Token(expected) => {
            if !bearer_matches(&headers, expected) {
                return (
                    StatusCode::UNAUTHORIZED,
                    "operator token required\n".to_string(),
                )
                    .into_response();
            }
        }
        ViewAuth::Open => {}
        ViewAuth::Closed => {
            return (
                StatusCode::FORBIDDEN,
                "report retrieval disabled; set --log-view-token-file or \
                 --unsafe-open-log-view on the server\n"
                    .to_string(),
            )
                .into_response();
        }
    }
    if let Err(retry_after) = state.rate_limits.allow(peer.ip(), LogOperation::View) {
        return rate_limited_response(retry_after);
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
    .into_response()
}

fn rate_limited_response(retry_after: u64) -> Response {
    let mut response = (StatusCode::TOO_MANY_REQUESTS, "rate limited\n").into_response();
    let value = HeaderValue::from_str(&retry_after.to_string())
        .expect("positive integer retry-after is a valid header value");
    response.headers_mut().insert(header::RETRY_AFTER, value);
    response
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
#[path = "logs_tests.rs"]
mod tests;
