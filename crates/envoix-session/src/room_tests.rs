use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use envoix_error::CoreError;
use envoix_invite::{Capabilities, InviteV2, TransferRole};
use envoix_protocol::manifest_v2::CompressionPolicyV2;
use envoix_rendezvous::{BrokerOutcome, BrokerRejection, RendezvousError};
use envoix_transfer::{
    CanonicalTransferJob, EventSink, TransferCancelToken, TransferEvent, TransferStage,
};
use envoix_types::TransferDirection;
use iroh::{EndpointAddr, SecretKey, TransportAddr};
use iroh_base::CustomAddr;
use tempfile::tempdir;
use tokio::fs;

use super::{
    TrackedAuthentication, broker_retry_delay, classify_hybrid_failure, invitation_consumed,
    receive_manifest_v2_offer_via_room_hybrid, send_manifest_v2_via_room_hybrid,
    should_retry_room_with_relay, unstructured_broker_error, wifi_aware_first_peer,
};
use crate::datagram_transport::{PlatformDatagramTransport, WIFI_AWARE_TRANSPORT_ID};
use crate::{
    AuthenticationHandler, AuthenticationOutcome, CandidateFilter, DEFAULT_DATA_STREAM_WINDOW,
    IdentityConfig, SenderTransferPhaseV2, SessionConfig, SessionError,
};

struct UnusedDatagramTransport;

#[async_trait]
impl PlatformDatagramTransport for UnusedDatagramTransport {
    async fn send_datagram(&self, _bytes: Vec<u8>) -> Result<(), SessionError> {
        Err(CoreError::Transport(
            "test transport must not be acquired".into(),
        ))
    }

    async fn receive_datagram(&self, _max_bytes: u32) -> Result<Vec<u8>, SessionError> {
        Err(CoreError::Transport(
            "test transport must not be acquired".into(),
        ))
    }

    async fn close(&self) -> Result<(), SessionError> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingEvents {
    events: Mutex<Vec<TransferEvent>>,
}

impl EventSink for RecordingEvents {
    fn on_event(&self, event: TransferEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl RecordingEvents {
    fn stages(&self, expected_direction: TransferDirection) -> Vec<TransferStage> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                TransferEvent::StageTiming {
                    direction, stage, ..
                } => {
                    assert_eq!(*direction, expected_direction);
                    Some(*stage)
                }
                _ => None,
            })
            .collect()
    }
}

fn test_config() -> SessionConfig {
    SessionConfig {
        identity: IdentityConfig::Ephemeral,
        relay: None,
        relay_only: false,
        direct_only: false,
        candidates: CandidateFilter::default(),
        data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
        rendezvous_retry: crate::RendezvousRetryPolicy::default(),
    }
}

fn test_bootstrap(role: TransferRole) -> envoix_invite::InvitationBootstrap {
    InviteV2::create(
        format!("{}@127.0.0.1:1", SecretKey::generate().public()),
        Vec::new(),
        role,
        Capabilities::current(),
        1,
    )
    .unwrap()
    .into_bootstrap()
}

#[tokio::test]
async fn hybrid_sender_attempt_starts_before_platform_acquire_failure() {
    let temporary = tempdir().unwrap();
    let source = temporary.path().join("hybrid-attempt.bin");
    fs::write(&source, b"hybrid attempt").await.unwrap();
    let mut job = CanonicalTransferJob::new(CompressionPolicyV2::Never).unwrap();
    job.add_local_path(source).await.unwrap();
    job.prepare_all().await.unwrap();
    job.seal_for_send().unwrap();
    let events = Arc::new(RecordingEvents::default());

    let result = send_manifest_v2_via_room_hybrid(
        Arc::new(UnusedDatagramTransport),
        1,
        EndpointAddr::new(SecretKey::generate().public()),
        test_bootstrap(TransferRole::Sender),
        &job,
        temporary.path().join("sender-state"),
        test_config(),
        events.clone(),
        &TransferCancelToken::new(),
    )
    .await;

    assert!(matches!(result, Err(CoreError::InvalidInput(_))));
    assert_eq!(
        events.stages(TransferDirection::Send),
        vec![TransferStage::SessionStarted, TransferStage::Failed]
    );
}

