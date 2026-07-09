//! Room rendezvous transfer: pair two peers via the rendezvous broker using a
//! short code, then transfer over iroh with the existing data plane.
//!
//! The rendezvous only finds + authenticates the peers and exchanges their iroh
//! addresses; the file transfer itself is the unchanged `send_file_manual` /
//! `receive_one_authenticated` path, authenticated with a token derived from the
//! pairing key (so the data-plane SPAKE2 still runs and is channel-bound).

use std::path::PathBuf;
use std::time::Duration;

use envoix_error::CoreError;
use envoix_rendezvous_iroh::{RoomPairing, build_endpoint, drive_pairing, join_room, split_code};
use envoix_types::PairingStep;
use iroh::{Endpoint, EndpointAddr, SecretKey};

use crate::{
    BindAddrs, EventSink, PairingConfig, SessionConfig, SessionError, TransferCancelToken,
    TransferEvent, TransferSummary, bind_iroh_endpoint_with_relay, receive_with_auth_retries,
    send_file_to_endpoint_addr,
};

const SEND_ROOM_PAIRING_TIMEOUT: Duration = Duration::from_secs(35);

/// An ephemeral iroh endpoint used only to reach the rendezvous broker, routed
/// through `relay` (a relay URL) when set so it can reach a NATed broker.
async fn rendezvous_endpoint(relay: &Option<String>) -> Result<Endpoint, SessionError> {
    build_endpoint(
        "0.0.0.0:0".parse().expect("valid addr"),
        SecretKey::generate(),
        crate::endpoint::relay_mode(relay)?,
    )
    .await
    .map_err(|error| CoreError::Transport(error.to_string()))
}

/// Pair in a room, re-joining if the broker matched us with a stale dead peer.
/// `join_room` blocks until the broker matches us, so it never cuts an honest
/// wait short. Once matched, the SPAKE2 exchange with a live partner takes
/// milliseconds, so if it stalls past `EXCHANGE_TIMEOUT` the partner is a dead
/// peer left by an earlier run (iroh has not yet noticed its connection is gone).
/// We drop it and re-join - that failed match already consumed the dead peer, so
/// the next join reaches a live partner (or parks to wait for one).
async fn pair_in_room_retrying<T>(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    room_id: &str,
    password: &str,
    mine: &T,
    events: &dyn EventSink,
) -> Result<RoomPairing<T>, SessionError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    const ATTEMPTS: usize = 4;
    const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(8);
    let mut last: Option<SessionError> = None;
    for _ in 0..ATTEMPTS {
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Joining,
        });
        let session = join_room(rdz, broker.clone(), room_id)
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Matched,
        });
        match tokio::time::timeout(EXCHANGE_TIMEOUT, drive_pairing(session, password, mine)).await {
            Ok(Ok(pairing)) => {
                events.on_event(TransferEvent::Pairing {
                    step: PairingStep::Exchanged,
                });
                return Ok(pairing);
            }
            Ok(Err(error)) => last = Some(CoreError::Transport(error.to_string())),
            Err(_) => last = Some(CoreError::Transport("rendezvous pairing stalled".into())),
        }
    }
    Err(last.expect("at least one attempt failed"))
}

/// Run the room pairing but abandon it - closing the rendezvous endpoint - if
/// cancellation is requested. `join_room` blocks until the broker matches a
/// partner (up to the room TTL) and that wait does not otherwise watch the
/// cancel token, so a Ctrl-C while waiting for a partner would hang; this lets
/// it exit promptly and cleanly instead. `rdz` is also closed on a pairing
/// error so it never drops without a graceful close.
async fn pair_or_cancel<T>(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    room_id: &str,
    password: &str,
    mine: &T,
    cancel: &TransferCancelToken,
    events: &dyn EventSink,
    timeout: Option<Duration>,
) -> Result<RoomPairing<T>, SessionError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let result = if let Some(timeout) = timeout {
        tokio::select! {
            result = pair_in_room_retrying(rdz, broker, room_id, password, mine, events) => result,
            _ = cancel.cancelled() => Err(CoreError::Transfer(crate::USER_INTERRUPT_MESSAGE.into())),
            _ = tokio::time::sleep(timeout) => {
                Err(CoreError::Transport("rendezvous pairing timed out".into()))
            }
        }
    } else {
        tokio::select! {
            result = pair_in_room_retrying(rdz, broker, room_id, password, mine, events) => result,
            _ = cancel.cancelled() => Err(CoreError::Transfer(crate::USER_INTERRUPT_MESSAGE.into())),
        }
    };
    if result.is_err() {
        rdz.close().await;
    }
    result
}

