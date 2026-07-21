use std::io::ErrorKind;
use std::net::UdpSocket;
use std::sync::Arc;

use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use iroh::RelayMode;

use super::*;
use crate::NoopEventSink;

async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..100 {
        if ep.addr().ip_addrs().next().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    endpoint_addr(ep)
}

fn loopback_transport_available() -> bool {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            println!("skipping room test: UDP bind permission denied ({error})");
            false
        }
        Err(error) => panic!("transport pre-check failed: {error}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_room_pairing_timeout_returns_before_room_expiry() {
    if !loopback_transport_available() {
        return;
    }

    let server = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .unwrap();
    let broker = ready_addr(&server).await;
    tokio::spawn(serve_endpoint(
        server,
        Arc::new(RoomRegistry::with_ttl(Duration::from_secs(30))),
        None,
    ));

    let rdz = rendezvous_endpoint(&None).await.unwrap();
    let placeholder = rdz.addr();
    let started = tokio::time::Instant::now();
    let result = pair_or_cancel(
        &rdz,
        &broker,
        "9999",
        "lonely-room",
        &placeholder,
        JoinIntent::Send,
        &TransferCancelToken::new(),
        &NoopEventSink,
        Some(Duration::from_millis(150)),
    )
    .await;
    let error = match result {
        Ok(_) => panic!("pairing should time out without a peer"),
        Err(error) => error,
    };

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout should beat the broker room expiry"
    );
    assert!(
        error.to_string().contains("rendezvous pairing timed out"),
        "expected pairing timeout, got: {error}"
    );
}
