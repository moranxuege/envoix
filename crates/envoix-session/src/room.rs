//! Directional InviteV2 rendezvous and authenticated descriptor exchange.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envoix_error::CoreError;
use envoix_invite::{
    BootstrapKind, InvitationBootstrap, InvitationError, InvitationSide, TransferRole,
};
use envoix_protocol::manifest_v2_frames::JobGenerationV2;
use envoix_rendezvous::{
    BrokerOutcome, BrokerRejection, Join, RENDEZVOUS_PROTOCOL_VERSION, RendezvousError,
};
use envoix_rendezvous_iroh::{
    AuthenticatedControl, BrokerSession, RoomPairing, authenticate_invitation,
    build_endpoint_with_dns, join_invitation,
};
use envoix_transfer::{SenderDeliveryStoreV2, SenderTransferPhaseV2};
use envoix_types::PairingStep;
use iroh::{Endpoint, EndpointAddr, SecretKey};

use crate::{
    AuthenticationHandler, AuthenticationOutcome, BindAddrs, BoundEndpoint, CanonicalTransferJob,
    EventSink, NoopAuthenticationHandler, PairingConfig, PendingManifestV2Receive,
    RememberedSession, RendezvousCause, RendezvousRetryPolicy, SenderManifestV2SessionSummary,
    SessionConfig, SessionError, TransferCancelToken, TransferEvent,
    bind_iroh_manifest_v2_endpoint, receive_manifest_v2_offer_with_authentication,
    send_manifest_v2_to_endpoint_addr_with_authentication,
};

const SEND_ROOM_PAIRING_TIMEOUT: Duration = Duration::from_secs(35);

struct TrackedAuthentication<'a> {
    inner: &'a dyn AuthenticationHandler,
    authenticated: AtomicBool,
}

impl<'a> TrackedAuthentication<'a> {
    fn new(inner: &'a dyn AuthenticationHandler) -> Self {
        Self {
            inner,
            authenticated: AtomicBool::new(false),
        }
    }

    fn authenticated(&self) -> bool {
        self.authenticated.load(Ordering::Acquire)
    }
}

impl AuthenticationHandler for TrackedAuthentication<'_> {
    fn remember_consent(&self) -> bool {
        self.inner.remember_consent()
    }

    fn on_authenticated(&self, outcome: AuthenticationOutcome) -> Result<(), SessionError> {
        self.authenticated.store(true, Ordering::Release);
        self.inner.on_authenticated(outcome)
    }
}

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

pub(crate) async fn join_broker_with_retry(
    endpoint: &Endpoint,
    broker: &EndpointAddr,
    join: Join,
    policy: RendezvousRetryPolicy,
) -> Result<BrokerSession, SessionError> {
    for retry in 0..=policy.server_retries {
        match join_invitation(endpoint, broker.clone(), join.clone()).await {
            Ok(session) => return Ok(session),
            Err(error) => {
                let Some(rejection) = error.downcast_ref::<BrokerRejection>() else {
                    return Err(unstructured_broker_error(&error));
                };
                let core_error = broker_rejection(rejection);
                let Some(retry_after) = broker_retry_delay(rejection, retry, policy) else {
                    return Err(core_error);
                };
                tokio::time::sleep(retry_after).await;
            }
        }
    }
    unreachable!("server retry loop always returns")
}

fn unstructured_broker_error(error: &anyhow::Error) -> CoreError {
    match error.downcast_ref::<RendezvousError>() {
        // V1 Paired replies do not carry the bootstrap method required by the
        // strict directional V2 protocol. Report that deployment mismatch as
        // a stable, terminal cause without weakening the client with fallback.
        Some(RendezvousError::BadMessage(message))
            if message.contains("selected_bootstrap_method") =>
        {
            CoreError::Rendezvous {
                cause: RendezvousCause::UnsupportedVersion,
                retry_after: None,
            }
        }
        Some(RendezvousError::BadMessage(_)) => {
            CoreError::Protocol("rendezvous broker returned a malformed control message".into())
        }
        _ => CoreError::Transport(error.to_string()),
    }
}

