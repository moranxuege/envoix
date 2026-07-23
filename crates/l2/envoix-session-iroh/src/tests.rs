use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use envoix_protocol::{CompleteAck, Frame, Hello, IngressState, Ready, decode_frame, encode_frame};
use envoix_types::TransferId;

use crate::{
    AuthFailureBudget, BindAddresses, CloseOrdering, CongestionControl, FlowWindow, IrohListener,
    SessionCancellation, SessionEndpointConfig, SessionLink, SessionTimeouts,
    SessionTransportConfig, dial,
};

fn timeouts() -> SessionTimeouts {
    SessionTimeouts::new(
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn loopback_config() -> SessionEndpointConfig {
    SessionEndpointConfig {
        bind: BindAddresses::ipv4_only(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        relay: None,
        transport: SessionTransportConfig {
            flow_window: FlowWindow::default(),
            congestion: CongestionControl::Bbr3,
        },
    }
}

#[tokio::test]
async fn session_iroh_loopback_link() {
    let cancellation = SessionCancellation::new();
    let listener = IrohListener::bind(loopback_config(), &cancellation, timeouts())
        .await
        .unwrap();
    let target = listener.addr();
    let server_cancel = cancellation.clone();
    let server = tokio::spawn(async move {
        let candidate = listener
            .accept_candidate(&server_cancel, timeouts())
            .await
            .unwrap();
        listener.promote(candidate)
    });
    let mut client = dial(loopback_config(), target, &cancellation, timeouts())
        .await
        .unwrap();
    let mut client_paths = client.take_path_observations();

    // Opening a QUIC stream is not visible to the peer until its first write.
    let hello = encode_frame(&Frame::Hello(Hello)).unwrap();
    client.send_packet(&hello).await.unwrap();
    let mut server = server.await.unwrap();
    let received = server.receive_packet().await.unwrap();
    assert_eq!(
        decode_frame(&received, IngressState::AwaitHello).unwrap(),
        Frame::Hello(Hello)
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), client_paths.recv())
            .await
            .unwrap(),
        Some(crate::PathObservation::Direct { .. })
    ));

    let ready = encode_frame(&Frame::Ready(Ready)).unwrap();
    server.send_packet(&ready).await.unwrap();
    let received = client.receive_packet().await.unwrap();
    assert_eq!(
        decode_frame(&received, IngressState::AwaitReady).unwrap(),
        Frame::Ready(Ready)
    );

    let label = b"envoix/session-test/export/v2";
    let context = b"loopback";
    let client_export = client
        .export_keying_material(label, context)
        .unwrap()
        .into_bytes();
    let server_export = server
        .export_keying_material(label, context)
        .unwrap()
        .into_bytes();
    assert_eq!(client_export, server_export);

    let ack = encode_frame(&Frame::CompleteAck(CompleteAck {
        transfer_id: TransferId::from_bytes([7; 16]),
    }))
    .unwrap();
    server.send_packet(&ack).await.unwrap();
    let received = client.receive_packet().await.unwrap();
    assert_eq!(
        decode_frame(&received, IngressState::AwaitCompleteAck).unwrap(),
        Frame::CompleteAck(CompleteAck {
            transfer_id: TransferId::from_bytes([7; 16]),
        })
    );

    let (server_close, client_close) = tokio::join!(
        server.close(CloseOrdering::AwaitPeer, timeouts()),
        client.close(CloseOrdering::Active, timeouts())
    );
    server_close.unwrap();
    client_close.unwrap();
}

#[test]
fn flow_window_and_auth_failure_bounds_are_rejected_not_clamped() {
    assert!(FlowWindow::new(1024 * 1024 - 1).is_err());
    assert!(FlowWindow::new(128 * 1024 * 1024 + 1).is_err());
    assert_eq!(
        FlowWindow::new(16 * 1024 * 1024).unwrap().bytes(),
        16 * 1024 * 1024
    );

    assert!(AuthFailureBudget::new(0).is_err());
    let mut budget = AuthFailureBudget::new(2).unwrap();
    assert!(budget.record_failure());
    assert!(!budget.record_failure());
    assert_eq!(budget.failures(), 2);
}

#[tokio::test]
async fn accept_wait_answers_cancellation() {
    let cancellation = SessionCancellation::new();
    let listener = IrohListener::bind(loopback_config(), &cancellation, timeouts())
        .await
        .unwrap();
    cancellation.cancel();
    let error = listener
        .accept_candidate(&cancellation, timeouts())
        .await
        .unwrap_err();
    assert_eq!(error, crate::SessionError::Cancelled);
    listener.close(timeouts()).await.unwrap();
}
