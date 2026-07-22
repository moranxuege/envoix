//! Room rendezvous transfer: pair two peers via the rendezvous broker using a
//! short code, then transfer over iroh with the existing data plane.
//!
//! The rendezvous only finds + authenticates the peers and exchanges their iroh
//! addresses; the data transfer then uses either the compatible single-file
//! path or the additive ALPN-negotiated Manifest path, authenticated with a
//! token derived from the pairing key (so data-plane SPAKE2 still runs and is
//! channel-bound).

use std::path::PathBuf;
use std::time::Duration;

use envoix_error::CoreError;
use envoix_rendezvous_iroh::{
    JoinIntent, RoomPairing, build_endpoint_with_dns, drive_pairing, join_room_with_intent,
    split_code,
};
use envoix_types::PairingStep;
use iroh::{Endpoint, EndpointAddr, SecretKey};

use crate::{
    BindAddrs, BoundEndpoint, EventSink, ManifestSendRequest, ManifestTransferSummary,
    PairingConfig, SessionConfig, SessionError, SessionEventSink, SessionTransferSummary,
    TransferCancelToken, TransferEvent, TransferSummary, bind_iroh_endpoint_with_relay,
    bind_iroh_transfer_endpoint_with_relay, receive_transfer_with_auth_retries,
    receive_with_auth_retries, send_file_to_endpoint_addr, send_manifest_to_endpoint_addr,
};

const SEND_ROOM_PAIRING_TIMEOUT: Duration = Duration::from_secs(35);

/// An ephemeral iroh endpoint used only to reach the rendezvous broker, routed
/// through `relay` (a relay URL) when set so it can reach a NATed broker.
async fn rendezvous_endpoint(relay: &Option<String>) -> Result<Endpoint, SessionError> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let dns_resolver = Some(crate::endpoint::platform_system_dns_resolver());
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let dns_resolver = None;

    build_endpoint_with_dns(
        "0.0.0.0:0".parse().expect("valid addr"),
        SecretKey::generate(),
        crate::endpoint::relay_mode(relay)?,
        dns_resolver,
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
    intent: JoinIntent,
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
        let session = join_room_with_intent(rdz, broker.clone(), room_id, Some(intent))
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Matched,
        });
        match tokio::time::timeout(EXCHANGE_TIMEOUT, drive_pairing(session, password, mine)).await {
            Ok(Ok(pairing)) => {
                events.on_event(TransferEvent::Pairing {
                    step: PairingStep::Confirming,
                });
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
#[allow(clippy::too_many_arguments)]
async fn pair_or_cancel<T>(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    room_id: &str,
    password: &str,
    mine: &T,
    intent: JoinIntent,
    cancel: &TransferCancelToken,
    events: &dyn EventSink,
    timeout: Option<Duration>,
) -> Result<RoomPairing<T>, SessionError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let result = if let Some(timeout) = timeout {
        tokio::select! {
            result = pair_in_room_retrying(rdz, broker, room_id, password, mine, intent, events) => result,
            _ = cancel.cancelled() => Err(CoreError::Transfer(interrupt_message(cancel).into())),
            _ = tokio::time::sleep(timeout) => {
                Err(CoreError::Transport("rendezvous pairing timed out".into()))
            }
        }
    } else {
        tokio::select! {
            result = pair_in_room_retrying(rdz, broker, room_id, password, mine, intent, events) => result,
            _ = cancel.cancelled() => Err(CoreError::Transfer(interrupt_message(cancel).into())),
        }
    };
    if result.is_err() {
        rdz.close().await;
    }
    result
}

fn interrupt_message(cancel: &TransferCancelToken) -> &'static str {
    if cancel.is_pause() {
        crate::USER_PAUSE_MESSAGE
    } else {
        crate::USER_INTERRUPT_MESSAGE
    }
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
    let bound = bind_iroh_endpoint_with_relay(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let auth = pair_room_receiver(&bound, broker, code, &config, events.as_ref(), &cancel).await?;
    receive_with_auth_retries(bound, output_dir, config, &auth, events, cancel).await
}

/// Receive a negotiated single-file or Manifest transfer through the existing
/// room rendezvous flow.
pub async fn receive_transfer_via_room(
    broker: EndpointAddr,
    code: &str,
    listen_addrs: impl Into<BindAddrs>,
    output_dir: PathBuf,
    config: SessionConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<SessionTransferSummary, SessionError> {
    let bound = bind_iroh_transfer_endpoint_with_relay(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let auth = pair_room_receiver(&bound, broker, code, &config, events.as_ref(), &cancel).await?;
    receive_transfer_with_auth_retries(bound, output_dir, config, &auth, events, cancel).await
}

async fn pair_room_receiver(
    bound: &BoundEndpoint,
    broker: EndpointAddr,
    code: &str,
    config: &SessionConfig,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
) -> Result<PairingConfig, SessionError> {
    let (room_id, password) = split_code(code);
    // With direct-only the data endpoint has no relay home, so wait for a direct
    // addr rather than a relay home; otherwise wait for the relay home as usual.
    let my_addr = bound
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;

    let rdz = match rendezvous_endpoint(&config.relay).await {
        Ok(rdz) => rdz,
        Err(error) => {
            bound.local_endpoint.close().await;
            return Err(error);
        }
    };
    let pairing = match pair_or_cancel(
        &rdz,
        &broker,
        room_id,
        password,
        &my_addr,
        JoinIntent::Receive,
        cancel,
        events,
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
    Ok(auth)
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
    let pairing = pair_room_sender(broker, code, &config, events.as_ref(), &cancel).await?;
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

/// Send one Manifest transfer set through the existing room rendezvous flow.
pub async fn send_manifest_via_room(
    broker: EndpointAddr,
    code: &str,
    request: ManifestSendRequest,
    resume: bool,
    config: SessionConfig,
    events: Box<dyn SessionEventSink>,
    cancel: TransferCancelToken,
) -> Result<ManifestTransferSummary, SessionError> {
    let pairing = pair_room_sender(broker, code, &config, events.as_ref(), &cancel).await?;
    let auth = PairingConfig::Spake2SharedToken {
        token: pairing.token,
    };
    send_manifest_to_endpoint_addr(pairing.peer, request, resume, config, &auth, events, cancel)
        .await
}

async fn pair_room_sender(
    broker: EndpointAddr,
    code: &str,
    config: &SessionConfig,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
) -> Result<RoomPairing<EndpointAddr>, SessionError> {
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
        JoinIntent::Send,
        cancel,
        events,
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
    Ok(pairing)
}

#[cfg(test)]
#[path = "room_tests.rs"]
mod tests;
