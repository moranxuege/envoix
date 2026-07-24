use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use envoix_protocol::mailbox::identifiers::RECEIPT_HTTP_ROUTE;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

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
    listener: TcpListener,
    limits: MailboxLimits,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), io::Error> {
    let state = MailboxState {
        entries: Arc::new(Mutex::new(HashMap::new())),
        limits,
    };
    let mut connections = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => {
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                return Ok(());
            }
            _ = connections.join_next(), if !connections.is_empty() => continue,
            accepted = listener.accept() => accepted,
        };
        let (stream, _) = accepted?;
        let state = state.clone();
        connections.spawn(async move {
            let service = service_fn(move |request| handle(request, state.clone()));
            if http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .is_err()
            {
                tracing::warn!("mailbox HTTP connection failed");
            }
        });
    }
}

async fn handle(
    request: Request<Incoming>,
    state: MailboxState,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let Some(slot) =
        route_slot(request.uri().path(), state.limits.max_key_length).map(str::to_owned)
    else {
        return Ok(empty(StatusCode::NOT_FOUND));
    };
    match *request.method() {
        // Writes are unauthenticated: any client that knows the (confidential,
        // transfer-id-derived) slot may replace the blob. The seal makes this a
        // bounded liveness/DoS surface only — a griefer cannot forge a valid
        // receipt, so it can never drive a false completion, only stall polling.
        // Real admission control (rate/quota/auth) is D1-deferred.
        Method::POST => {
            let body = Limited::new(request.into_body(), state.limits.max_blob_size);
            let collected = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => return Ok(empty(StatusCode::PAYLOAD_TOO_LARGE)),
            };
            if collected.is_empty() {
                return Ok(empty(StatusCode::BAD_REQUEST));
            }
            let mut entries = state
                .entries
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            evict_expired(&mut entries, state.limits.ttl);
            if !entries.contains_key(&slot) && entries.len() >= state.limits.max_entries {
                return Ok(empty(StatusCode::TOO_MANY_REQUESTS));
            }
            entries.insert(
                slot,
                Entry {
                    blob: collected,
                    stored_at: Instant::now(),
                },
            );
            Ok(empty(StatusCode::NO_CONTENT))
        }
        Method::GET => {
            let mut entries = state
                .entries
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            evict_expired(&mut entries, state.limits.ttl);
            match entries.get(&slot) {
                Some(entry) => Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Full::new(entry.blob.clone()))
                    .expect("fixed mailbox response is valid")),
                None => Ok(empty(StatusCode::NOT_FOUND)),
            }
        }
        _ => Ok(empty(StatusCode::METHOD_NOT_ALLOWED)),
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
