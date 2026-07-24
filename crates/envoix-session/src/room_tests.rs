use envoix_error::CoreError;

use super::should_retry_room_with_relay;
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
