//! End-to-end: two peers join a room through the broker and run the full
//! `envoix-pairing` exchange (SPAKE2 + sealed descriptors) over the broker's
//! blind relay. Uses in-memory duplexes - no sockets, no iroh.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use envoix_pairing::{
    Confirm, PakeResponse, PakeStart, initiator_start, open_json, responder_respond, seal_json,
};
use envoix_rendezvous::{
    BootstrapKind, BrokerConfig, BrokerOutcome, BrokerRejection, InvitationSide, Join, PeerConn,
    PeerSource, RENDEZVOUS_PROTOCOL_VERSION, RateLimitConfig, RendezvousError, Reply, Role,
    RoomRegistry, TransferRole, read_framed, write_framed,
};
use tokio::io::{AsyncWriteExt, DuplexStream};

/// Wrap the broker's side of a duplex as a `PeerConn` (the halves own the
/// stream, so no separate keep-alive is needed).
fn broker_conn(stream: DuplexStream) -> PeerConn {
    let (reader, writer) = tokio::io::split(stream);
    PeerConn::new(writer, reader, ())
}

fn creator_join(room_id: &str, role: TransferRole) -> Join {
    Join {
        version: RENDEZVOUS_PROTOCOL_VERSION,
        room_id: room_id.to_string(),
        invitation_side: InvitationSide::Creator,
        transfer_role: role,
        bootstrap_methods: vec![BootstrapKind::FullTicket, BootstrapKind::RoomCode],
        selected_bootstrap_method: None,
    }
}

fn joiner_join(room_id: &str, role: TransferRole) -> Join {
    Join {
        version: RENDEZVOUS_PROTOCOL_VERSION,
        room_id: room_id.to_string(),
        invitation_side: InvitationSide::Joiner,
        transfer_role: role,
        bootstrap_methods: Vec::new(),
        selected_bootstrap_method: Some(BootstrapKind::RoomCode),
    }
}

fn remembered_creator_join(room_id: &str) -> Join {
    Join {
        bootstrap_methods: vec![BootstrapKind::FullTicket],
        ..creator_join(room_id, TransferRole::Receiver)
    }
}

fn remembered_joiner_join(room_id: &str) -> Join {
    Join {
        selected_bootstrap_method: Some(BootstrapKind::FullTicket),
        ..joiner_join(room_id, TransferRole::Sender)
    }
}

fn room_control_creator_join(room_id: &str) -> Join {
    Join {
        bootstrap_methods: vec![BootstrapKind::RoomCode],
        ..creator_join(room_id, TransferRole::Receiver)
    }
}

fn room_control_joiner_join(room_id: &str) -> Join {
    joiner_join(room_id, TransferRole::Sender)
}

fn control_context(room_id: &str) -> envoix_invite::InvitationControlContext {
    envoix_invite::InvitationControlContext::new(
        room_id.to_string(),
        BootstrapKind::RoomCode,
        TransferRole::Receiver,
        TransferRole::Sender,
    )
    .unwrap()
}

/// Drive the initiator client over `stream`; returns the role the broker
/// assigned and the peer descriptor recovered from the other side.
async fn run_initiator(
    stream: DuplexStream,
    room: &str,
    code: &str,
    my_descriptor: &str,
) -> Result<(Role, String), Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(&mut writer, &joiner_join(room, TransferRole::Sender)).await?;
    let reply: Reply = read_framed(&mut reader).await?;
    let Reply::Paired(paired) = reply else {
        panic!("expected Paired, got {reply:?}");
    };

    let (pending, start) = initiator_start(code, &control_context(room))?;
    write_framed(&mut writer, &start).await?;
    let response: PakeResponse = read_framed(&mut reader).await?;
    let (confirming, confirm) = pending.finish(&response)?;
    write_framed(&mut writer, &confirm).await?;
    let responder_confirm: Confirm = read_framed(&mut reader).await?;
    let key = confirming.verify(&responder_confirm)?;

    // Seal our descriptor under K and exchange.
    write_framed(
        &mut writer,
        &seal_json(key.key(), b"room-test", &my_descriptor.to_string())?,
    )
    .await?;
    let sealed: Vec<u8> = read_framed(&mut reader).await?;
    let other: String = open_json(key.key(), b"room-test", &sealed)?;
    Ok((paired.role, other))
}

