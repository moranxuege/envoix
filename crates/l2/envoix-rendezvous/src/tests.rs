use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use crate::{
    ClientConfig, ConfigError, ControlError, ControlFrame, ControlLimits, Join, NamespacedRoomKey,
    Paired, PeerConn, RegistryConfig, RejectionReason, RendezvousError, Reply, Role, RoomRegistry,
    decode_control, encode_control, join_room, read_control, write_control,
};

const KEY_A: &str = "v2:123456";
const KEY_B: &str = "v2:654321";

fn limits(maximum: usize) -> ControlLimits {
    ControlLimits::new(maximum).unwrap()
}

fn registry_config(
    room_ttl: Duration,
    maximum_waiters: usize,
    maximum_key: usize,
) -> RegistryConfig {
    RegistryConfig::new(
        room_ttl,
        Duration::from_secs(5),
        Duration::from_secs(1),
        Duration::from_secs(1),
        limits(maximum_key),
        maximum_waiters,
    )
    .unwrap()
}

fn client_config(maximum_key: usize) -> ClientConfig {
    ClientConfig::new(Duration::from_secs(2), limits(maximum_key)).unwrap()
}

fn key(value: &str) -> NamespacedRoomKey {
    NamespacedRoomKey::parse(value).unwrap()
}

fn broker_conn(stream: DuplexStream) -> PeerConn {
    let (reader, writer) = tokio::io::split(stream);
    PeerConn::new(writer, reader, ())
}

async fn assert_wire_fixtures() {
    let limits = limits(32);
    let fixtures = [
        (
            ControlFrame::Join(Join::new(key(KEY_A))),
            b"ENVR\x00\x02\x01\x00\x00\x00\x00\x09v2:123456".as_slice(),
        ),
        (
            ControlFrame::Reply(Reply::Paired(Paired {
                role: Role::Initiator,
            })),
            b"ENVR\x00\x02\x02\x00\x00\x00\x00\x01\x00".as_slice(),
        ),
        (
            ControlFrame::Reply(Reply::Paired(Paired {
                role: Role::Responder,
            })),
            b"ENVR\x00\x02\x02\x00\x00\x00\x00\x01\x01".as_slice(),
        ),
        (
            ControlFrame::Reply(Reply::Expired),
            b"ENVR\x00\x02\x03\x00\x00\x00\x00\x00".as_slice(),
        ),
        (
            ControlFrame::Reply(Reply::Rejected(RejectionReason::WaitingRoomsFull)),
            b"ENVR\x00\x02\x04\x00\x00\x00\x00\x01\x03".as_slice(),
        ),
    ];
    for (frame, expected) in fixtures {
        assert_eq!(encode_control(&frame, limits).unwrap(), expected);
        assert_eq!(decode_control(expected, limits).unwrap(), frame);

        let mut stream = tokio::io::duplex(128);
        write_control(&mut stream.0, &frame, limits).await.unwrap();
        assert_eq!(read_control(&mut stream.1, limits).await.unwrap(), frame);
    }

    let join = encode_control(&ControlFrame::Join(Join::new(key(KEY_A))), limits).unwrap();
    // Every possible truncation point is rejected, not just the two edges.
    for cut in 0..join.len() {
        assert!(
            decode_control(&join[..cut], limits).is_err(),
            "truncation at {cut} must be rejected"
        );
    }

    let mut malformed = join.clone();
    malformed[0] = b'X';
    assert_eq!(
        decode_control(&malformed, limits),
        Err(ControlError::WrongMagic)
    );
    let mut malformed = join.clone();
    malformed[5] = 1;
    assert_eq!(
        decode_control(&malformed, limits),
        Err(ControlError::UnsupportedVersion)
    );
    let mut malformed = join.clone();
    malformed[6] = 99;
    assert_eq!(
        decode_control(&malformed, limits),
        Err(ControlError::UnknownKind)
    );
    let mut oversized = join[..12].to_vec();
    oversized[8..12].copy_from_slice(&33_u32.to_be_bytes());
    assert_eq!(
        decode_control(&oversized, limits),
        Err(ControlError::FrameTooLarge)
    );
    let mut trailing = join.clone();
    trailing.push(0);
    assert_eq!(
        decode_control(&trailing, limits),
        Err(ControlError::TrailingBytes)
    );

    let leaked_secret = b"ENVR\x00\x02\x01\x00\x00\x00\x00\x15v2:123456-amber-comet";
    assert_eq!(
        decode_control(leaked_secret, limits),
        Err(ControlError::InvalidRoomKey)
    );
}