#[tokio::test]
async fn hybrid_receiver_attempt_starts_before_platform_acquire_failure() {
    let events = Arc::new(RecordingEvents::default());

    let result = receive_manifest_v2_offer_via_room_hybrid(
        Arc::new(UnusedDatagramTransport),
        1,
        EndpointAddr::new(SecretKey::generate().public()),
        test_bootstrap(TransferRole::Receiver),
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        test_config(),
        events.clone(),
        &TransferCancelToken::new(),
    )
    .await;

    assert!(matches!(result, Err(CoreError::InvalidInput(_))));
    assert_eq!(
        events.stages(TransferDirection::Receive),
        vec![TransferStage::SessionStarted, TransferStage::Failed]
    );
}

#[test]
fn auto_room_retries_only_pre_offer_route_failures_through_relay() {
    let mut config = SessionConfig {
        identity: IdentityConfig::Ephemeral,
        relay: Some("https://relay.example.test".into()),
        relay_only: false,
        direct_only: false,
        candidates: CandidateFilter::default(),
        data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
        rendezvous_retry: crate::RendezvousRetryPolicy::default(),
    };

    assert!(should_retry_room_with_relay(
        &config,
        &CoreError::Protocol("authentication timed out".into()),
        Some(SenderTransferPhaseV2::Offering),
    ));
    assert!(should_retry_room_with_relay(
        &config,
        &CoreError::Transport("timed out".into()),
        Some(SenderTransferPhaseV2::Offering),
    ));
    assert!(!should_retry_room_with_relay(
        &config,
        &CoreError::Protocol("delivery proof mismatch".into()),
        Some(SenderTransferPhaseV2::Offering),
    ));
    assert!(!should_retry_room_with_relay(
        &config,
        &CoreError::Transport("timed out".into()),
        Some(SenderTransferPhaseV2::Transferring),
    ));

    config.relay_only = true;
    assert!(!should_retry_room_with_relay(
        &config,
        &CoreError::Protocol("authentication timed out".into()),
        Some(SenderTransferPhaseV2::Offering),
    ));
}

#[test]
fn nearby_hybrid_starts_on_the_authenticated_wifi_aware_peer() {
    let peer_id = SecretKey::generate().public();
    let custom = TransportAddr::Custom(CustomAddr::from_parts(
        WIFI_AWARE_TRANSPORT_ID,
        peer_id.as_bytes(),
    ));
    let ip = TransportAddr::Ip("127.0.0.1:4242".parse().unwrap());
    let room_peer = EndpointAddr::from_parts(peer_id, [ip.clone()]);
    let wifi_aware_peer = EndpointAddr::from_parts(peer_id, [custom.clone()]);

    let initial = wifi_aware_first_peer(&room_peer, &wifi_aware_peer).unwrap();

    assert_eq!(initial.addrs.len(), 1);
    assert!(!initial.addrs.contains(&ip));
    assert!(initial.addrs.contains(&custom));
}

#[test]
fn nearby_hybrid_rejects_wifi_aware_identity_mismatch() {
    let room_peer = EndpointAddr::new(SecretKey::generate().public());
    let wifi_aware_peer = EndpointAddr::new(SecretKey::generate().public());

    let error = wifi_aware_first_peer(&room_peer, &wifi_aware_peer).unwrap_err();

    assert!(matches!(error, CoreError::Crypto(_)));
}

#[test]
fn nearby_hybrid_rejects_missing_wifi_aware_custom_address() {
    let peer_id = SecretKey::generate().public();
    let room_peer = EndpointAddr::new(peer_id);
    let wifi_aware_peer = EndpointAddr::new(peer_id);

    let error = wifi_aware_first_peer(&room_peer, &wifi_aware_peer).unwrap_err();

    assert!(matches!(error, CoreError::InvalidInput(_)));
}

