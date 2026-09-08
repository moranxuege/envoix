//! A service-manager acknowledgement is not a ready Agent control endpoint.

use std::future::Future;
use std::io;
use std::time::Duration;

use envoix_client::agent_control::AgentControlClient;
use envoix_client::product::{AgentRequest, AgentResponse};
use tokio::time::{Instant, sleep_until, timeout_at};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) async fn wait_for_current_user() -> io::Result<()> {
    let client = AgentControlClient::for_current_user()?;
    wait_until_ready(STARTUP_TIMEOUT, POLL_INTERVAL, || async {
        match client.call(AgentRequest::Status).await? {
            AgentResponse::Status { .. } => Ok(()),
            _ => Err(io::Error::other(
                "Agent returned an unexpected readiness response",
            )),
        }
    })
    .await
}

async fn wait_until_ready<F, P>(
    timeout: Duration,
    interval: Duration,
    mut probe: P,
) -> io::Result<()>
where
    P: FnMut() -> F,
    F: Future<Output = io::Result<()>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        match timeout_at(deadline, probe()).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) if Instant::now() < deadline => {
                sleep_until((Instant::now() + interval).min(deadline)).await;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Agent did not become ready before the startup deadline",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Agent did not become ready before the startup deadline",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn waits_through_transient_endpoint_failures() {
        let attempts = Cell::new(0);
        wait_until_ready(Duration::from_secs(1), Duration::from_millis(1), || {
            attempts.set(attempts.get() + 1);
            std::future::ready(if attempts.get() < 3 {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                Ok(())
            })
        })
        .await
        .unwrap();
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn bounds_a_probe_that_never_responds() {
        let error = wait_until_ready(Duration::from_millis(20), Duration::from_millis(1), || {
            std::future::pending::<io::Result<()>>()
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn never_reports_success_for_a_permanently_unavailable_endpoint() {
        let error = wait_until_ready(Duration::from_millis(20), Duration::from_millis(1), || {
            std::future::ready(Err(io::Error::from(io::ErrorKind::ConnectionRefused)))
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