fn retryable_broker_outcome(outcome: BrokerOutcome) -> bool {
    matches!(
        outcome,
        BrokerOutcome::RoomNotFound
            | BrokerOutcome::RoomFull
            | BrokerOutcome::RoomRateLimited
            | BrokerOutcome::EndpointRateLimited
            | BrokerOutcome::IpRateLimited
            | BrokerOutcome::ServerBusy
    )
}

fn broker_retry_delay(
    rejection: &BrokerRejection,
    retries_completed: usize,
    policy: RendezvousRetryPolicy,
) -> Option<Duration> {
    let delay = Duration::from_secs(rejection.retry_after?);
    (retries_completed < policy.server_retries
        && retryable_broker_outcome(rejection.outcome)
        && delay <= policy.max_retry_after)
        .then_some(delay)
}

fn broker_rejection(rejection: &BrokerRejection) -> CoreError {
    let cause = match rejection.outcome {
        BrokerOutcome::RoomNotFound => RendezvousCause::RoomNotFound,
        BrokerOutcome::RoomExpired => RendezvousCause::RoomExpired,
        BrokerOutcome::RoomFull => RendezvousCause::RoomFull,
        BrokerOutcome::RoomRateLimited => RendezvousCause::RoomRateLimited,
        BrokerOutcome::RoomUnderAttack => RendezvousCause::RoomUnderAttack,
        BrokerOutcome::EndpointRateLimited => RendezvousCause::EndpointRateLimited,
        BrokerOutcome::IpRateLimited => RendezvousCause::IpRateLimited,
        BrokerOutcome::ServerBusy => RendezvousCause::ServerBusy,
        BrokerOutcome::MalformedJoin => RendezvousCause::MalformedJoin,
        BrokerOutcome::UnsupportedVersion => RendezvousCause::UnsupportedVersion,
    };
    CoreError::Rendezvous {
        cause,
        retry_after: rejection.retry_after,
    }
}

async fn authenticate_in_room_retrying(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    bootstrap: &InvitationBootstrap,
    public_context: Option<&[u8]>,
    events: &dyn EventSink,
    retry_policy: RendezvousRetryPolicy,
) -> Result<AuthenticatedControl, SessionError> {
    const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(8);
    let mut last: Option<SessionError> = None;
    for _ in 0..retry_policy.pairing_attempts.max(1) {
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Joining,
        });
        let session = join_broker_with_retry(
            rdz,
            broker,
            Join {
                version: RENDEZVOUS_PROTOCOL_VERSION,
                room_id: bootstrap.room_id().to_string(),
                invitation_side: bootstrap.side(),
                transfer_role: bootstrap.local_role(),
                bootstrap_methods: bootstrap.advertised_methods(),
                selected_bootstrap_method: bootstrap.selected_method(),
            },
            retry_policy,
        )
        .await?;
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Matched,
        });
        let selected = session.selected_bootstrap_method;
        let context = bootstrap
            .control_context(selected)
            .map_err(invitation_error)?;
        let password = bootstrap
            .control_pake_password(selected)
            .map_err(invitation_error)?;
        match tokio::time::timeout(
            EXCHANGE_TIMEOUT,
            authenticate_invitation(session, password.expose(), &context, public_context),
        )
        .await
        {
            Ok(Ok(pairing)) => {
                return Ok(pairing);
            }
            Ok(Err(error)) => last = Some(CoreError::Transport(error.to_string())),
            Err(_) => last = Some(CoreError::Transport("rendezvous pairing stalled".into())),
        }
    }
    Err(last.expect("at least one pairing attempt failed"))
}

