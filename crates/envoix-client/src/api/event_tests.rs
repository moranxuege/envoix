use super::SessionFailureCode;

#[test]
fn classify_maps_canonical_messages_to_codes() {
    // The session layer prefixes/wraps these, so classify uses contains.
    assert_eq!(
        SessionFailureCode::classify("transfer paused by user"),
        SessionFailureCode::Paused
    );
    assert_eq!(
        SessionFailureCode::classify("transfer interrupted by user"),
        SessionFailureCode::Cancelled
    );
    assert_eq!(
        SessionFailureCode::classify("transfer paused by peer"),
        SessionFailureCode::PeerPaused
    );
    assert_eq!(
        SessionFailureCode::classify("transfer interrupted by peer"),
        SessionFailureCode::PeerCancelled
    );
    assert_eq!(
        SessionFailureCode::classify("io error: connection lost"),
        SessionFailureCode::ConnectionLost
    );
    assert_eq!(
        SessionFailureCode::classify("connection closed by peer"),
        SessionFailureCode::ConnectionLost
    );
    assert_eq!(
        SessionFailureCode::classify("hash mismatch"),
        SessionFailureCode::Other
    );
}

#[test]
fn reason_code_serializes_snake_case() {
    assert_eq!(
        serde_json::to_string(&SessionFailureCode::PeerPaused).unwrap(),
        r#""peer_paused""#
    );
}
