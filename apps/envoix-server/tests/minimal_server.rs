use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use envoix_invite::RoomCode;
use envoix_pairing::{
    DescriptorPayload, MAX_MESSAGE_BODY, PairingCode, SystemEntropy, WIRE_HEADER_LEN,
    initiator_start, responder_respond,
};
use envoix_rendezvous::{ClientConfig, ControlLimits, Role};
use envoix_rendezvous_iroh::{
    BrokerSession, EndpointConfig, IrohClientConfig, bind_endpoint, join_room,
};
use envoix_server::{ServerConfig, run};
use iroh::SecretKey;
use tempfile::TempDir;

const FULL_ROOM_CODE: &str = "123456-amber-comet";

fn loopback_endpoint_config() -> EndpointConfig {
    EndpointConfig::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        None,
        SecretKey::generate(),
        Duration::from_secs(2),
    )
    .unwrap()
}

fn client_config() -> IrohClientConfig {
    IrohClientConfig::new(
        Duration::from_secs(2),
        Duration::from_secs(2),
        Duration::from_secs(2),
        ClientConfig::new(Duration::from_secs(3), ControlLimits::new(64).unwrap()).unwrap(),
    )
    .unwrap()
}

async fn send_pairing_frame(session: &mut BrokerSession, frame: &[u8]) {
    session.streams_mut().0.write_all(frame).await.unwrap();
}

async fn receive_pairing_frame(session: &mut BrokerSession) -> Vec<u8> {
    let mut header = [0; WIRE_HEADER_LEN];
    session
        .streams_mut()
        .1
        .read_exact(&mut header)
        .await
        .unwrap();
    let body_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    assert!(
        body_len <= MAX_MESSAGE_BODY,
        "pairing frame must be bounded before allocation"
    );
    let mut frame = Vec::with_capacity(WIRE_HEADER_LEN + body_len);
    frame.extend_from_slice(&header);
    frame.resize(WIRE_HEADER_LEN + body_len, 0);
    session
        .streams_mut()
        .1
        .read_exact(&mut frame[WIRE_HEADER_LEN..])
        .await
        .unwrap();
    frame
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn minimal_server_pairs_two_peers() {
    let directory = TempDir::new().unwrap();
    let mut server_config = ServerConfig::operational_defaults();
    server_config.bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    server_config.mailbox_bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    server_config.node_key_path = directory.path().join("rendezvous-node.key");
    server_config.room_ttl = Duration::from_secs(5);
    server_config.relay_ttl = Duration::from_secs(5);
    server_config.join_deadline = Duration::from_secs(2);
    server_config.close_grace = Duration::from_secs(2);
    server_config.handshake_deadline = Duration::from_secs(2);
    server_config.bind_deadline = Duration::from_secs(2);
    server_config.max_waiting_rooms = 16;
    server_config.max_connections = 16;

    let server = run(server_config.clone()).await.unwrap();
    let first_endpoint_id = server.endpoint_id();
    let broker = server.endpoint_addr().clone();
    assert!(server.is_running());

    let client_a = bind_endpoint(loopback_endpoint_config()).await.unwrap();
    let client_b = bind_endpoint(loopback_endpoint_config()).await.unwrap();
    let room_key = RoomCode::parse(FULL_ROOM_CODE).unwrap().namespaced_key();
    assert_eq!(room_key.as_str(), "v2:123456");

    let join_a = join_room(&client_a, broker.clone(), room_key.clone(), client_config());
    let join_b = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        join_room(&client_b, broker, room_key, client_config()).await
    };
    let (session_a, session_b) = tokio::join!(join_a, join_b);
    let mut initiator_session = session_a.unwrap();
    let mut responder_session = session_b.unwrap();
    assert_eq!(initiator_session.role(), Role::Initiator);
    assert_eq!(responder_session.role(), Role::Responder);

    let initiator_code = PairingCode::new(FULL_ROOM_CODE.as_bytes().to_vec()).unwrap();
    let responder_code = PairingCode::new(FULL_ROOM_CODE.as_bytes().to_vec()).unwrap();
    let mut initiator_entropy = SystemEntropy;
    let mut responder_entropy = SystemEntropy;

    let (initiator_waiting, start) =
        initiator_start(&initiator_code, &mut initiator_entropy).unwrap();
    send_pairing_frame(&mut initiator_session, &start).await;
    let start = receive_pairing_frame(&mut responder_session).await;

    let (responder_waiting, response) =
        responder_respond(&responder_code, &start, &mut responder_entropy).unwrap();
    send_pairing_frame(&mut responder_session, &response).await;
    let response = receive_pairing_frame(&mut initiator_session).await;

    let (initiator_confirming, initiator_confirmation) =
        initiator_waiting.receive_response(&response).unwrap();
    send_pairing_frame(&mut initiator_session, &initiator_confirmation).await;
    let initiator_confirmation = receive_pairing_frame(&mut responder_session).await;

    let (mut responder_paired, responder_confirmation) = responder_waiting
        .verify_initiator(&initiator_confirmation)
        .unwrap();
    send_pairing_frame(&mut responder_session, &responder_confirmation).await;
    let responder_confirmation = receive_pairing_frame(&mut initiator_session).await;
    let mut initiator_paired = initiator_confirming
        .verify_responder(&responder_confirmation)
        .unwrap();

    assert_eq!(initiator_paired.data_token(), responder_paired.data_token());

    let initiator_descriptor = DescriptorPayload::new(b"initiator descriptor".to_vec()).unwrap();
    let responder_descriptor = DescriptorPayload::new(b"responder descriptor".to_vec()).unwrap();
    let from_initiator = initiator_paired
        .seal_descriptor(&initiator_descriptor)
        .unwrap();
    let from_responder = responder_paired
        .seal_descriptor(&responder_descriptor)
        .unwrap();
    send_pairing_frame(&mut initiator_session, &from_initiator).await;
    send_pairing_frame(&mut responder_session, &from_responder).await;
    let at_initiator = receive_pairing_frame(&mut initiator_session).await;
    let at_responder = receive_pairing_frame(&mut responder_session).await;

    let opened_by_initiator = initiator_paired
        .open_peer_descriptor(&at_initiator)
        .unwrap();
    let opened_by_responder = responder_paired
        .open_peer_descriptor(&at_responder)
        .unwrap();
    assert_eq!(
        opened_by_initiator.payload().as_bytes(),
        responder_descriptor.as_bytes()
    );
    assert_eq!(
        opened_by_responder.payload().as_bytes(),
        initiator_descriptor.as_bytes()
    );
    assert!(server.is_running());

    let (closed_a, closed_b) = tokio::join!(initiator_session.close(), responder_session.close());
    closed_a.unwrap();
    closed_b.unwrap();
    client_a.close().await;
    client_b.close().await;
    assert!(server.is_running());
    server.shutdown().await.unwrap();

    let restarted = run(server_config).await.unwrap();
    assert_eq!(restarted.endpoint_id(), first_endpoint_id);
    restarted.shutdown().await.unwrap();
}
