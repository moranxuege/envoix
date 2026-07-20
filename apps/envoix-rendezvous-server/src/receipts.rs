//! Sealed completion-receipt mailbox (the magic-wormhole mailbox pattern,
//! scoped to one message type). When a transfer's final CompleteAck is lost,
//! the receiver posts its completion receipt here — the rdz is the one party
//! that is always online — and the unconfirmed sender fetches it later, with
//! no receiver presence needed.
//!
//! The rdz stays blind: keys are `blake3(transfer id)` (opaque; retrieval is
//! gated on knowing the high-entropy transfer id) and blobs are sealed by the
//! peers under a key derived from transfer id + pairing code. The server
//! stores bytes it can neither read nor correlate. In-memory with a TTL —
//! a restart drops pending receipts, which peers recover from via the
//! peer-present re-verify path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::post;

/// Cap on a sealed receipt blob (a receipt is ~200 bytes sealed).
const MAX_BLOB: usize = 4 * 1024;
/// Reject absurd mailbox keys (blake3 hex is 64).
const MAX_KEY: usize = 128;
/// Cap on concurrently stored receipts, to bound memory under abuse.
const MAX_ENTRIES: usize = 10_000;

/// Mailbox key → (stored-at, sealed blob), evicted after `ttl`.
pub struct ReceiptStore {
    ttl: Duration,
    entries: Mutex<HashMap<String, (Instant, Vec<u8>)>>,
}

impl ReceiptStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn put(&self, key: String, blob: Vec<u8>) -> StatusCode {
        let mut entries = self.entries.lock().unwrap();
        let now = Instant::now();
        entries.retain(|_, (stored, _)| now.duration_since(*stored) < self.ttl);
        if entries.len() >= MAX_ENTRIES && !entries.contains_key(&key) {
            tracing::warn!("receipt store full");
            return StatusCode::INSUFFICIENT_STORAGE;
        }
        entries.insert(key, (now, blob));
        StatusCode::NO_CONTENT
    }

    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let entries = self.entries.lock().unwrap();
        entries
            .get(key)
            .filter(|(stored, _)| stored.elapsed() < self.ttl)
            .map(|(_, blob)| blob.clone())
    }
}

pub fn router(store: Arc<ReceiptStore>) -> Router {
    Router::new()
        .route("/receipts/{key}", post(put_receipt).get(get_receipt))
        .layer(DefaultBodyLimit::max(MAX_BLOB))
        .with_state(store)
}

async fn put_receipt(
    State(store): State<Arc<ReceiptStore>>,
    Path(key): Path<String>,
    body: Bytes,
) -> StatusCode {
    if key.is_empty() || key.len() > MAX_KEY {
        return StatusCode::BAD_REQUEST;
    }
    if body.is_empty() || body.len() > MAX_BLOB {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    tracing::debug!(key = %&key[..key.len().min(12)], len = body.len(), "receipt stored");
    store.put(key, body.to_vec())
}

async fn get_receipt(
    State(store): State<Arc<ReceiptStore>>,
    Path(key): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(StatusCode::BAD_REQUEST);
    }
    store.get(&key).ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
#[path = "receipts_tests.rs"]
mod tests;
