use super::*;

#[tokio::test(start_paused = true)]
async fn wedged_auth_times_out() {
    let cancel = TransferCancelToken::new();
    let result = auth_bounded(std::future::pending(), &cancel).await;
    assert!(matches!(result, Err(CoreError::Protocol(m)) if m.contains("timed out")));
}

#[tokio::test]
async fn cancel_interrupts_a_pending_auth() {
    let cancel = TransferCancelToken::new();
    cancel.pause();
    let result = auth_bounded(std::future::pending(), &cancel).await;
    assert!(matches!(result, Err(CoreError::Transfer(m)) if m == USER_PAUSE_MESSAGE));
}

#[tokio::test]
async fn a_finishing_auth_passes_through() {
    let cancel = TransferCancelToken::new();
    assert!(auth_bounded(async { Ok(()) }, &cancel).await.is_ok());
}