/// Drive the responder client over `stream`.
async fn run_responder(
    stream: DuplexStream,
    room: &str,
    code: &str,
    my_descriptor: &str,
) -> Result<(Role, String), Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(&mut writer, &creator_join(room, TransferRole::Receiver)).await?;
    let reply: Reply = read_framed(&mut reader).await?;
    let Reply::Paired(paired) = reply else {
        panic!("expected Paired, got {reply:?}");
    };

    let start: PakeStart = read_framed(&mut reader).await?;
    let (confirming, response) = responder_respond(code, &control_context(room), &start)?;
    write_framed(&mut writer, &response).await?;
    let initiator_confirm: Confirm = read_framed(&mut reader).await?;
    let (key, confirm) = confirming.verify(&initiator_confirm)?;
    write_framed(&mut writer, &confirm).await?;

    write_framed(
        &mut writer,
        &seal_json(key.key(), b"room-test", &my_descriptor.to_string())?,
    )
    .await?;
    let sealed: Vec<u8> = read_framed(&mut reader).await?;
    let other: String = open_json(key.key(), b"room-test", &sealed)?;
    Ok((paired.role, other))
}

async fn join_only(
    stream: DuplexStream,
    join: Join,
) -> Result<Reply, Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(&mut writer, &join).await?;
    Ok(read_framed(&mut reader).await?)
}

fn abuse_test_config() -> BrokerConfig {
    let generous = RateLimitConfig {
        events: 100,
        period: Duration::from_secs(60),
        burst: 100,
    };
    BrokerConfig {
        room_ttl: Duration::from_secs(5),
        room_tombstone_ttl: Duration::from_secs(5),
        room_attempt_rate: generous,
        endpoint_join_rate: generous,
        ip_join_rate: generous,
        subnet_join_rate: generous,
        ..BrokerConfig::default()
    }
}

fn start_peer(
    registry: Arc<RoomRegistry>,
    join: Join,
) -> (
    tokio::task::JoinHandle<Reply>,
    tokio::task::JoinHandle<Result<(), RendezvousError>>,
) {
    let (client, broker) = tokio::io::duplex(64 * 1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let reply = tokio::spawn(async move { join_only(client, join).await.unwrap() });
    (reply, serve)
}

struct HeldPeer {
    reply: tokio::sync::oneshot::Receiver<Reply>,
    release: tokio::sync::oneshot::Sender<()>,
    client: tokio::task::JoinHandle<()>,
    serve: tokio::task::JoinHandle<Result<(), RendezvousError>>,
}

fn start_held_peer(registry: Arc<RoomRegistry>, join: Join) -> HeldPeer {
    let (client, broker) = tokio::io::duplex(64 * 1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let client = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(client);
        write_framed(&mut writer, &join).await.unwrap();
        let reply = read_framed(&mut reader).await.unwrap();
        reply_tx.send(reply).ok();
        release_rx.await.ok();
    });
    HeldPeer {
        reply: reply_rx,
        release: release_tx,
        client,
        serve,
    }
}

fn start_sourced_peer(
    registry: Arc<RoomRegistry>,
    source: PeerSource,
    join: Join,
) -> (
    tokio::task::JoinHandle<Reply>,
    tokio::task::JoinHandle<Result<(), RendezvousError>>,
) {
    let (client, broker) = tokio::io::duplex(64 * 1024);
    let serve = tokio::spawn(async move { registry.serve_from(broker_conn(broker), source).await });
    let reply = tokio::spawn(async move { join_only(client, join).await.unwrap() });
    (reply, serve)
}

async fn wait_for_creator(registry: &RoomRegistry) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while registry.metrics_snapshot().waiting_creators == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("creator was not parked");
}