async fn assert_blind_pairing_and_roles() {
    let registry = Arc::new(RoomRegistry::new(registry_config(
        Duration::from_secs(5),
        16,
        32,
    )));
    let (client_a, broker_a) = tokio::io::duplex(4096);
    let (client_b, broker_b) = tokio::io::duplex(4096);
    let registry_a = registry.clone();
    let serve_a = tokio::spawn(async move { registry_a.serve(broker_conn(broker_a)).await });
    let registry_b = registry.clone();
    let serve_b = tokio::spawn(async move { registry_b.serve(broker_conn(broker_b)).await });

    let (mut a_reader, mut a_writer) = tokio::io::split(client_a);
    write_control(
        &mut a_writer,
        &ControlFrame::Join(Join::new(key(KEY_A))),
        limits(32),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (mut b_reader, mut b_writer) = tokio::io::split(client_b);
    let role_b = join_room(&mut b_reader, &mut b_writer, key(KEY_A), client_config(32))
        .await
        .unwrap();
    let role_a = match read_control(&mut a_reader, limits(32)).await.unwrap() {
        ControlFrame::Reply(Reply::Paired(paired)) => paired.role,
        other => panic!("expected paired reply, got {other:?}"),
    };
    assert_eq!(role_a, Role::Initiator);
    assert_eq!(role_b, Role::Responder);

    let from_a = b"\x00opaque-a\xff";
    let from_b = b"\xfeopaque-b\x01";
    a_writer.write_all(from_a).await.unwrap();
    b_writer.write_all(from_b).await.unwrap();
    let mut at_a = vec![0; from_b.len()];
    let mut at_b = vec![0; from_a.len()];
    tokio::try_join!(
        a_reader.read_exact(&mut at_a),
        b_reader.read_exact(&mut at_b)
    )
    .unwrap();
    assert_eq!(at_a, from_b);
    assert_eq!(at_b, from_a);

    a_writer.shutdown().await.unwrap();
    b_writer.shutdown().await.unwrap();
    drop(a_reader);
    drop(b_reader);
    serve_a.await.unwrap().unwrap();
    serve_b.await.unwrap().unwrap();
}

async fn assert_silent_connection_reclaimed_by_join_deadline() {
    // A connection that never sends its join is in no room, so only the join
    // deadline can reclaim it — it must not pin a slot indefinitely.
    let registry = Arc::new(RoomRegistry::new(
        RegistryConfig::new(
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_millis(40),
            Duration::from_millis(40),
            limits(32),
            4,
        )
        .unwrap(),
    ));
    let (client, broker) = tokio::io::duplex(1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    assert_eq!(
        serve.await.unwrap(),
        Err(RendezvousError::Rejected(RejectionReason::JoinDeadline))
    );
    drop(client);
}

async fn assert_lone_peer_expiry() {
    let registry = Arc::new(RoomRegistry::new(registry_config(
        Duration::from_millis(40),
        4,
        32,
    )));
    let (client, broker) = tokio::io::duplex(1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let (mut reader, mut writer) = tokio::io::split(client);
    let result = join_room(&mut reader, &mut writer, key(KEY_A), client_config(32)).await;
    assert_eq!(result, Err(RendezvousError::Expired));
    assert_eq!(serve.await.unwrap(), Err(RendezvousError::Expired));
}

async fn assert_overlong_key_rejected_before_allocation() {
    let registry = Arc::new(RoomRegistry::new(registry_config(
        Duration::from_secs(1),
        4,
        9,
    )));
    let (mut client, broker) = tokio::io::duplex(1024);
    let serve = tokio::spawn(async move { registry.serve(broker_conn(broker)).await });
    let mut header = b"ENVR\x00\x02\x01\x00\x00\x00\x00\x0a".to_vec();
    header.extend_from_slice(b"0123456789");
    client.write_all(&header).await.unwrap();
    assert_eq!(
        serve.await.unwrap(),
        Err(RendezvousError::Control(ControlError::FrameTooLarge))
    );
}

async fn assert_dead_waiter_and_silent_waiter_eviction() {
    let registry = Arc::new(RoomRegistry::new(registry_config(
        Duration::from_secs(5),
        8,
        32,
    )));

    let (dead_client, dead_broker) = tokio::io::duplex(1024);
    let dead_registry = registry.clone();
    let dead_serve =
        tokio::spawn(async move { dead_registry.serve(broker_conn(dead_broker)).await });
    let (reader, mut writer) = tokio::io::split(dead_client);
    write_control(
        &mut writer,
        &ControlFrame::Join(Join::new(key(KEY_A))),
        limits(32),
    )
    .await
    .unwrap();
    drop(reader);
    drop(writer);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), dead_serve)
            .await
            .expect("dead waiter must be evicted")
            .unwrap(),
        Err(RendezvousError::PeerClosed)
    );

    let (silent_client, silent_broker) = tokio::io::duplex(1024);
    let silent_registry = registry.clone();
    let silent_serve =
        tokio::spawn(async move { silent_registry.serve(broker_conn(silent_broker)).await });
    let (mut silent_reader, mut silent_writer) = tokio::io::split(silent_client);
    write_control(
        &mut silent_writer,
        &ControlFrame::Join(Join::new(key(KEY_B))),
        limits(32),
    )
    .await
    .unwrap();
    silent_writer.write_all(b"x").await.unwrap();
    assert_eq!(
        silent_serve.await.unwrap(),
        Err(RendezvousError::Rejected(RejectionReason::PeerNotSilent))
    );
    assert_eq!(
        read_control(&mut silent_reader, limits(32)).await.unwrap(),
        ControlFrame::Reply(Reply::Rejected(RejectionReason::PeerNotSilent))
    );

    assert_blind_pairing_and_roles_with_registry(registry, KEY_A).await;
}

