//! The one way an HTTP service on this server accepts a caller.
//!
//! Both HTTP lanes share this loop so that "admitted within my own budget, or
//! refused now" is a property of the server rather than of each lane's author.

use std::convert::Infallible;
use std::io;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::budget::{BudgetMeter, ServiceBudget};

/// What a refused caller is told: an immediate, complete HTTP answer naming the
/// condition, never a dropped connection and never a wait.
const REFUSAL: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
retry-after: 1\r\n\
content-length: 0\r\n\
connection: close\r\n\
\r\n";

/// How long delivering a refusal may take before the caller is abandoned. It
/// bounds a path that exists to protect a budget, so it cannot be allowed to
/// become a way to hold one.
const REFUSAL_DEADLINE: Duration = Duration::from_millis(500);

pub(crate) async fn serve_bounded_http<H, F>(
    listener: std::net::TcpListener,
    budget: ServiceBudget,
    mut shutdown: oneshot::Receiver<()>,
    handler: H,
) -> Result<(), io::Error>
where
    H: Fn(Request<Incoming>) -> F + Clone + Send + 'static,
    F: Future<Output = Response<Full<Bytes>>> + Send + 'static,
{
    // Registered here rather than by the caller: a tokio socket belongs to the
    // reactor that created it, and this service's reactor is its own.
    let listener = TcpListener::from_std(listener)?;
    let meter = budget.meter();
    let mut connections = JoinSet::new();
    let mut refusals = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => {
                connections.abort_all();
                refusals.abort_all();
                while connections.join_next().await.is_some() {}
                while refusals.join_next().await.is_some() {}
                return Ok(());
            }
            _ = connections.join_next(), if !connections.is_empty() => continue,
            _ = refusals.join_next(), if !refusals.is_empty() => continue,
            accepted = listener.accept() => accepted,
        };
        let (stream, _) = accepted?;
        let Some(admission) = budget.try_admit() else {
            refuse(stream, &meter, &mut refusals);
            continue;
        };
        let handler = handler.clone();
        let meter = meter.clone();
        connections.spawn(async move {
            let _admission = admission;
            meter.record_worker();
            let service = service_fn(move |request| {
                let handler = handler.clone();
                async move { Ok::<_, Infallible>(handler(request).await) }
            });
            if http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .is_err()
            {
                tracing::warn!(service = meter.service().as_str(), "HTTP connection failed");
            }
        });
    }
}

/// Delivering a refusal never spends the budget being protected: it runs in a
/// separately bounded set of short-lived tasks. When even that is full the
/// connection is closed unanswered, which is a worse answer but still an
/// immediate one.
fn refuse(stream: TcpStream, meter: &BudgetMeter, refusals: &mut JoinSet<()>) {
    meter.record_refused();
    tracing::warn!(
        service = meter.service().as_str(),
        capacity = meter.capacity(),
        "refused: this service's budget is full"
    );
    if refusals.len() >= meter.capacity() {
        return;
    }
    refusals.spawn(deliver_refusal(stream));
}

/// The response, then an orderly close. Closing on a socket whose request bytes
/// are still unread would reset the connection, and the caller would see a
/// broken pipe where an answer should have been.
async fn deliver_refusal(mut stream: TcpStream) {
    let _ = timeout(REFUSAL_DEADLINE, async {
        stream.write_all(REFUSAL).await?;
        stream.shutdown().await?;
        let mut drain = [0; 512];
        while stream.read(&mut drain).await? > 0 {}
        Ok::<(), io::Error>(())
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_its_status_and_asks_for_a_retry() {
        let refusal = std::str::from_utf8(REFUSAL).unwrap();
        assert!(refusal.starts_with("HTTP/1.1 503 Service Unavailable"));
        assert!(refusal.contains("retry-after: 1"));
        assert!(
            refusal.ends_with("\r\n\r\n"),
            "a refused caller gets a complete response, not a truncated one"
        );
    }
}