async fn match_once(registry: Arc<RoomRegistry>, room: &str) {
    let (creator, creator_serve) =
        start_peer(registry.clone(), creator_join(room, TransferRole::Receiver));
    wait_for_creator(&registry).await;
    let (joiner, joiner_serve) = start_peer(registry, joiner_join(room, TransferRole::Sender));
    assert!(matches!(creator.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(joiner.await.unwrap(), Reply::Paired(_)));
    creator_serve.await.unwrap().unwrap();
    joiner_serve.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_peers_pair_and_exchange_descriptors() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(5)));
    let (client_a, broker_a) = tokio::io::duplex(64 * 1024);
    let (client_b, broker_b) = tokio::io::duplex(64 * 1024);

    let r1 = registry.clone();
    let s1 = tokio::spawn(async move { r1.serve(broker_conn(broker_a)).await });
    let r2 = registry.clone();
    let s2 = tokio::spawn(async move { r2.serve(broker_conn(broker_b)).await });

    let a = tokio::spawn(async move {
        run_responder(client_a, "420042", "12-orange-tiger", "endpoint-A").await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let b = tokio::spawn(async move {
        run_initiator(client_b, "420042", "12-orange-tiger", "endpoint-B").await
    });

    let (role_a, a_got) = a.await.unwrap().expect("creator pairs");
    let (role_b, b_got) = b.await.unwrap().expect("joiner pairs");

    assert_eq!(role_a, Role::Responder);
    assert_eq!(role_b, Role::Initiator);
    // Each recovered the OTHER peer's descriptor, sealed under the shared key.
    assert_eq!(a_got, "endpoint-B");
    assert_eq!(b_got, "endpoint-A");

    s1.await.unwrap().expect("broker serves A");
    s2.await.unwrap().expect("broker serves B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remembered_locator_pairs_only_the_fixed_complementary_roles() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(5)));
    let room = format!("r1_{}", "A".repeat(43));
    let (receiver, broker_receiver) = tokio::io::duplex(4096);
    let (sender, broker_sender) = tokio::io::duplex(4096);

    let receiver_registry = registry.clone();
    let receiver_serve =
        tokio::spawn(async move { receiver_registry.serve(broker_conn(broker_receiver)).await });
    let sender_registry = registry.clone();
    let sender_serve =
        tokio::spawn(async move { sender_registry.serve(broker_conn(broker_sender)).await });

    let receiver_join = remembered_creator_join(&room);
    let sender_join = remembered_joiner_join(&room);
    let receiver = tokio::spawn(async move { join_only(receiver, receiver_join).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let sender = tokio::spawn(async move { join_only(sender, sender_join).await });

    let Reply::Paired(receiver) = receiver.await.unwrap().unwrap() else {
        panic!("receiver was not paired");
    };
    let Reply::Paired(sender) = sender.await.unwrap().unwrap() else {
        panic!("sender was not paired");
    };
    assert_eq!(receiver.role, Role::Responder);
    assert_eq!(sender.role, Role::Initiator);
    assert_eq!(
        receiver.selected_bootstrap_method,
        BootstrapKind::FullTicket
    );

    receiver_serve.await.unwrap().expect("serve receiver");
    sender_serve.await.unwrap().expect("serve sender");
}

#[tokio::test]
async fn remembered_locator_rejects_reversed_roles() {
    let registry = Arc::new(RoomRegistry::new());
    let (mut client, broker) = tokio::io::duplex(4096);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let room = format!("r1_{}", "B".repeat(43));
    let (_reader, mut writer) = tokio::io::split(&mut client);
    write_framed(
        &mut writer,
        &Join {
            transfer_role: TransferRole::Sender,
            ..remembered_creator_join(&room)
        },
    )
    .await
    .unwrap();

    assert!(matches!(
        serve.await.unwrap(),
        Err(envoix_rendezvous::RendezvousError::Rejected(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_control_locator_pairs_only_the_fixed_control_roles() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(5)));
    let room = "c1_123456";
    let (creator, creator_serve) = start_peer(registry.clone(), room_control_creator_join(room));
    wait_for_creator(&registry).await;
    let (joiner, joiner_serve) = start_peer(registry, room_control_joiner_join(room));

    let Reply::Paired(creator) = creator.await.unwrap() else {
        panic!("room-control creator was not paired");
    };
    let Reply::Paired(joiner) = joiner.await.unwrap() else {
        panic!("room-control joiner was not paired");
    };
    assert_eq!(creator.role, Role::Responder);
    assert_eq!(joiner.role, Role::Initiator);
    assert_eq!(creator.selected_bootstrap_method, BootstrapKind::RoomCode);
    assert_eq!(joiner.selected_bootstrap_method, BootstrapKind::RoomCode);

    creator_serve.await.unwrap().expect("serve creator");
    joiner_serve.await.unwrap().expect("serve joiner");
}

#[tokio::test]
async fn room_control_locator_rejects_non_control_join_shapes() {
    let invalid_joins = [
        creator_join("c1_123456", TransferRole::Receiver),
        Join {
            transfer_role: TransferRole::Sender,
            ..room_control_creator_join("c1_123456")
        },
        Join {
            selected_bootstrap_method: Some(BootstrapKind::FullTicket),
            ..room_control_joiner_join("c1_123456")
        },
        Join {
            transfer_role: TransferRole::Receiver,
            ..room_control_joiner_join("c1_123456")
        },
    ];

    for join in invalid_joins {
        let registry = Arc::new(RoomRegistry::new());
        let (mut client, broker) = tokio::io::duplex(4096);
        let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
        let (_reader, mut writer) = tokio::io::split(&mut client);
        write_framed(&mut writer, &join).await.unwrap();

        assert!(matches!(
            serve.await.unwrap(),
            Err(envoix_rendezvous::RendezvousError::Rejected(_))
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lone_peer_expires() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_millis(200)));
    let (mut client, broker) = tokio::io::duplex(4096);

    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });

    // Join a room nobody else joins.
    let (mut reader, mut writer) = tokio::io::split(&mut client);
    write_framed(&mut writer, &creator_join("100001", TransferRole::Receiver))
        .await
        .unwrap();

    // The broker gives up after the TTL.
    let result = serve.await.unwrap();
    assert!(matches!(
        result,
        Err(envoix_rendezvous::RendezvousError::Expired)
    ));

    // The parked peer is told the room expired before the stream closes.
    let reply: Reply = read_framed(&mut reader).await.unwrap();
    assert_eq!(reply, Reply::Expired);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_direction_peers_do_not_match() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_millis(150)));
    let (sender_a, broker_a) = tokio::io::duplex(4096);
    let (sender_b, broker_b) = tokio::io::duplex(4096);

    let registry_a = registry.clone();
    let serve_a = tokio::spawn(async move { registry_a.serve(broker_conn(broker_a)).await });
    let registry_b = registry.clone();
    let serve_b = tokio::spawn(async move { registry_b.serve(broker_conn(broker_b)).await });

    let first = tokio::spawn(async move {
        join_only(sender_a, creator_join("200002", TransferRole::Sender)).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = tokio::spawn(async move {
        join_only(sender_b, joiner_join("200002", TransferRole::Sender)).await
    });

    assert_eq!(first.await.unwrap().unwrap(), Reply::Expired);
    assert!(matches!(
        second.await.unwrap().unwrap(),
        Reply::Rejected(envoix_rendezvous::BrokerRejection {
            outcome: envoix_rendezvous::BrokerOutcome::RoomNotFound,
            ..
        })
    ));
    assert!(matches!(
        serve_a.await.unwrap(),
        Err(envoix_rendezvous::RendezvousError::Expired)
    ));
    assert!(matches!(
        serve_b.await.unwrap(),
        Err(envoix_rendezvous::RendezvousError::Rejected(
            envoix_rendezvous::BrokerOutcome::RoomNotFound
        ))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_long_room_id_is_rejected() {
    let registry = Arc::new(RoomRegistry::new());
    let (mut client, broker) = tokio::io::duplex(64 * 1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });

    let (_reader, mut writer) = tokio::io::split(&mut client);
    write_framed(
        &mut writer,
        &Join {
            room_id: "x".repeat(1024),
            ..creator_join("300003", TransferRole::Sender)
        },
    )
    .await
    .unwrap();

    let result = serve.await.unwrap();
    assert!(matches!(
        result,
        Err(envoix_rendezvous::RendezvousError::Rejected(_))
    ));
}

/// The dead-slot bug (observed in the field): a peer that joins and then
/// disconnects while parked must be evicted immediately - not linger until the
/// TTL, consume the next join, and leave the real partner parked in an emptied
/// room. Sequence: A joins and dies; B then C join the same room; B and C must
/// pair with each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_waiter_is_evicted_and_next_two_peers_pair() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(30)));

    // A joins, then its connection dies (both halves dropped -> broker EOF).
    let (a_client, a_broker) = tokio::io::duplex(4096);
    let ra = registry.clone();
    let serve_a = tokio::spawn(async move { ra.serve(broker_conn(a_broker)).await });
    {
        let (_reader, mut writer) = tokio::io::split(a_client);
        write_framed(&mut writer, &creator_join("400004", TransferRole::Receiver))
            .await
            .unwrap();
    } // a_client dropped here

    // A's serve task must end promptly with an eviction error - well before
    // the 30s TTL, which is the pre-fix behavior.
    let a_result = tokio::time::timeout(Duration::from_secs(2), serve_a)
        .await
        .expect("dead waiter was not evicted promptly (dead-slot bug)")
        .unwrap();
    assert!(matches!(
        a_result,
        Err(envoix_rendezvous::RendezvousError::Io(_))
    ));
    assert_eq!(
        registry.metrics_snapshot().active_rooms,
        1,
        "human Room state must retain its original expiry and abuse budget"
    );

    // B and C join the same room and must pair with each other, not the corpse.
    let (client_b, broker_b) = tokio::io::duplex(64 * 1024);
    let (client_c, broker_c) = tokio::io::duplex(64 * 1024);
    let rb = registry.clone();
    let sb = tokio::spawn(async move { rb.serve(broker_conn(broker_b)).await });
    let rc = registry.clone();
    let sc = tokio::spawn(async move { rc.serve(broker_conn(broker_c)).await });

    let b = tokio::spawn(async move {
        run_responder(client_b, "400004", "12-kelp-coral", "endpoint-B").await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let c = tokio::spawn(async move {
        run_initiator(client_c, "400004", "12-kelp-coral", "endpoint-C").await
    });

    let (role_b, b_got) = b.await.unwrap().expect("B pairs despite the corpse");
    let (role_c, c_got) = c.await.unwrap().expect("C pairs despite the corpse");
    assert_eq!(role_b, Role::Responder);
    assert_eq!(role_c, Role::Initiator);
    assert_eq!(b_got, "endpoint-C");
    assert_eq!(c_got, "endpoint-B");

    sb.await.unwrap().expect("broker serves B");
    sc.await.unwrap().expect("broker serves C");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_remembered_waiter_releases_its_live_locator() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(30)));
    let room = format!("r1_{}", "C".repeat(43));

    let (mut canceled_client, canceled_broker) = tokio::io::duplex(4096);
    let canceled_registry = registry.clone();
    let canceled_serve =
        tokio::spawn(async move { canceled_registry.serve(broker_conn(canceled_broker)).await });
    write_framed(&mut canceled_client, &remembered_creator_join(&room))
        .await
        .unwrap();
    wait_for_creator(&registry).await;
    drop(canceled_client);

    let canceled_result = tokio::time::timeout(Duration::from_secs(2), canceled_serve)
        .await
        .expect("canceled remembered waiter was not evicted promptly")
        .unwrap();
    assert!(matches!(canceled_result, Err(RendezvousError::Io(_))));
    let metrics = registry.metrics_snapshot();
    assert_eq!(metrics.waiting_creators, 0);
    assert_eq!(metrics.room_connections, 0);
    assert_eq!(
        metrics.active_rooms, 0,
        "an unpaired remembered locator must not retain a stale live Room"
    );

    let (responder, responder_serve) = start_peer(registry.clone(), remembered_creator_join(&room));
    wait_for_creator(&registry).await;
    let (connector, connector_serve) = start_peer(registry.clone(), remembered_joiner_join(&room));
    assert!(matches!(responder.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(connector.await.unwrap(), Reply::Paired(_)));
    responder_serve.await.unwrap().unwrap();
    connector_serve.await.unwrap().unwrap();
    assert_eq!(registry.metrics_snapshot().matches, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_remembered_match_releases_its_live_locator() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_secs(30)));
    let room = format!("r1_{}", "D".repeat(43));

    let (responder, responder_serve) = start_peer(registry.clone(), remembered_creator_join(&room));
    wait_for_creator(&registry).await;
    let (connector, connector_serve) = start_peer(registry.clone(), remembered_joiner_join(&room));
    assert!(matches!(responder.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(connector.await.unwrap(), Reply::Paired(_)));
    responder_serve.await.unwrap().unwrap();
    connector_serve.await.unwrap().unwrap();

    let metrics = registry.metrics_snapshot();
    assert_eq!(metrics.matches, 1);
    assert_eq!(metrics.room_connections, 0);
    assert_eq!(
        metrics.active_rooms, 0,
        "a consumed high-entropy locator must not occupy broker capacity until its TTL"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matched_remembered_locator_rejects_a_third_peer_until_release() {
    let config = BrokerConfig {
        room_ttl: Duration::from_millis(100),
        max_connections_per_room: 3,
        ..abuse_test_config()
    };
    let registry = Arc::new(RoomRegistry::with_config(config).unwrap());
    let room = format!("r1_{}", "E".repeat(43));

    let HeldPeer {
        reply: responder_reply,
        release: release_responder,
        client: responder,
        serve: responder_serve,
    } = start_held_peer(registry.clone(), remembered_creator_join(&room));
    wait_for_creator(&registry).await;
    let HeldPeer {
        reply: connector_reply,
        release: release_connector,
        client: connector,
        serve: connector_serve,
    } = start_held_peer(registry.clone(), remembered_joiner_join(&room));
    assert!(matches!(responder_reply.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(connector_reply.await.unwrap(), Reply::Paired(_)));

    let (third, third_serve) = start_peer(registry.clone(), remembered_creator_join(&room));
    let third = tokio::time::timeout(Duration::from_secs(1), third)
        .await
        .expect("a consumed remembered locator must reject a third peer")
        .unwrap();
    assert!(matches!(
        third,
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomFull,
            retry_after: Some(_),
        })
    ));
    assert!(third_serve.await.unwrap().is_err());

    let (fourth, fourth_serve) = start_peer(registry.clone(), remembered_joiner_join(&room));
    let fourth = tokio::time::timeout(Duration::from_secs(1), fourth)
        .await
        .expect("a consumed remembered locator must reject a later connector")
        .unwrap();
    assert!(matches!(
        fourth,
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomFull,
            retry_after: Some(_),
        })
    ));
    assert!(fourth_serve.await.unwrap().is_err());

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        registry.metrics_snapshot().active_rooms,
        1,
        "the waiting-room TTL must not tombstone a locator whose pair is already matched"
    );

    release_responder.send(()).ok();
    release_connector.send(()).ok();
    responder.await.unwrap();
    connector.await.unwrap();
    responder_serve.await.unwrap().unwrap();
    connector_serve.await.unwrap().unwrap();
    assert_eq!(registry.metrics_snapshot().active_rooms, 0);

    let (next_responder, next_responder_serve) =
        start_peer(registry.clone(), remembered_creator_join(&room));
    wait_for_creator(&registry).await;
    let (next_connector, next_connector_serve) =
        start_peer(registry.clone(), remembered_joiner_join(&room));
    assert!(matches!(next_responder.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(next_connector.await.unwrap(), Reply::Paired(_)));
    next_responder_serve.await.unwrap().unwrap();
    next_connector_serve.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creator_first_room_charges_matches_and_exhausts_without_reset() {
    let config = BrokerConfig {
        room_attempt_limit: 2,
        ..abuse_test_config()
    };
    let registry = Arc::new(RoomRegistry::with_config(config).unwrap());

    match_once(registry.clone(), "510001").await;
    match_once(registry.clone(), "510001").await;

    let (creator, serve) = start_peer(
        registry.clone(),
        creator_join("510001", TransferRole::Receiver),
    );
    assert_eq!(
        creator.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomUnderAttack,
            retry_after: None,
        })
    );
    assert!(matches!(
        serve.await.unwrap(),
        Err(RendezvousError::Rejected(BrokerOutcome::RoomUnderAttack))
    ));

    match_once(registry.clone(), "510002").await;
    let metrics = registry.metrics_snapshot();
    assert_eq!(metrics.matches, 3);
    assert_eq!(metrics.exhausted_rooms, 1);
    assert_eq!(metrics.active_rooms, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unmatched_and_incompatible_joiners_never_pin_creator_slots() {
    let registry = Arc::new(RoomRegistry::with_config(abuse_test_config()).unwrap());

    let (early, early_serve) = start_peer(
        registry.clone(),
        joiner_join("520001", TransferRole::Sender),
    );
    assert!(matches!(
        early.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomNotFound,
            ..
        })
    ));
    assert!(early_serve.await.unwrap().is_err());
    assert_eq!(registry.metrics_snapshot().active_rooms, 0);

    let (creator, creator_serve) = start_peer(
        registry.clone(),
        creator_join("520001", TransferRole::Receiver),
    );
    wait_for_creator(&registry).await;

    let (incompatible, incompatible_serve) = start_peer(
        registry.clone(),
        joiner_join("520001", TransferRole::Receiver),
    );
    assert!(matches!(
        incompatible.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomNotFound,
            ..
        })
    ));
    assert!(incompatible_serve.await.unwrap().is_err());
    assert_eq!(registry.metrics_snapshot().waiting_creators, 1);

    let (duplicate, duplicate_serve) = start_peer(
        registry.clone(),
        creator_join("520001", TransferRole::Receiver),
    );
    assert!(matches!(
        duplicate.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomFull,
            ..
        })
    ));
    assert!(duplicate_serve.await.unwrap().is_err());

    let (joiner, joiner_serve) = start_peer(
        registry.clone(),
        joiner_join("520001", TransferRole::Sender),
    );
    assert!(matches!(creator.await.unwrap(), Reply::Paired(_)));
    assert!(matches!(joiner.await.unwrap(), Reply::Paired(_)));
    creator_serve.await.unwrap().unwrap();
    joiner_serve.await.unwrap().unwrap();
    assert_eq!(registry.metrics_snapshot().matches, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_joiners_charge_exactly_one_match() {
    let registry = Arc::new(RoomRegistry::with_config(abuse_test_config()).unwrap());
    let (creator, creator_serve) = start_peer(
        registry.clone(),
        creator_join("525001", TransferRole::Receiver),
    );
    wait_for_creator(&registry).await;
    let (joiner_a, serve_a) = start_peer(
        registry.clone(),
        joiner_join("525001", TransferRole::Sender),
    );
    let (joiner_b, serve_b) = start_peer(
        registry.clone(),
        joiner_join("525001", TransferRole::Sender),
    );

    assert!(matches!(creator.await.unwrap(), Reply::Paired(_)));
    let replies = [joiner_a.await.unwrap(), joiner_b.await.unwrap()];
    assert_eq!(
        replies
            .iter()
            .filter(|reply| matches!(reply, Reply::Paired(_)))
            .count(),
        1
    );
    assert_eq!(
        replies
            .iter()
            .filter(|reply| {
                matches!(
                    reply,
                    Reply::Rejected(BrokerRejection {
                        outcome: BrokerOutcome::RoomFull | BrokerOutcome::RoomNotFound,
                        ..
                    })
                )
            })
            .count(),
        1
    );
    creator_serve.await.unwrap().unwrap();
    let results = [serve_a.await.unwrap(), serve_b.await.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(registry.metrics_snapshot().matches, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_rate_limit_preserves_creator_and_bounds_retry_after() {
    let config = BrokerConfig {
        room_attempt_limit: 10,
        room_attempt_rate: RateLimitConfig {
            events: 1,
            period: Duration::from_secs(60),
            burst: 1,
        },
        max_retry_after: Duration::from_secs(3),
        ..abuse_test_config()
    };
    let registry = Arc::new(RoomRegistry::with_config(config).unwrap());
    match_once(registry.clone(), "530001").await;

    let (creator, creator_serve) = start_peer(
        registry.clone(),
        creator_join("530001", TransferRole::Receiver),
    );
    wait_for_creator(&registry).await;
    let (joiner, joiner_serve) = start_peer(
        registry.clone(),
        joiner_join("530001", TransferRole::Sender),
    );
    assert_eq!(
        joiner.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomRateLimited,
            retry_after: Some(3),
        })
    );
    assert!(joiner_serve.await.unwrap().is_err());
    assert_eq!(registry.metrics_snapshot().waiting_creators, 1);
    assert_eq!(registry.metrics_snapshot().matches, 1);

    creator.abort();
    let _ = creator.await;
    assert!(creator_serve.await.unwrap().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_and_source_state_caps_return_server_busy_without_growth() {
    let config = BrokerConfig {
        max_room_states: 1,
        max_waiting_creators: 1,
        ..abuse_test_config()
    };
    let registry = Arc::new(RoomRegistry::with_config(config).unwrap());
    let (first, first_serve) = start_peer(
        registry.clone(),
        creator_join("540001", TransferRole::Receiver),
    );
    wait_for_creator(&registry).await;

    let (second, second_serve) = start_peer(
        registry.clone(),
        creator_join("540002", TransferRole::Receiver),
    );
    assert!(matches!(
        second.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::ServerBusy,
            ..
        })
    ));
    assert!(second_serve.await.unwrap().is_err());
    assert_eq!(registry.metrics_snapshot().active_rooms, 1);
    assert_eq!(registry.metrics_snapshot().server_busy_rejections, 1);

    first.abort();
    let _ = first.await;
    assert!(first_serve.await.unwrap().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_and_idle_join_frames_are_structured_and_counted() {
    let oversized_registry = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            max_frame_body: 32,
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let (mut client, broker) = tokio::io::duplex(4096);
    let registry = oversized_registry.clone();
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    client.write_all(&33_u32.to_be_bytes()).await.unwrap();
    let reply: Reply = read_framed(&mut client).await.unwrap();
    assert!(matches!(
        reply,
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::MalformedJoin,
            ..
        })
    ));
    assert!(serve.await.unwrap().is_err());
    let metrics = oversized_registry.metrics_snapshot();
    assert_eq!(metrics.oversized_frames, 1);
    assert_eq!(metrics.malformed_joins, 1);

    let timeout_registry = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            join_timeout: Duration::from_millis(20),
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let (mut client, broker) = tokio::io::duplex(4096);
    let registry = timeout_registry.clone();
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let reply: Reply = read_framed(&mut client).await.unwrap();
    assert!(matches!(
        reply,
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::MalformedJoin,
            ..
        })
    ));
    assert!(serve.await.unwrap().is_err());
    assert_eq!(timeout_registry.metrics_snapshot().timeouts, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_match_frames_obey_size_and_idle_deadlines() {
    let config = BrokerConfig {
        max_frame_body: 512,
        relay_idle_timeout: Duration::from_millis(30),
        close_grace: Duration::from_millis(20),
        ..abuse_test_config()
    };
    let registry = Arc::new(RoomRegistry::with_config(config).unwrap());
    let (mut creator, creator_broker) = tokio::io::duplex(4096);
    let creator_registry = registry.clone();
    let creator_serve =
        tokio::spawn(async move { creator_registry.serve(broker_conn(creator_broker)).await });
    write_framed(
        &mut creator,
        &creator_join("550001", TransferRole::Receiver),
    )
    .await
    .unwrap();
    wait_for_creator(&registry).await;

    let (mut joiner, joiner_broker) = tokio::io::duplex(4096);
    let joiner_registry = registry.clone();
    let joiner_serve =
        tokio::spawn(async move { joiner_registry.serve(broker_conn(joiner_broker)).await });
    write_framed(&mut joiner, &joiner_join("550001", TransferRole::Sender))
        .await
        .unwrap();
    assert!(matches!(
        read_framed::<_, Reply>(&mut creator).await.unwrap(),
        Reply::Paired(_)
    ));
    assert!(matches!(
        read_framed::<_, Reply>(&mut joiner).await.unwrap(),
        Reply::Paired(_)
    ));

    creator.write_all(&513_u32.to_be_bytes()).await.unwrap();
    drop(creator);
    drop(joiner);
    creator_serve.await.unwrap().unwrap();
    joiner_serve.await.unwrap().unwrap();
    assert_eq!(registry.metrics_snapshot().oversized_frames, 1);

    let idle_registry = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            relay_idle_timeout: Duration::from_millis(20),
            close_grace: Duration::from_millis(20),
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let (mut creator, creator_broker) = tokio::io::duplex(4096);
    let registry_clone = idle_registry.clone();
    let creator_serve =
        tokio::spawn(async move { registry_clone.serve(broker_conn(creator_broker)).await });
    write_framed(
        &mut creator,
        &creator_join("550002", TransferRole::Receiver),
    )
    .await
    .unwrap();
    wait_for_creator(&idle_registry).await;
    let (mut joiner, joiner_broker) = tokio::io::duplex(4096);
    let registry_clone = idle_registry.clone();
    let joiner_serve =
        tokio::spawn(async move { registry_clone.serve(broker_conn(joiner_broker)).await });
    write_framed(&mut joiner, &joiner_join("550002", TransferRole::Sender))
        .await
        .unwrap();
    let _: Reply = read_framed(&mut creator).await.unwrap();
    let _: Reply = read_framed(&mut joiner).await.unwrap();
    creator_serve.await.unwrap().unwrap();
    joiner_serve.await.unwrap().unwrap();
    assert!(idle_registry.metrics_snapshot().timeouts >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_limits_are_structured_on_the_wire() {
    let endpoint_limited = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            endpoint_join_rate: RateLimitConfig {
                events: 1,
                period: Duration::from_secs(60),
                burst: 1,
            },
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let source = PeerSource::new([7; 32], None);
    let (first, first_serve) = start_sourced_peer(
        endpoint_limited.clone(),
        source,
        joiner_join("560001", TransferRole::Sender),
    );
    assert!(matches!(
        first.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomNotFound,
            ..
        })
    ));
    assert!(first_serve.await.unwrap().is_err());
    let (second, second_serve) = start_sourced_peer(
        endpoint_limited.clone(),
        source,
        joiner_join("560002", TransferRole::Sender),
    );
    assert!(matches!(
        second.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::EndpointRateLimited,
            ..
        })
    ));
    assert!(second_serve.await.unwrap().is_err());

    let ip_limited = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            ip_join_rate: RateLimitConfig {
                events: 1,
                period: Duration::from_secs(60),
                burst: 1,
            },
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let ip = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)));
    let (first, first_serve) = start_sourced_peer(
        ip_limited.clone(),
        PeerSource::new([8; 32], ip),
        joiner_join("560003", TransferRole::Sender),
    );
    let _ = first.await.unwrap();
    assert!(first_serve.await.unwrap().is_err());
    let (second, second_serve) = start_sourced_peer(
        ip_limited.clone(),
        PeerSource::new([9; 32], ip),
        joiner_join("560004", TransferRole::Sender),
    );
    assert!(matches!(
        second.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::IpRateLimited,
            ..
        })
    ));
    assert!(second_serve.await.unwrap().is_err());
    assert_eq!(ip_limited.metrics_snapshot().ip_rate_limit_rejections, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_and_unsupported_rooms_return_stable_outcomes() {
    let registry = Arc::new(
        RoomRegistry::with_config(BrokerConfig {
            room_ttl: Duration::from_millis(30),
            room_tombstone_ttl: Duration::from_secs(1),
            ..abuse_test_config()
        })
        .unwrap(),
    );
    let (creator, creator_serve) = start_peer(
        registry.clone(),
        creator_join("570001", TransferRole::Receiver),
    );
    assert_eq!(creator.await.unwrap(), Reply::Expired);
    assert!(matches!(
        creator_serve.await.unwrap(),
        Err(RendezvousError::Expired)
    ));
    let (reconnect, reconnect_serve) = start_peer(
        registry.clone(),
        creator_join("570001", TransferRole::Receiver),
    );
    assert!(matches!(
        reconnect.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::RoomExpired,
            ..
        })
    ));
    assert!(reconnect_serve.await.unwrap().is_err());

    let unsupported = Join {
        version: RENDEZVOUS_PROTOCOL_VERSION + 1,
        ..creator_join("570002", TransferRole::Receiver)
    };
    let (reply, serve) = start_peer(registry.clone(), unsupported);
    assert!(matches!(
        reply.await.unwrap(),
        Reply::Rejected(BrokerRejection {
            outcome: BrokerOutcome::UnsupportedVersion,
            ..
        })
    ));
    assert!(serve.await.unwrap().is_err());
    assert_eq!(registry.metrics_snapshot().unsupported_versions, 1);
}