async fn assert_blind_pairing_and_roles_with_registry(
    registry: Arc<RoomRegistry>,
    room: &'static str,
) {
    let (client_a, broker_a) = tokio::io::duplex(2048);
    let (client_b, broker_b) = tokio::io::duplex(2048);
    let registry_a = registry.clone();
    let serve_a = tokio::spawn(async move { registry_a.serve(broker_conn(broker_a)).await });
    let serve_b = tokio::spawn(async move { registry.serve(broker_conn(broker_b)).await });
    let (mut a_reader, mut a_writer) = tokio::io::split(client_a);
    let (mut b_reader, mut b_writer) = tokio::io::split(client_b);
    let a = join_room(&mut a_reader, &mut a_writer, key(room), client_config(32));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let b = join_room(&mut b_reader, &mut b_writer, key(room), client_config(32));
    let (a_role, b_role) = tokio::join!(a, b);
    assert_ne!(a_role.unwrap(), b_role.unwrap());
    a_writer.shutdown().await.unwrap();
    b_writer.shutdown().await.unwrap();
    drop(a_reader);
    drop(b_reader);
    serve_a.await.unwrap().unwrap();
    serve_b.await.unwrap().unwrap();
}

async fn assert_waiting_room_cap() {
    let registry = Arc::new(RoomRegistry::new(registry_config(
        Duration::from_secs(5),
        1,
        32,
    )));
    let (client_a, broker_a) = tokio::io::duplex(1024);
    let registry_a = registry.clone();
    let serve_a = tokio::spawn(async move { registry_a.serve(broker_conn(broker_a)).await });
    let (reader_a, mut writer_a) = tokio::io::split(client_a);
    write_control(
        &mut writer_a,
        &ControlFrame::Join(Join::new(key(KEY_A))),
        limits(32),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (client_b, broker_b) = tokio::io::duplex(1024);
    let serve_b = tokio::spawn(async move { registry.serve(broker_conn(broker_b)).await });
    let (mut reader_b, mut writer_b) = tokio::io::split(client_b);
    assert_eq!(
        join_room(&mut reader_b, &mut writer_b, key(KEY_B), client_config(32)).await,
        Err(RendezvousError::Rejected(RejectionReason::WaitingRoomsFull))
    );
    assert_eq!(
        serve_b.await.unwrap(),
        Err(RendezvousError::Rejected(RejectionReason::WaitingRoomsFull))
    );
    drop(reader_a);
    drop(writer_a);
    assert_eq!(serve_a.await.unwrap(), Err(RendezvousError::PeerClosed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rendezvous_v1_conformance() {
    assert_wire_fixtures().await;
    assert_blind_pairing_and_roles().await;
    assert_silent_connection_reclaimed_by_join_deadline().await;
    assert_lone_peer_expiry().await;
    assert_overlong_key_rejected_before_allocation().await;
    assert_dead_waiter_and_silent_waiter_eviction().await;
    assert_waiting_room_cap().await;
}

#[test]
fn rendezvous_config_rejects_zero_policy() {
    assert!(matches!(
        ControlLimits::new(0),
        Err(ConfigError::ZeroLimit { .. })
    ));
    assert!(matches!(
        ClientConfig::new(Duration::ZERO, limits(9)),
        Err(ConfigError::ZeroDuration { .. })
    ));
    assert!(matches!(
        RegistryConfig::new(
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            limits(9),
            1,
        ),
        Err(ConfigError::ZeroDuration { .. })
    ));
}
