//! End-to-end over real iroh: two clients connect to a loopback rendezvous,
//! join the same room, and pair via `pair_in_room`, exchanging their real iroh
//! `PeerDescriptor`s through the broker's blind relay.

use std::io::ErrorKind;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use envoix_protocol::PeerDescriptor;
use envoix_rendezvous::{
    BootstrapKind, InvitationSide, Join, RENDEZVOUS_PROTOCOL_VERSION, RoomRegistry, TransferRole,
};
use envoix_rendezvous_iroh::{
    InvitationControlContext, build_endpoint, drive_pairing, endpoint_addr, join_invitation,
    serve_endpoint,
};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};

/// Loopback bind, fresh identity.
async fn endpoint() -> Endpoint {
    build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .expect("bind endpoint")
}

/// Wait until the endpoint has a direct address, then return its connectable addr.
async fn ready_addr(ep: &Endpoint) -> EndpointAddr {
    for _ in 0..100 {
        if ep.addr().ip_addrs().next().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    endpoint_addr(ep)
}

fn udp_transport_available() -> bool {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            println!("skipping iroh loopback test: UDP bind permission denied ({error})");
            false
        }
        Err(error) => panic!("transport pre-check failed: {error}"),
    }
}

/// This endpoint's app-level descriptor (id + direct addrs).
fn descriptor(ep: &Endpoint) -> PeerDescriptor {
    PeerDescriptor::new(ep.id().to_string(), ep.addr().ip_addrs().copied().collect())
        .expect("descriptor")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_iroh_peers_pair_through_the_rendezvous() {
    if !udp_transport_available() {
        return;
    }

    // Broker.
    let server = endpoint().await;
    let broker = ready_addr(&server).await;
    tokio::spawn(serve_endpoint(server, Arc::new(RoomRegistry::new()), None));

    // Two clients, each with a real (address-ready) descriptor to exchange.
    let ca = endpoint().await;
    let cb = endpoint().await;
    let _ = ready_addr(&ca).await;
    let _ = ready_addr(&cb).await;
    let desc_a = descriptor(&ca);
    let desc_b = descriptor(&cb);

    let (broker_a, broker_b) = (broker.clone(), broker.clone());
    let (mine_a, mine_b) = (desc_a.clone(), desc_b.clone());
    let joiner_context = InvitationControlContext::new(
        "700007".to_string(),
        BootstrapKind::RoomCode,
        TransferRole::Receiver,
        TransferRole::Sender,
    )
    .unwrap();
    let creator_context = joiner_context.clone();
    let a = tokio::spawn(async move {
        let session = join_invitation(
            &ca,
            broker_a,
            Join {
                version: RENDEZVOUS_PROTOCOL_VERSION,
                room_id: "700007".to_string(),
                invitation_side: InvitationSide::Joiner,
                transfer_role: TransferRole::Sender,
                bootstrap_methods: Vec::new(),
                selected_bootstrap_method: Some(BootstrapKind::RoomCode),
            },
        )
        .await?;
        drive_pairing(session, "700007-abcd-1234", &joiner_context, &mine_a, None).await
    });
    // Arrival order does not select the PAKE role.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let b = tokio::spawn(async move {
        let session = join_invitation(
            &cb,
            broker_b,
            Join {
                version: RENDEZVOUS_PROTOCOL_VERSION,
                room_id: "700007".to_string(),
                invitation_side: InvitationSide::Creator,
                transfer_role: TransferRole::Receiver,
                bootstrap_methods: vec![BootstrapKind::FullTicket, BootstrapKind::RoomCode],
                selected_bootstrap_method: None,
            },
        )
        .await?;
        drive_pairing(session, "700007-abcd-1234", &creator_context, &mine_b, None).await
    });

    let join = Duration::from_secs(20);
    let a_got = tokio::time::timeout(join, a)
        .await
        .expect("A timed out")
        .unwrap()
        .expect("A pairs");
    let b_got = tokio::time::timeout(join, b)
        .await
        .expect("B timed out")
        .unwrap()
        .expect("B pairs");

    // Each recovered the OTHER peer's iroh descriptor, sealed under the shared key.
    assert_eq!(a_got.peer, desc_b);
    assert_eq!(b_got.peer, desc_a);
    assert_eq!(a_got.control_key(), b_got.control_key());
    assert_eq!(a_got.control_transcript_hash, b_got.control_transcript_hash);
}