#[test]
fn server_retry_guidance_obeys_both_bounds() {
    let policy = crate::RendezvousRetryPolicy {
        pairing_attempts: 4,
        server_retries: 2,
        max_retry_after: std::time::Duration::from_secs(5),
    };
    let rejection = BrokerRejection {
        outcome: BrokerOutcome::RoomRateLimited,
        retry_after: Some(5),
    };
    assert_eq!(
        broker_retry_delay(&rejection, 0, policy),
        Some(std::time::Duration::from_secs(5))
    );
    assert_eq!(broker_retry_delay(&rejection, 2, policy), None);

    let too_long = BrokerRejection {
        retry_after: Some(6),
        ..rejection.clone()
    };
    assert_eq!(broker_retry_delay(&too_long, 0, policy), None);
    let terminal = BrokerRejection {
        outcome: BrokerOutcome::RoomUnderAttack,
        retry_after: Some(1),
    };
    assert_eq!(broker_retry_delay(&terminal, 0, policy), None);
}

#[test]
fn legacy_broker_reply_is_a_stable_unsupported_version_error() {
    let error = anyhow::Error::new(RendezvousError::BadMessage(
        "missing field `selected_bootstrap_method` at line 1 column 30".into(),
    ));
    assert!(matches!(
        unstructured_broker_error(&error),
        CoreError::Rendezvous {
            cause: crate::RendezvousCause::UnsupportedVersion,
            retry_after: None,
        }
    ));

    let malformed = anyhow::Error::new(RendezvousError::BadMessage("not JSON".into()));
    assert!(matches!(
        unstructured_broker_error(&malformed),
        CoreError::Protocol(message)
            if message == "rendezvous broker returned a malformed control message"
    ));
}

#[test]
fn post_authentication_failure_requires_a_new_invitation() {
    let error = invitation_consumed(CoreError::Transport("connection lost".into()));
    assert!(matches!(
        error,
        CoreError::InvitationConsumed(source)
            if matches!(source.as_ref(), CoreError::Transport(detail) if detail == "connection lost")
    ));

    struct FailingHandler;
    impl AuthenticationHandler for FailingHandler {
        fn on_authenticated(&self, _outcome: AuthenticationOutcome) -> Result<(), SessionError> {
            Err(CoreError::Storage("credential persistence failed".into()))
        }
    }

    let authentication = TrackedAuthentication::new(&FailingHandler);
    assert!(
        authentication
            .on_authenticated(AuthenticationOutcome {
                remember_secret: None,
            })
            .is_err()
    );
    assert!(authentication.authenticated());
}

#[test]
fn hybrid_transport_failure_before_connected_gets_stable_fallback_cause() {
    let error = classify_hybrid_failure(
        CoreError::Transport("custom QUIC dial failed".into()),
        false,
        false,
    );

    assert!(matches!(
        error,
        CoreError::Cause {
            cause: envoix_error::TransferCause::NearbyHybridPreAuthTransportFailure,
            detail,
        } if detail == "custom QUIC dial failed"
    ));
}

#[test]
fn hybrid_failure_after_connected_is_not_marked_for_fallback() {
    let error = classify_hybrid_failure(
        CoreError::Transport("authentication stream closed".into()),
        false,
        true,
    );

    assert!(matches!(
        error,
        CoreError::Transport(detail) if detail == "authentication stream closed"
    ));
}

#[test]
fn hybrid_pre_connected_non_transport_failure_is_not_marked_for_fallback() {
    let error = classify_hybrid_failure(
        CoreError::Crypto("peer proof mismatch".into()),
        false,
        false,
    );

    assert!(matches!(
        error,
        CoreError::Crypto(detail) if detail == "peer proof mismatch"
    ));
}

#[test]
fn hybrid_post_authentication_transport_failure_consumes_invitation_without_fallback_marker() {
    let error = classify_hybrid_failure(
        CoreError::Transport("authenticated connection lost".into()),
        true,
        false,
    );

    assert!(matches!(
        error,
        CoreError::InvitationConsumed(source)
            if matches!(source.as_ref(), CoreError::Transport(detail)
                if detail == "authenticated connection lost")
    ));
}
