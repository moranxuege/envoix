//! Real-world probe: two local clients pair through a REMOTE broker.
//!
//! Usage: `cargo run -p envoix-server --example pair_probe [-- <node-id>@<host:port>]`
//!
//! With no argument it dials THIS BUILD'S deployment — the same
//! `<node_id>@<host>:<port>` that `deploy/environments.toml` derives and that
//! the app freezes into every invite it mints. So "the endpoint the app is
//! built for" and "the endpoint this probe proves is live" are one string, not
//! two that agree by hand.
//!
//! Generates a random room code, joins the broker from two endpoints, runs
//! the full C3 pairing through the blind relay, and exchanges sealed
//! descriptors — the deployment counterpart of `minimal_server_pairs_two_peers`.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use envoix_deployment::BUILD_TARGET;
use envoix_pairing::{
    DescriptorPayload, MAX_MESSAGE_BODY, PairingCode, SystemEntropy, WIRE_HEADER_LEN,
    initiator_start, responder_respond,
};
use envoix_rendezvous::{ClientConfig, ControlLimits, Role};
use envoix_rendezvous_iroh::{
    BrokerSession, EndpointConfig, IrohClientConfig, bind_endpoint, join_room,
};
use iroh::{EndpointAddr, EndpointId, SecretKey, TransportAddr};

/// C4 keeps its own entropy trait; bridge it to the system source.
struct InviteEntropy;

impl envoix_invite::EntropySource for InviteEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), envoix_invite::EntropyError> {
        use envoix_pairing::EntropySource as _;
        SystemEntropy
            .fill(destination)
            .map_err(|_| envoix_invite::EntropyError::Unavailable)
    }
}

fn client_config() -> IrohClientConfig {
    IrohClientConfig::new(
        Duration::from_secs(15),
        Duration::from_secs(15),
        Duration::from_secs(10),
        ClientConfig::new(Duration::from_secs(60), ControlLimits::new(64).unwrap()).unwrap(),
    )
    .unwrap()
}

fn parse_broker(addr: &str) -> Result<EndpointAddr, String> {
    let (id, socket) = addr
        .split_once('@')
        .ok_or("broker must be <node-id>@<host:port>")?;
    let id: EndpointId = id.parse().map_err(|_| "invalid node id".to_string())?;
    let socket: SocketAddr = socket
        .to_socket_addrs()
        .map_err(|error| format!("cannot resolve {socket}: {error}"))?
        .next()
        .ok_or("host resolved to no address")?;
    Ok(EndpointAddr::from_parts(id, [TransportAddr::Ip(socket)]))
}

async fn send_frame(session: &mut BrokerSession, frame: &[u8]) {
    session
        .streams_mut()
        .0
        .write_all(frame)
        .await
        .expect("send");
}

async fn receive_frame(session: &mut BrokerSession) -> Vec<u8> {
    let mut header = [0; WIRE_HEADER_LEN];
    session
        .streams_mut()
        .1
        .read_exact(&mut header)
        .await
        .expect("frame header");
    let body_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    assert!(body_len <= MAX_MESSAGE_BODY, "oversize frame declared");
    let mut frame = Vec::with_capacity(WIRE_HEADER_LEN + body_len);
    frame.extend_from_slice(&header);
    frame.resize(WIRE_HEADER_LEN + body_len, 0);
    session
        .streams_mut()
        .1
        .read_exact(&mut frame[WIRE_HEADER_LEN..])
        .await
        .expect("frame body");
    frame
}

#[tokio::main]
async fn main() {
    let broker_arg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| BUILD_TARGET.rendezvous_endpoint.to_string());
    let broker = parse_broker(&broker_arg).expect("broker address");

    let room_code =
        envoix_invite::generate_room_code(&mut InviteEntropy).expect("generate room code");
    let room_key = room_code.namespaced_key();
    println!("probing {broker_arg} in room {}", room_key.as_str());

    let bind_any: SocketAddr = "0.0.0.0:0".parse().unwrap();
    let client_a = bind_endpoint(
        EndpointConfig::new(
            bind_any,
            None,
            SecretKey::generate(),
            Duration::from_secs(10),
        )
        .unwrap(),
    )
    .await
    .expect("bind client a");
    let client_b = bind_endpoint(
        EndpointConfig::new(
            bind_any,
            None,
            SecretKey::generate(),
            Duration::from_secs(10),
        )
        .unwrap(),
    )
    .await
    .expect("bind client b");

    let started = Instant::now();
    let join_a = join_room(&client_a, broker.clone(), room_key.clone(), client_config());
    let join_b = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        join_room(&client_b, broker, room_key, client_config()).await
    };
    let (session_a, session_b) = tokio::join!(join_a, join_b);
    let mut session_a = session_a.expect("join a");
    let mut session_b = session_b.expect("join b");
    println!(
        "joined in {:?}: a={:?} b={:?}",
        started.elapsed(),
        session_a.role(),
        session_b.role()
    );
    let (initiator, responder) = match (session_a.role(), session_b.role()) {
        (Role::Initiator, Role::Responder) => (&mut session_a, &mut session_b),
        (Role::Responder, Role::Initiator) => (&mut session_b, &mut session_a),
        other => panic!("broker assigned {other:?}"),
    };

    let code = PairingCode::new(room_code.as_str().as_bytes().to_vec()).expect("code");
    let mut entropy = SystemEntropy;
    let (initiator_waiting, start) = initiator_start(&code, &mut entropy).expect("start");
    send_frame(initiator, &start).await;
    let start = receive_frame(responder).await;
    let (responder_waiting, response) =
        responder_respond(&code, &start, &mut entropy).expect("respond");
    send_frame(responder, &response).await;
    let response = receive_frame(initiator).await;
    let (initiator_confirming, confirmation) = initiator_waiting
        .receive_response(&response)
        .expect("response");
    send_frame(initiator, &confirmation).await;
    let confirmation = receive_frame(responder).await;
    let (mut responder_paired, responder_confirmation) = responder_waiting
        .verify_initiator(&confirmation)
        .expect("verify initiator");
    send_frame(responder, &responder_confirmation).await;
    let responder_confirmation = receive_frame(initiator).await;
    let mut initiator_paired = initiator_confirming
        .verify_responder(&responder_confirmation)
        .expect("verify responder");
    println!("SPAKE2 pairing complete in {:?}", started.elapsed());

    let sealed_i = initiator_paired
        .seal_descriptor(&DescriptorPayload::new(b"probe-initiator".to_vec()).unwrap())
        .expect("seal");
    let sealed_r = responder_paired
        .seal_descriptor(&DescriptorPayload::new(b"probe-responder".to_vec()).unwrap())
        .expect("seal");
    send_frame(initiator, &sealed_i).await;
    send_frame(responder, &sealed_r).await;
    let at_i = receive_frame(initiator).await;
    let at_r = receive_frame(responder).await;
    let opened_i = initiator_paired.open_peer_descriptor(&at_i).expect("open");
    let opened_r = responder_paired.open_peer_descriptor(&at_r).expect("open");
    assert_eq!(opened_i.payload().as_bytes(), b"probe-responder");
    assert_eq!(opened_r.payload().as_bytes(), b"probe-initiator");
    assert_eq!(
        initiator_paired.data_token(),
        responder_paired.data_token(),
        "both sides must derive the same data-plane token"
    );

    let (closed_a, closed_b) = tokio::join!(session_a.close(), session_b.close());
    closed_a.expect("close a");
    closed_b.expect("close b");
    client_a.close().await;
    client_b.close().await;
    println!(
        "PASS: paired + descriptors exchanged in {:?}",
        started.elapsed()
    );
}
