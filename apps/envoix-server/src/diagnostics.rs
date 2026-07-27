//! The operator surface: what each service's budget is, and how much of it is
//! spent right now.
//!
//! It is a service like the others — its own listener, its own budget, its own
//! workers — because the moment an operator most needs to ask is the moment
//! another service is saturated. It binds loopback only, so the answer is
//! reachable through an SSH tunnel and not from the internet.

use std::io;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use tokio::sync::oneshot;

use crate::budget::{BudgetMeter, ServiceBudget};
use crate::serve::serve_bounded_http;

/// The single route this service answers, owned by the server's identifier
/// manifest.
pub const BUDGET_HTTP_ROUTE: &str = "/api/envoix/diagnostics/v2/budgets";

pub(crate) async fn serve(
    listener: std::net::TcpListener,
    budget: ServiceBudget,
    meters: [BudgetMeter; 3],
    shutdown: oneshot::Receiver<()>,
) -> Result<(), io::Error> {
    let own = budget.meter();
    serve_bounded_http(listener, budget, shutdown, move |request| {
        let meters = meters.clone();
        let own = own.clone();
        async move {
            own.record_worker();
            handle(&request, &meters)
        }
    })
    .await
}

fn handle(request: &Request<Incoming>, meters: &[BudgetMeter; 3]) -> Response<Full<Bytes>> {
    if request.uri().path() != BUDGET_HTTP_ROUTE {
        return status(StatusCode::NOT_FOUND);
    }
    if request.method() != Method::GET {
        return status(StatusCode::METHOD_NOT_ALLOWED);
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(Full::new(Bytes::from(render(meters))))
        .expect("fixed diagnostics response is valid")
}

/// Hand-written because the shape is fixed and every field is a number or an
/// identifier this crate owns; nothing a caller sends reaches it.
fn render(meters: &[BudgetMeter; 3]) -> String {
    let mut body = String::from("{\"budget\":[");
    for (index, meter) in meters.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            "{{\"service\":\"{}\",\"capacity\":{},\"in_flight\":{},\"admitted\":{},\"refused\":{},\"worker\":{}}}",
            meter.service().as_str(),
            meter.capacity(),
            meter.in_flight(),
            meter.admitted(),
            meter.refused(),
            match meter.worker() {
                Some(worker) => format!("\"{worker}\""),
                None => "null".to_owned(),
            }
        ));
    }
    body.push_str("]}");
    body
}

fn status(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .expect("fixed diagnostics response is valid")
}

#[cfg(test)]
mod tests {
    use crate::budget::{BudgetPlan, Service, ServiceBudgets};

    use super::*;

    #[test]
    fn the_route_matches_the_identifier_manifest() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../server-identifiers.toml")).unwrap();
        assert_eq!(
            manifest["diagnostics"]["http_route"].as_str(),
            Some(BUDGET_HTTP_ROUTE)
        );
    }

    #[test]
    fn the_readout_names_every_service_and_its_spend() {
        let budgets = ServiceBudgets::build(|_| BudgetPlan {
            max_concurrent: 3,
            worker_threads: 1,
        })
        .unwrap();
        let held = budgets.mailbox.0.try_admit().expect("capacity 3");
        let body = render(&budgets.meters());
        drop(held);

        for service in Service::ALL {
            assert!(
                body.contains(&format!("\"service\":\"{}\"", service.as_str())),
                "{body}"
            );
        }
        assert!(body.contains("\"capacity\":3"));
        assert!(
            body.contains("\"in_flight\":1"),
            "the held mailbox slot must show: {body}"
        );
    }
}
