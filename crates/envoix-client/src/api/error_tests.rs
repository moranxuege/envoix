use super::*;

#[test]
fn classifies_kind_and_keeps_phase() {
    let error = TransferError::from_core(
        CoreError::Transport("connection lost".into()),
        Phase::Pairing,
    );
    assert_eq!(error.kind, ErrorKind::Transport);
    assert_eq!(error.phase, Phase::Pairing);
    assert_eq!(
        error.to_string(),
        "transport error during pairing: connection lost"
    );
}

#[test]
fn maps_user_interrupt_to_cancelled() {
    let error = TransferError::from_core(
        CoreError::Transfer(USER_INTERRUPT_MESSAGE.into()),
        Phase::Transfer,
    );
    assert_eq!(error.kind, ErrorKind::Cancelled);
}

#[test]
fn exposes_retryable_timeout_failure() {
    let error = TransferError::from_core(
        CoreError::Transfer(
            "receiver did not confirm completion within 60 seconds; retry may resume the transfer"
                .into(),
        ),
        Phase::Transfer,
    );
    let failure = error.to_failure(Some(TransferDirection::Send));
    assert_eq!(failure.code, FailureCode::Timeout);
    assert_eq!(failure.category, FailureCategory::Network);
    assert_eq!(failure.phase, FailurePhase::Acknowledging);
    assert_eq!(failure.direction, Some(TransferDirection::Send));
    assert!(failure.retryable);
    assert_eq!(failure.recovery_action, RecoveryAction::Retry);
}

#[test]
fn exposes_peer_cancellation_failure() {
    let error = TransferError::from_core(
        CoreError::Transfer("transfer interrupted by peer".into()),
        Phase::Transfer,
    );
    let failure = error.to_failure(Some(TransferDirection::Receive));
    assert_eq!(failure.code, FailureCode::PeerCanceled);
    assert_eq!(failure.category, FailureCategory::User);
    assert_eq!(failure.origin, FailureOrigin::Peer);
    assert!(!failure.retryable);
}

#[test]
fn exposes_permission_recovery_action() {
    let error = TransferError::from_core(
        CoreError::Storage("permission denied opening destination folder".into()),
        Phase::Transfer,
    );
    let failure = error.to_failure(Some(TransferDirection::Receive));
    assert_eq!(failure.code, FailureCode::PermissionDenied);
    assert_eq!(failure.category, FailureCategory::Permission);
    assert_eq!(failure.origin, FailureOrigin::Local);
    assert!(failure.retryable);
    assert_eq!(failure.recovery_action, RecoveryAction::ChooseFolder);
}
