//! End-to-end: two peers join a room through the broker and run the full
//! `envoix-pairing` exchange (SPAKE2 + sealed descriptors) over the broker's
//! blind relay. Uses in-memory duplexes - no sockets, no iroh.

use std::sync::Arc;
use std::time::Duration;

use envoix_pairing::{
    Confirm, PakeResponse, PakeStart, initiator_start, open_json, responder_respond, seal_json,
};
use envoix_rendezvous::{
    Join, JoinIntent, PeerConn, Reply, Role, RoomRegistry, read_framed, write_framed,
};
use tokio::io::DuplexStream;

/// Wrap the broker's side of a duplex as a `PeerConn` (the halves own the
/// stream, so no separate keep-alive is needed).
fn broker_conn(stream: DuplexStream) -> PeerConn {
    let (reader, writer) = tokio::io::split(stream);
    PeerConn::new(writer, reader, ())
}

/// Drive the initiator client over `stream`; returns the role the broker
/// assigned and the peer descriptor recovered from the other side.
async fn run_initiator(
    stream: DuplexStream,
    room: &str,
    code: &str,
    my_descriptor: &str,
    intent: Option<JoinIntent>,
) -> Result<(Role, String), Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(
        &mut writer,
        &Join {
            room_id: room.to_string(),
            intent,
        },
    )
    .await?;
    let reply: Reply = read_framed(&mut reader).await?;
    let Reply::Paired(paired) = reply else {
        panic!("expected Paired, got {reply:?}");
    };

    let (pending, start) = initiator_start(code)?;
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
    intent: Option<JoinIntent>,
) -> Result<(Role, String), Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(
        &mut writer,
        &Join {
            room_id: room.to_string(),
            intent,
        },
    )
    .await?;
    let reply: Reply = read_framed(&mut reader).await?;
    let Reply::Paired(paired) = reply else {
        panic!("expected Paired, got {reply:?}");
    };

    let start: PakeStart = read_framed(&mut reader).await?;
    let (confirming, response) = responder_respond(code, &start)?;
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
    room: &str,
    intent: Option<JoinIntent>,
) -> Result<Reply, Box<dyn std::error::Error + Send + Sync>> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_framed(
        &mut writer,
        &Join {
            room_id: room.to_string(),
            intent,
        },
    )
    .await?;
    Ok(read_framed(&mut reader).await?)
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

    // First joiner becomes the initiator. Give A a small head start so the
    // role assignment is deterministic.
    let a = tokio::spawn(async move {
        run_initiator(
            client_a,
            "room-42",
            "12-orange-tiger",
            "endpoint-A",
            Some(JoinIntent::Send),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let b = tokio::spawn(async move {
        run_responder(
            client_b,
            "room-42",
            "12-orange-tiger",
            "endpoint-B",
            Some(JoinIntent::Receive),
        )
        .await
    });

    let (role_a, a_got) = a.await.unwrap().expect("initiator pairs");
    let (role_b, b_got) = b.await.unwrap().expect("responder pairs");

    assert_eq!(role_a, Role::Initiator);
    assert_eq!(role_b, Role::Responder);
    // Each recovered the OTHER peer's descriptor, sealed under the shared key.
    assert_eq!(a_got, "endpoint-B");
    assert_eq!(b_got, "endpoint-A");

    s1.await.unwrap().expect("broker serves A");
    s2.await.unwrap().expect("broker serves B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lone_peer_expires() {
    let registry = Arc::new(RoomRegistry::with_ttl(Duration::from_millis(200)));
    let (mut client, broker) = tokio::io::duplex(4096);

    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });

    // Join a room nobody else joins.
    let (mut reader, mut writer) = tokio::io::split(&mut client);
    write_framed(
        &mut writer,
        &Join {
            room_id: "empty".to_string(),
            intent: Some(JoinIntent::Receive),
        },
    )
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

    let first =
        tokio::spawn(
            async move { join_only(sender_a, "senders-only", Some(JoinIntent::Send)).await },
        );
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second =
        tokio::spawn(
            async move { join_only(sender_b, "senders-only", Some(JoinIntent::Send)).await },
        );

    assert_eq!(first.await.unwrap().unwrap(), Reply::Expired);
    assert_eq!(second.await.unwrap().unwrap(), Reply::Expired);
    assert!(matches!(
        serve_a.await.unwrap(),
        Err(envoix_rendezvous::RendezvousError::Expired)
    ));
    assert!(matches!(
        serve_b.await.unwrap(),
        Err(envoix_rendezvous::RendezvousError::Expired)
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
            intent: Some(JoinIntent::Send),
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
        write_framed(
            &mut writer,
            &Join {
                room_id: "room-dead".to_string(),
                intent: Some(JoinIntent::Send),
            },
        )
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
        Err(envoix_rendezvous::RendezvousError::Rejected(_))
    ));

    // B and C join the same room and must pair with each other, not the corpse.
    let (client_b, broker_b) = tokio::io::duplex(64 * 1024);
    let (client_c, broker_c) = tokio::io::duplex(64 * 1024);
    let rb = registry.clone();
    let sb = tokio::spawn(async move { rb.serve(broker_conn(broker_b)).await });
    let rc = registry.clone();
    let sc = tokio::spawn(async move { rc.serve(broker_conn(broker_c)).await });

    let b = tokio::spawn(async move {
        run_initiator(
            client_b,
            "room-dead",
            "12-kelp-coral",
            "endpoint-B",
            Some(JoinIntent::Send),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let c = tokio::spawn(async move {
        run_responder(
            client_c,
            "room-dead",
            "12-kelp-coral",
            "endpoint-C",
            Some(JoinIntent::Receive),
        )
        .await
    });

    let (role_b, b_got) = b.await.unwrap().expect("B pairs despite the corpse");
    let (role_c, c_got) = c.await.unwrap().expect("C pairs despite the corpse");
    assert_eq!(role_b, Role::Initiator);
    assert_eq!(role_c, Role::Responder);
    assert_eq!(b_got, "endpoint-C");
    assert_eq!(c_got, "endpoint-B");

    sb.await.unwrap().expect("broker serves B");
    sc.await.unwrap().expect("broker serves C");
}