async fn authenticate_remembered_in_room_retrying(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    remembered: &RememberedSession,
    local_role: TransferRole,
    events: &dyn EventSink,
    retry_policy: RendezvousRetryPolicy,
) -> Result<AuthenticatedControl, SessionError> {
    const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(8);
    let (invitation_side, bootstrap_methods, selected_bootstrap_method) = match local_role {
        TransferRole::Sender => (
            InvitationSide::Joiner,
            Vec::new(),
            Some(BootstrapKind::FullTicket),
        ),
        TransferRole::Receiver => (
            InvitationSide::Creator,
            vec![BootstrapKind::FullTicket],
            None,
        ),
    };
    let context = remembered.control_context()?;
    let password = remembered.control_password();
    let mut last = None;
    for _ in 0..retry_policy.pairing_attempts.max(1) {
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Joining,
        });
        let session = join_broker_with_retry(
            rdz,
            broker,
            Join {
                version: RENDEZVOUS_PROTOCOL_VERSION,
                room_id: remembered.room_id().to_string(),
                invitation_side,
                transfer_role: local_role,
                bootstrap_methods: bootstrap_methods.clone(),
                selected_bootstrap_method,
            },
            retry_policy,
        )
        .await?;
        events.on_event(TransferEvent::Pairing {
            step: PairingStep::Matched,
        });
        match tokio::time::timeout(
            EXCHANGE_TIMEOUT,
            authenticate_invitation(session, &password, &context, None),
        )
        .await
        {
            Ok(Ok(pairing)) => return Ok(pairing),
            Ok(Err(error)) => last = Some(CoreError::Transport(error.to_string())),
            Err(_) => last = Some(CoreError::Transport("rendezvous pairing stalled".into())),
        }
    }
    Err(last.expect("at least one pairing attempt failed"))
}

