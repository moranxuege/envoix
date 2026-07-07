//! Per-room log collection for holistic debugging: peers POST their transfer log,
//! an operator GETs the merged view — the rdz's own room events plus both peers'
//! logs, one page. Keyed by room id (the code's first segment, the same id the
//! broker matches on). In-memory with a TTL; auth is possession of the room id
//! (it's the URL). Runs on a separate HTTP port from the pairing endpoint, so it
//! never touches the SPAKE2 wire protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::post;

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

async fn view(Path(room): Path<String>, State(store): State<Arc<RoomLogs>>) -> (StatusCode, String) {
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