/// Receive a file by pairing in a room: bind the data endpoint, exchange its
/// descriptor with the sender over the broker (SPAKE2 with `code`), then accept
/// the transfer using the token derived from the pairing.
pub async fn receive_file_via_room(
    broker: EndpointAddr,
    code: &str,
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let (room_id, password) = split_code(code);
    let bound = bind_iroh_endpoint_with_relay(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
    )
    .await?;
    // With direct-only the data endpoint has no relay home, so wait for a direct
    // addr rather than a relay home; otherwise wait for the relay home as usual.
    let my_addr = bound
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;

    let rdz = rendezvous_endpoint(&config.relay).await?;
    let pairing = match pair_or_cancel(
        &rdz,
        &broker,
        room_id,
        password,
        &my_addr,
        &cancel,
        events.as_ref(),
        None,
    )
    .await
    {
        Ok(pairing) => pairing,
        Err(error) => {
            // Pairing was cancelled or failed (e.g. room expiry); close both the
            // rendezvous endpoint and our data listener so neither drops without
            // a graceful close.
            rdz.close().await;
            bound.local_endpoint.close().await;
            return Err(error);
        }
    };
    // The rendezvous endpoint is only needed for the broker handshake; close it
    // so it does not linger (and log) while the data transfer runs.
    rdz.close().await;

    events.on_event(TransferEvent::Connecting);
    // Accept with retries: a stray or wrong-token dial must not kill the
    // transfer before the legitimate sender connects.
    let auth = PairingConfig::Spake2SharedToken {
        token: pairing.token,
    };
    receive_with_auth_retries(bound, output_dir, config, &auth, events, cancel).await
}

/// Send a file by pairing in a room: exchange descriptors with the receiver over
/// the broker (SPAKE2 with `code`), then dial the receiver and send using the
/// token derived from the pairing.
pub async fn send_file_via_room(
    broker: EndpointAddr,
    code: &str,
    file_path: PathBuf,
    resume: bool,
    config: SessionConfig,
    events: Box<dyn EventSink>,
    cancel: TransferCancelToken,
) -> Result<TransferSummary, SessionError> {
    let (room_id, password) = split_code(code);
    let rdz = rendezvous_endpoint(&config.relay).await?;
    // The receiver ignores the sender's payload (the sender only dials), so any
    // valid endpoint address works as a placeholder.
    let placeholder = rdz.addr();

    let pairing = match pair_or_cancel(
        &rdz,
        &broker,
        room_id,
        password,
        &placeholder,
        &cancel,
        events.as_ref(),
        Some(SEND_ROOM_PAIRING_TIMEOUT),
    )
    .await
    {
        Ok(pairing) => pairing,
        Err(error) => {
            // Close the rendezvous endpoint before returning so a pairing
            // failure does not drop it ungracefully.
            rdz.close().await;
            return Err(error);
        }
    };
    // The rendezvous endpoint is only needed for the broker handshake; close it
    // so it does not linger (and log) while the data transfer runs.
    rdz.close().await;

    let auth = PairingConfig::Spake2SharedToken {
        token: pairing.token,
    };
    send_file_to_endpoint_addr(
        pairing.peer,
        file_path,
        resume,
        config,
        &auth,
        events,
        cancel,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use envoix_rendezvous::RoomRegistry;
    use envoix_rendezvous_iroh::{endpoint_addr, serve_endpoint};
    use iroh::RelayMode;

    use super::*;
    use crate::NoopEventSink;

    async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
        for _ in 0..100 {
            if ep.addr().ip_addrs().next().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        endpoint_addr(ep)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_room_pairing_timeout_returns_before_room_expiry() {
        let server = build_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            SecretKey::generate(),
            RelayMode::Disabled,
        )
        .await
        .unwrap();
        let broker = ready_addr(&server).await;
        tokio::spawn(serve_endpoint(
            server,
            Arc::new(RoomRegistry::with_ttl(Duration::from_secs(30))),
            None,
        ));

        let rdz = rendezvous_endpoint(&None).await.unwrap();
        let placeholder = rdz.addr();
        let started = tokio::time::Instant::now();
        let result = pair_or_cancel(
            &rdz,
            &broker,
            "9999",
            "lonely-room",
            &placeholder,
            &TransferCancelToken::new(),
            &NoopEventSink,
            Some(Duration::from_millis(150)),
        )
        .await;
        let error = match result {
            Ok(_) => panic!("pairing should time out without a peer"),
            Err(error) => error,
        };

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout should beat the broker room expiry"
        );
        assert!(
            error.to_string().contains("rendezvous pairing timed out"),
            "expected pairing timeout, got: {error}"
        );
    }
}