#[allow(clippy::too_many_arguments)]
async fn pair_or_cancel(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    bootstrap: &InvitationBootstrap,
    public_context: Option<&[u8]>,
    cancel: &TransferCancelToken,
    events: &dyn EventSink,
    retry_policy: RendezvousRetryPolicy,
    timeout: Option<Duration>,
) -> Result<AuthenticatedControl, SessionError> {
    let pairing =
        authenticate_in_room_retrying(rdz, broker, bootstrap, public_context, events, retry_policy);
    let result = if let Some(timeout) = timeout {
        tokio::select! {
            result = pairing => result,
            _ = cancel.cancelled() => Err(CoreError::Cancelled),
            _ = tokio::time::sleep(timeout) => {
                Err(CoreError::Transport("rendezvous pairing timed out".into()))
            }
        }
    } else {
        tokio::select! {
            result = pairing => result,
            _ = cancel.cancelled() => Err(CoreError::Cancelled),
        }
    };
    if result.is_err() {
        rdz.close().await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn pair_remembered_or_cancel(
    rdz: &Endpoint,
    broker: &EndpointAddr,
    remembered: &RememberedSession,
    local_role: TransferRole,
    cancel: &TransferCancelToken,
    events: &dyn EventSink,
    retry_policy: RendezvousRetryPolicy,
    timeout: Option<Duration>,
) -> Result<AuthenticatedControl, SessionError> {
    let pairing = authenticate_remembered_in_room_retrying(
        rdz,
        broker,
        remembered,
        local_role,
        events,
        retry_policy,
    );
    let result = if let Some(timeout) = timeout {
        tokio::select! {
            result = pairing => result,
            _ = cancel.cancelled() => Err(CoreError::Cancelled),
            _ = tokio::time::sleep(timeout) => {
                Err(CoreError::Transport("remembered rendezvous pairing timed out".into()))
            }
        }
    } else {
        tokio::select! {
            result = pairing => result,
            _ = cancel.cancelled() => Err(CoreError::Cancelled),
        }
    };
    if result.is_err() {
        rdz.close().await;
    }
    result
}

/// Receives an authenticated Manifest V2 offer. Invitation authentication and
/// context validation complete before the data endpoint is created.
pub async fn receive_manifest_v2_offer_via_room(
    broker: EndpointAddr,
    bootstrap: InvitationBootstrap,
    listen_addrs: impl Into<BindAddrs>,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive, SessionError> {
    receive_manifest_v2_offer_via_room_with_authentication(
        broker,
        bootstrap,
        listen_addrs,
        config,
        events,
        cancel,
        &NoopAuthenticationHandler,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_manifest_v2_offer_via_room_with_authentication(
    broker: EndpointAddr,
    bootstrap: InvitationBootstrap,
    listen_addrs: impl Into<BindAddrs>,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    authentication: &dyn AuthenticationHandler,
) -> Result<PendingManifestV2Receive, SessionError> {
    require_bootstrap_role(&bootstrap, TransferRole::Receiver)?;
    let (rdz, control) =
        authenticate_room(broker, &bootstrap, &config, events.as_ref(), cancel, None).await?;
    validate_authenticated_context(&bootstrap, &control)?;
    let bound = match bind_iroh_manifest_v2_endpoint(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await
    {
        Ok(bound) => bound,
        Err(error) => {
            rdz.close().await;
            return Err(error);
        }
    };
    let auth = match complete_receiver_pairing(
        control,
        &bound,
        &bootstrap,
        &config,
        events.as_ref(),
        cancel,
    )
    .await
    {
        Ok(auth) => auth,
        Err(error) => {
            bound.local_endpoint.close().await;
            rdz.close().await;
            return Err(error);
        }
    };
    rdz.close().await;
    let authentication = TrackedAuthentication::new(authentication);
    let result = receive_manifest_v2_offer_with_authentication(
        bound,
        &auth,
        events,
        cancel,
        &authentication,
    )
    .await;
    if authentication.authenticated() {
        result.map_err(invitation_consumed)
    } else {
        result
    }
}

/// Receives through a high-entropy remembered-device rendezvous. The receiver
/// always advertises as the control responder.
#[allow(clippy::too_many_arguments)]
pub async fn receive_manifest_v2_offer_via_remembered(
    broker: EndpointAddr,
    broker_binding: String,
    remembered: RememberedSession,
    listen_addrs: impl Into<BindAddrs>,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    authentication: &dyn AuthenticationHandler,
) -> Result<PendingManifestV2Receive, SessionError> {
    let rdz = rendezvous_endpoint(&config.relay).await?;
    let control = pair_remembered_or_cancel(
        &rdz,
        &broker,
        &remembered,
        TransferRole::Receiver,
        cancel,
        events.as_ref(),
        config.rendezvous_retry,
        None,
    )
    .await?;
    let bound = match bind_iroh_manifest_v2_endpoint(
        listen_addrs,
        &config.identity,
        &config.data_relay(),
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await
    {
        Ok(bound) => bound,
        Err(error) => {
            rdz.close().await;
            return Err(error);
        }
    };
    let my_addr = bound
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;
    let pairing = match exchange_descriptor_or_cancel(control, Some(&my_addr), cancel).await {
        Ok(pairing) => pairing,
        Err(error) => {
            bound.local_endpoint.close().await;
            rdz.close().await;
            return Err(error);
        }
    };
    events.on_event(TransferEvent::Pairing {
        step: PairingStep::Exchanged,
    });
    events.on_event(TransferEvent::Connecting);
    let auth = remembered.finish_pairing(
        broker_binding,
        pairing.control_key(),
        pairing.control_transcript_hash,
    )?;
    rdz.close().await;
    receive_manifest_v2_offer_with_authentication(bound, &auth, events, cancel, authentication)
        .await
}

async fn complete_receiver_pairing(
    control: AuthenticatedControl,
    bound: &BoundEndpoint,
    bootstrap: &InvitationBootstrap,
    config: &SessionConfig,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
) -> Result<PairingConfig, SessionError> {
    let my_addr = bound
        .ready_endpoint_addr(config.data_relay().is_some())
        .await;
    let pairing = exchange_descriptor_or_cancel(control, Some(&my_addr), cancel).await?;
    events.on_event(TransferEvent::Pairing {
        step: PairingStep::Exchanged,
    });
    events.on_event(TransferEvent::Connecting);
    finish_invitation_pairing(bootstrap, &pairing)
}

/// Sends a sealed canonical Manifest V2 job through an authenticated
/// directional invitation.
pub async fn send_manifest_v2_via_room(
    broker: EndpointAddr,
    bootstrap: InvitationBootstrap,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    send_manifest_v2_via_room_with_authentication(
        broker,
        bootstrap,
        job,
        state_directory,
        config,
        events,
        cancel,
        &NoopAuthenticationHandler,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn send_manifest_v2_via_room_with_authentication(
    broker: EndpointAddr,
    bootstrap: InvitationBootstrap,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    authentication: &dyn AuthenticationHandler,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    require_bootstrap_role(&bootstrap, TransferRole::Sender)?;
    let pairing = pair_room_sender(broker, &bootstrap, &config, events.as_ref(), cancel).await?;
    let authentication = TrackedAuthentication::new(authentication);
    let first_attempt = send_manifest_v2_to_endpoint_addr_with_authentication(
        pairing.peer.clone(),
        job,
        state_directory.clone(),
        config.clone(),
        &pairing.auth,
        events.clone(),
        cancel,
        &authentication,
    )
    .await;
    let error = match first_attempt {
        Ok(summary) => return Ok(summary),
        Err(error) => error,
    };
    if authentication.authenticated() {
        return Err(invitation_consumed(error));
    }
    let sender_phase = sender_delivery_phase(job, &state_directory).await;
    if !should_retry_room_with_relay(&config, &error, sender_phase) {
        return Err(error);
    }
    events.on_event(TransferEvent::Diagnostic {
        message: "direct connection failed before the offer; retrying through the relay".into(),
    });
    let mut relay_config = config;
    relay_config.relay_only = true;
    let result = send_manifest_v2_to_endpoint_addr_with_authentication(
        pairing.peer,
        job,
        state_directory,
        relay_config,
        &pairing.auth,
        events,
        cancel,
        &authentication,
    )
    .await;
    if authentication.authenticated() {
        result.map_err(invitation_consumed)
    } else {
        result
    }
}

/// Sends through a high-entropy remembered-device rendezvous. The sender
/// always joins as the control initiator.
#[allow(clippy::too_many_arguments)]
pub async fn send_manifest_v2_via_remembered(
    broker: EndpointAddr,
    broker_binding: String,
    remembered: RememberedSession,
    job: &CanonicalTransferJob,
    state_directory: PathBuf,
    config: SessionConfig,
    events: Arc<dyn EventSink>,
    cancel: &TransferCancelToken,
    authentication: &dyn AuthenticationHandler,
) -> Result<SenderManifestV2SessionSummary, SessionError> {
    let rdz = rendezvous_endpoint(&config.relay).await?;
    let control = pair_remembered_or_cancel(
        &rdz,
        &broker,
        &remembered,
        TransferRole::Sender,
        cancel,
        events.as_ref(),
        config.rendezvous_retry,
        Some(SEND_ROOM_PAIRING_TIMEOUT),
    )
    .await?;
    let pairing = exchange_descriptor_or_cancel::<EndpointAddr>(control, None, cancel).await?;
    rdz.close().await;
    events.on_event(TransferEvent::Pairing {
        step: PairingStep::Exchanged,
    });
    let auth = remembered.finish_pairing(
        broker_binding,
        pairing.control_key(),
        pairing.control_transcript_hash,
    )?;
    let peer = pairing
        .peer
        .ok_or_else(|| CoreError::Protocol("receiver omitted endpoint descriptor".into()))?;
    send_manifest_v2_to_endpoint_addr_with_authentication(
        peer,
        job,
        state_directory,
        config,
        &auth,
        events,
        cancel,
        authentication,
    )
    .await
}

fn should_retry_room_with_relay(
    config: &SessionConfig,
    error: &SessionError,
    sender_phase: Option<SenderTransferPhaseV2>,
) -> bool {
    !config.relay_only
        && config.data_relay().is_some()
        && sender_phase == Some(SenderTransferPhaseV2::Offering)
        && (matches!(error, CoreError::Transport(_))
            || matches!(
                error,
                CoreError::Protocol(message) if message == "authentication timed out"
            ))
}

async fn sender_delivery_phase(
    job: &CanonicalTransferJob,
    state_directory: &std::path::Path,
) -> Option<SenderTransferPhaseV2> {
    let manifest = job.manifest()?;
    let identity = JobGenerationV2 {
        job_id: manifest.job_id,
        generation: manifest.generation,
    };
    SenderDeliveryStoreV2::new(state_directory.join("sender-delivery"))
        .load(identity)
        .await
        .ok()
        .flatten()
        .map(|record| record.phase())
}

struct AuthenticatedRoomPairing<T> {
    peer: T,
    auth: PairingConfig,
}

async fn pair_room_sender(
    broker: EndpointAddr,
    bootstrap: &InvitationBootstrap,
    config: &SessionConfig,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
) -> Result<AuthenticatedRoomPairing<EndpointAddr>, SessionError> {
    let (rdz, control) = authenticate_room(
        broker,
        bootstrap,
        config,
        events,
        cancel,
        Some(SEND_ROOM_PAIRING_TIMEOUT),
    )
    .await?;
    validate_authenticated_context(bootstrap, &control)?;
    let pairing = exchange_descriptor_or_cancel::<EndpointAddr>(control, None, cancel).await?;
    rdz.close().await;
    events.on_event(TransferEvent::Pairing {
        step: PairingStep::Exchanged,
    });
    let auth = finish_invitation_pairing(bootstrap, &pairing)?;
    Ok(AuthenticatedRoomPairing {
        peer: pairing
            .peer
            .ok_or_else(|| CoreError::Protocol("receiver omitted endpoint descriptor".into()))?,
        auth,
    })
}

async fn authenticate_room(
    broker: EndpointAddr,
    bootstrap: &InvitationBootstrap,
    config: &SessionConfig,
    events: &dyn EventSink,
    cancel: &TransferCancelToken,
    timeout: Option<Duration>,
) -> Result<(Endpoint, AuthenticatedControl), SessionError> {
    let rdz = rendezvous_endpoint(&config.relay).await?;
    let public_context = bootstrap
        .creator_public_context()
        .map_err(invitation_error)?;
    let control = match pair_or_cancel(
        &rdz,
        &broker,
        bootstrap,
        public_context.as_deref(),
        cancel,
        events,
        config.rendezvous_retry,
        timeout,
    )
    .await
    {
        Ok(control) => control,
        Err(error) => {
            rdz.close().await;
            return Err(error);
        }
    };
    Ok((rdz, control))
}

fn validate_authenticated_context(
    bootstrap: &InvitationBootstrap,
    control: &AuthenticatedControl,
) -> Result<(), SessionError> {
    bootstrap
        .validate_control_context(
            control.selected_bootstrap_method,
            control.peer_public_context.as_deref(),
            unix_now()?,
        )
        .map_err(invitation_error)
}

async fn exchange_descriptor_or_cancel<T>(
    control: AuthenticatedControl,
    mine: Option<&T>,
    cancel: &TransferCancelToken,
) -> Result<RoomPairing<Option<T>>, SessionError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    tokio::select! {
        result = control.exchange_descriptor(mine) => {
            result.map_err(|error| CoreError::Transport(error.to_string()))
        }
        _ = cancel.cancelled() => {
            Err(CoreError::Cancelled)
        }
        _ = tokio::time::sleep(Duration::from_secs(8)) => {
            Err(CoreError::Transport("rendezvous descriptor exchange stalled".into()))
        }
    }
}

fn finish_invitation_pairing<T>(
    bootstrap: &InvitationBootstrap,
    pairing: &RoomPairing<T>,
) -> Result<PairingConfig, SessionError> {
    let (password, context) = bootstrap
        .finish_control(
            pairing.selected_bootstrap_method,
            pairing.peer_public_context.as_deref(),
            pairing.control_key(),
            pairing.control_transcript_hash,
            unix_now()?,
        )
        .map_err(invitation_error)?;
    PairingConfig::invitation_v2(password, context)
}

fn require_bootstrap_role(
    bootstrap: &InvitationBootstrap,
    expected: TransferRole,
) -> Result<(), SessionError> {
    if bootstrap.local_role() == expected {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(
            "invitation transfer role conflicts with selected operation".into(),
        ))
    }
}

fn invitation_error(error: InvitationError) -> SessionError {
    CoreError::InvalidInput(format!("invitation {}: {error}", error.code().as_str()))
}

fn invitation_consumed(error: SessionError) -> SessionError {
    match error {
        SessionError::InvitationConsumed(_) => error,
        error => SessionError::InvitationConsumed(Box::new(error)),
    }
}

fn unix_now() -> Result<u64, SessionError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            CoreError::InvalidInput(format!("system clock before Unix epoch: {error}"))
        })
}

#[cfg(test)]
#[path = "room_tests.rs"]
mod tests;
