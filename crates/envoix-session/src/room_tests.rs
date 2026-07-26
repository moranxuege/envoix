use envoix_error::CoreError;

use iroh::{EndpointAddr, SecretKey, TransportAddr};
use iroh_base::CustomAddr;

use super::{should_retry_room_with_relay, wifi_aware_first_peer};
use crate::datagram_transport::WIFI_AWARE_TRANSPORT_ID;
use crate::{
    CandidateFilter, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig, SenderTransferPhaseV2,
    SessionConfig,
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
