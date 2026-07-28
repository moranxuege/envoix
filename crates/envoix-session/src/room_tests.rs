use envoix_error::CoreError;
use envoix_rendezvous::{BrokerOutcome, BrokerRejection, RendezvousError};
use iroh::{EndpointAddr, SecretKey, TransportAddr};
use iroh_base::CustomAddr;

use super::{
    TrackedAuthentication, broker_retry_delay, invitation_consumed, should_retry_room_with_relay,
    unstructured_broker_error, wifi_aware_first_peer,
};
use crate::datagram_transport::WIFI_AWARE_TRANSPORT_ID;
use crate::{
    AuthenticationHandler, AuthenticationOutcome, CandidateFilter, DEFAULT_DATA_STREAM_WINDOW,
    IdentityConfig, SenderTransferPhaseV2, SessionConfig, SessionError,
};

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
