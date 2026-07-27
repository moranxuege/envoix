use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use envoix_protocol::mailbox::identifiers::RECEIPT_HTTP_ROUTE;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use tokio::sync::oneshot;

use crate::budget::ServiceBudget;
use crate::serve::serve_bounded_http;

#[derive(Clone, Copy)]
pub(crate) struct MailboxLimits {
    pub(crate) ttl: Duration,
    pub(crate) max_blob_size: usize,
    pub(crate) max_key_length: usize,
    pub(crate) max_entries: usize,
}

#[derive(Clone)]
struct MailboxState {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    limits: MailboxLimits,
}

struct Entry {
    blob: Bytes,
    stored_at: Instant,
}

pub(crate) async fn serve(
    listener: std::net::TcpListener,
    limits: MailboxLimits,
    budget: ServiceBudget,
    shutdown: oneshot::Receiver<()>,
) -> Result<(), io::Error> {
    let state = MailboxState {
        entries: Arc::new(Mutex::new(HashMap::new())),
        limits,
    };
    serve_bounded_http(listener, budget, shutdown, move |request| {
        handle(request, state.clone())
    })
    .await
}

async fn handle(request: Request<Incoming>, state: MailboxState) -> Response<Full<Bytes>> {
    let Some(slot) =
        route_slot(request.uri().path(), state.limits.max_key_length).map(str::to_owned)
    else {
        return empty(StatusCode::NOT_FOUND);
    };
    match *request.method() {
        // Writes are unauthenticated: any client that knows the (confidential,
        // transfer-id-derived) slot may replace the blob. The seal makes this a
        // bounded liveness/DoS surface only — a griefer cannot forge a valid
        // receipt, so it can never drive a false completion, only stall polling.
        //
        // Two budgets answer separately, and neither ever stalls a caller:
        // 503 means this service is serving all it may at once, 429 means the
        // store itself is full until a TTL expires.
        Method::POST => {
            let body = Limited::new(request.into_body(), state.limits.max_blob_size);
            let collected = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => return empty(StatusCode::PAYLOAD_TOO_LARGE),
            };
            if collected.is_empty() {
                return empty(StatusCode::BAD_REQUEST);
            }
            let mut entries = state
                .entries
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            evict_expired(&mut entries, state.limits.ttl);
            if !entries.contains_key(&slot) && entries.len() >= state.limits.max_entries {
                return empty(StatusCode::TOO_MANY_REQUESTS);
            }
            entries.insert(
                slot,
                Entry {
                    blob: collected,
                    stored_at: Instant::now(),
                },
            );
            empty(StatusCode::NO_CONTENT)
        }
        Method::GET => {
            let mut entries = state
                .entries
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            evict_expired(&mut entries, state.limits.ttl);
            match entries.get(&slot) {
                Some(entry) => Response::builder()
                    .status(StatusCode::OK)
                    .body(Full::new(entry.blob.clone()))
                    .expect("fixed mailbox response is valid"),
                None => empty(StatusCode::NOT_FOUND),
            }
        }
        _ => empty(StatusCode::METHOD_NOT_ALLOWED),
    }
}

fn route_slot(path: &str, max_key_length: usize) -> Option<&str> {
    let (prefix, suffix) = RECEIPT_HTTP_ROUTE.split_once("{slot}")?;
    if !suffix.is_empty() {
        return None;
    }
    let slot = path.strip_prefix(prefix)?;
    if slot.is_empty() || slot.len() > max_key_length || slot.contains('/') {
        return None;
    }
    Some(slot)
}

fn evict_expired(entries: &mut HashMap<String, Entry>, ttl: Duration) {
    let now = Instant::now();
    entries.retain(|_, entry| now.duration_since(entry.stored_at) < ttl);
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .expect("fixed mailbox response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_accepts_only_one_bounded_opaque_key() {
        assert_eq!(
            route_slot("/api/envoix/mailbox/v2/receipts/opaque", 64),
            Some("opaque")
        );
        assert_eq!(route_slot("/api/envoix/mailbox/v2/receipts/", 64), None);
        assert_eq!(route_slot("/api/envoix/mailbox/v2/receipts/a/b", 64), None);
        assert_eq!(
            route_slot("/api/envoix/mailbox/v2/receipts/toolong", 3),
            None
        );
    }
}
