use std::sync::Arc;
use std::time::Duration;
use std::{io::ErrorKind, net::UdpSocket};

use envoix_auth::RememberedCredential;
use envoix_error::CoreError;
use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use iroh::{RelayMode, SecretKey};

use super::*;
use crate::{
    CandidateFilter, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig, RendezvousCause,
    RendezvousRetryPolicy, SessionConfig, TransferCancelToken,
};

#[test]
fn room_invitation_has_a_distinct_uri_code_and_broker_namespace() {
    let invite = RoomControlInvite::from_parts(
        "123456-a1b2-c3d4".into(),
        "peer@example:8445".into(),
        Some("https://relay.example".into()),
        u64::MAX,
    )
    .unwrap();
    assert!(invite.payload().starts_with("envoix://room/123456-"));
    assert_eq!(invite.room_id(), "c2_123456");
    assert!(!invite.payload().starts_with("envoix://pair/"));
}

#[test]
fn generated_room_invitation_uses_the_full_base36_secret() {
    let invite = RoomControlInvite::generate("peer@example:8445", None).unwrap();
    let code = invite.code().as_bytes();

    assert_eq!(code.len(), 16);
    assert_eq!(code[6], b'-');
    assert_eq!(code[11], b'-');
    assert!(code[..6].iter().all(u8::is_ascii_digit));
    assert!(
        code[7..11]
            .iter()
            .chain(&code[12..16])
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
    );
    assert_eq!(invite.room_id(), format!("c2_{}", &invite.code()[..6]));
    assert_eq!(
        invite
            .payload()
            .strip_prefix("envoix://room/")
            .expect("Room URI prefix")
            .split_once('?')
            .expect("Room URI query")
            .0,
        invite.code()
    );
    assert_eq!(
        RoomControlInvite::parse(&invite.payload(), "fallback", None).unwrap(),
        invite
    );
}

#[test]
fn invitation_round_trips_reserved_transport_characters() {
    let invite = RoomControlInvite::from_parts(
        "123456-a1b2-c3d4".into(),
        "id@[2001:db8::1]:8445".into(),
        Some("https://relay.example/path?a=b".into()),
        u64::MAX,
    )
    .unwrap();
    let decoded =
        RoomControlInvite::parse(&invite.payload(), "fallback", None).expect("parse invite");
    assert_eq!(decoded, invite);
}

#[test]
fn invitation_canonicalizes_transport_endpoint_whitespace() {
    let invite = RoomControlInvite::from_parts(
        "123456-a1b2-c3d4".into(),
        "  id@[2001:db8::1]:8445  ".into(),
        Some("  https://relay.example/path?a=b  ".into()),
        u64::MAX,
    )
    .unwrap();
    assert_eq!(invite.broker(), "id@[2001:db8::1]:8445");
    assert_eq!(invite.relay(), Some("https://relay.example/path?a=b"));

    let payload = invite.payload();
    assert!(payload.contains("broker=id%40%5B2001%3Adb8%3A%3A1%5D%3A8445"));
    assert!(payload.contains("relay=https%3A%2F%2Frelay.example%2Fpath%3Fa%3Db"));
    assert!(!payload.contains("%20"));
}

#[test]
fn legacy_prefixed_room_codes_are_rejected_in_text_and_uri() {
    for code in ["R123456-a1b2-c3d4", "r123456-a1b2-c3d4"] {
        assert!(matches!(
            RoomControlInvite::parse(code, "broker", None),
            Err(CoreError::InvalidInput(message))
                if message == "legacy R-prefixed room codes are not supported"
        ));
        assert!(matches!(
            RoomControlInvite::parse(
                &format!("envoix://room/{code}?broker=broker&expires={}", u64::MAX),
                "fallback",
                None,
            ),
            Err(CoreError::InvalidInput(message))
                if message == "legacy R-prefixed room codes are not supported"
        ));
    }

    assert!(matches!(
        RoomControlInvite::parse(
            &format!(
                "envoix://room/%52123456-a1b2-c3d4?broker=broker&expires={}",
                u64::MAX
            ),
            "fallback",
            None,
        ),
        Err(CoreError::InvalidInput(message))
            if message == "legacy R-prefixed room codes are not supported"
    ));
}

#[test]
fn typed_room_code_is_case_insensitive_and_canonicalized() {
    let invite =
        RoomControlInvite::parse("123456-A1B2-C3D4", "broker", None).expect("typed room code");
    assert_eq!(invite.code(), "123456-a1b2-c3d4");
}

#[test]
fn offer_requires_three_bounded_sanitized_roots_and_sender_invite() {
    let valid = offer("offer_1", TEST_BROKER, Vec::new());
    assert!(valid.validate(TEST_BROKER, None).is_ok());
    let mut invalid = valid.clone();
    invalid.root_names.push("../secret".into());
    assert!(invalid.validate(TEST_BROKER, None).is_err());
}

#[test]
fn offer_directory_count_cannot_exceed_item_count() {
    let mut invalid = offer("offer_1", TEST_BROKER, Vec::new());
    invalid.directory_count = invalid.item_count + 1;
    assert!(matches!(
        invalid.validate(TEST_BROKER, None),
        Err(CoreError::InvalidInput(message))
            if message == "room offer directory count exceeds item count"
    ));
}

#[test]
fn offer_route_must_exactly_match_room_control_route() {
    let no_relay = offer("offer_1", TEST_BROKER, Vec::new());
    assert!(no_relay.validate(TEST_BROKER, None).is_ok());
    assert!(matches!(
        no_relay.validate(OTHER_BROKER, None),
        Err(CoreError::InvalidInput(message))
            if message == "room offer transfer route differs from room control route"
    ));

    let relay = "https://relay.example.test".to_string();
    let one_relay = offer("offer_2", TEST_BROKER, vec![relay.clone()]);
    assert!(one_relay.validate(TEST_BROKER, Some(&relay)).is_ok());
    assert!(one_relay.validate(TEST_BROKER, None).is_err());

    let two_relays = offer(
        "offer_3",
        TEST_BROKER,
        vec![relay.clone(), "https://relay2.example.test".into()],
    );
    assert!(two_relays.validate(TEST_BROKER, Some(&relay)).is_err());
}

#[test]
fn room_control_protocol_identifiers_are_v5() {
    assert_eq!(ROOM_CONTROL_ALPN, b"envoix-room-control/5");
    assert_eq!(ROOM_CONTROL_VERSION, 5);
}

#[test]
fn remembered_hello_rejects_protocol_and_session_mode_mismatches() {
    let mode = RoomControlSessionMode::Remembered { generation: 7 };
    let binding = [0x42; 32];
    let hello = |protocol_version, session_kind, creator, lifetime| ControlMessage::Hello {
        protocol_version,
        session_kind,
        display_name: "Peer".into(),
        creator,
        pairing_binding: binding.to_vec(),
        lifetime,
    };

    assert!(matches!(
        validate_control_hello(
            hello(
                ROOM_CONTROL_VERSION + 1,
                ControlSessionKind::Remembered,
                false,
                None,
            ),
            mode,
            &binding,
            None,
        ),
        Err(CoreError::Protocol(message))
            if message == format!("unsupported room control version {}", ROOM_CONTROL_VERSION + 1)
    ));
    assert!(matches!(
        validate_control_hello(
            hello(
                ROOM_CONTROL_VERSION,
                ControlSessionKind::Invitation,
                false,
                None,
            ),
            mode,
            &binding,
            None,
        ),
        Err(CoreError::Protocol(message))
            if message == "room control peer selected a different session mode"
    ));
    assert!(matches!(
        validate_control_hello(
            hello(
                ROOM_CONTROL_VERSION,
                ControlSessionKind::Remembered,
                true,
                Some(RoomLifetimeState::initial(1)),
            ),
            mode,
            &binding,
            None,
        ),
        Err(CoreError::Protocol(message))
            if message
                == "remembered room peers cannot claim creator or lifetime ownership"
    ));

    let (_, lifetime) = validate_control_hello(
        hello(
            ROOM_CONTROL_VERSION,
            ControlSessionKind::Remembered,
            false,
            None,
        ),
        mode,
        &binding,
        None,
    )
    .expect("equal-member remembered hello");
    assert_eq!(lifetime, RoomLifetimeState::remembered());
}

#[tokio::test]
async fn exhausted_remembered_generation_is_reported_before_authentication() {
    let error = match connect_remembered_room_control(
        remembered_session(u64::MAX),
        TEST_BROKER.into(),
        None,
        "Peer".into(),
        RememberedRoomControlRole::Connector,
        test_config(),
        &TransferCancelToken::new(),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("an exhausted generation cannot reconnect"),
    };

    assert!(!error.peer_authenticated());
    assert!(matches!(
        error.error(),
        CoreError::InvalidInput(message)
            if message == "remembered credential generation is exhausted"
    ));
}

#[test]
fn remembered_room_control_forces_one_broker_and_pairing_attempt() {
    let policy = remembered_room_control_retry_policy(RendezvousRetryPolicy {
        pairing_attempts: 9,
        server_retries: 9,
        max_retry_after: Duration::from_secs(47),
    });

    assert_eq!(policy.pairing_attempts, 1);
    assert_eq!(policy.server_retries, 0);
    assert_eq!(policy.max_retry_after, Duration::from_secs(47));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_remembered_connector_makes_exactly_one_broker_join() {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipping remembered probe loopback: UDP bind denied ({error})");
            return;
        }
        Err(error) => panic!("remembered probe transport pre-check failed: {error}"),
    }
    let broker = build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    .expect("bind loopback broker");
    let broker_endpoint_addr = endpoint_addr(&broker);
    let broker_socket = *broker_endpoint_addr
        .ip_addrs()
        .next()
        .expect("loopback broker address");
    let broker_text = format!("{}@{broker_socket}", broker_endpoint_addr.id);
    let registry = Arc::new(RoomRegistry::new());
    let broker_task = tokio::spawn(serve_endpoint(broker.clone(), Arc::clone(&registry), None));

    let error = match connect_remembered_room_control(
        remembered_session(7),
        broker_text,
        None,
        "Connector".into(),
        RememberedRoomControlRole::Connector,
        test_config(),
        &TransferCancelToken::new(),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("a connector without a responder must fail"),
    };

    assert!(!error.peer_authenticated());
    assert!(matches!(
        error.error(),
        CoreError::Rendezvous {
            cause: RendezvousCause::RoomNotFound,
            ..
        }
    ));
    assert_eq!(
        registry.metrics_snapshot().room_not_found_rejections,
        1,
        "one logical remembered probe must be one broker admission"
    );

    broker.close().await;
    broker_task.abort();
}

#[tokio::test]
async fn oversized_control_frame_is_rejected_before_allocation() {
    let mut bytes = Vec::from(*b"ENRC");
    bytes.extend_from_slice(&ROOM_CONTROL_VERSION.to_be_bytes());
    bytes.extend_from_slice(&((MAX_CONTROL_FRAME_BYTES as u32) + 1).to_be_bytes());
    assert!(matches!(
        read_control_message(&mut bytes.as_slice()).await,
        Err(CoreError::Protocol(_))
    ));
}

#[tokio::test]
async fn stalled_control_phase_is_bounded_and_cancelable() {
    let cancel = TransferCancelToken::new();
    let timed_out = run_control_phase(
        std::future::pending::<Result<(), SessionError>>(),
        &cancel,
        Duration::from_millis(1),
        "test phase",
    )
    .await;
    assert!(matches!(
        timed_out,
        Err(CoreError::Transport(message)) if message == "test phase timed out"
    ));

    let cancel = TransferCancelToken::new();
    cancel.cancel();
    assert!(matches!(
        run_control_phase(
            std::future::pending::<Result<(), SessionError>>(),
            &cancel,
            Duration::from_secs(1),
            "test phase",
        )
        .await,
        Err(CoreError::Cancelled)
    ));
}

#[test]
fn failed_offer_response_terminates_the_session() {
    let terminated = std::sync::atomic::AtomicBool::new(false);
    let result = terminate_on_response_send_failure::<()>(
        Err(CoreError::Transport("peer disappeared".into())),
        || terminated.store(true, std::sync::atomic::Ordering::Release),
    );
    assert!(matches!(result, Err(CoreError::Transport(_))));
    assert!(terminated.load(std::sync::atomic::Ordering::Acquire));

    terminated.store(false, std::sync::atomic::Ordering::Release);
    assert!(
        terminate_on_response_send_failure(Ok(()), || {
            terminated.store(true, std::sync::atomic::Ordering::Release);
        })
        .is_ok()
    );
    assert!(!terminated.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn creator_lifetime_pauses_until_both_transfer_edges_clear() {
    let initial = RoomLifetimeState::initial(1_000);
    assert_eq!(initial.idle_deadline_unix_ms, Some(901_000));
    let mut lifetime = RoomLifetimeMachine::new(initial);

    let paused = lifetime
        .set_local_transfer_active(true, 2_000)
        .unwrap()
        .expect("first active edge pauses");
    assert_eq!(paused.revision, 2);
    assert_eq!(paused.idle_deadline_unix_ms, None);

    assert!(
        lifetime
            .set_peer_transfer_active(true, 3_000)
            .unwrap()
            .is_none()
    );
    assert!(
        lifetime
            .set_local_transfer_active(false, 4_000)
            .unwrap()
            .is_none()
    );
    assert_eq!(lifetime.state, paused);

    let resumed = lifetime
        .set_peer_transfer_active(false, 5_000)
        .unwrap()
        .expect("last inactive edge resumes");
    assert_eq!(resumed.revision, 3);
    assert_eq!(resumed.idle_deadline_unix_ms, Some(905_000));
}

#[test]
fn creator_activity_and_policy_transitions_stamp_one_authoritative_state() {
    let mut lifetime = RoomLifetimeMachine::new(RoomLifetimeState::initial(10));

    let active = lifetime.note_activity(20).unwrap().expect("activity");
    assert_eq!(active.revision, 2);
    assert_eq!(active.idle_deadline_unix_ms, Some(900_020));

    let kept_open = lifetime
        .set_policy(RoomLifetimePolicy::UntilForegroundEnds, 30)
        .unwrap()
        .expect("policy change");
    assert_eq!(kept_open.revision, 3);
    assert_eq!(kept_open.idle_deadline_unix_ms, None);
    assert!(lifetime.note_activity(40).unwrap().is_none());

    lifetime
        .set_peer_transfer_active(true, 50)
        .expect("track peer while kept open");
    let idle_while_active = lifetime
        .set_policy(RoomLifetimePolicy::Idle15Minutes, 60)
        .unwrap()
        .expect("restore idle policy");
    assert_eq!(idle_while_active.revision, 4);
    assert_eq!(idle_while_active.idle_deadline_unix_ms, None);

    let resumed = lifetime
        .set_peer_transfer_active(false, 70)
        .unwrap()
        .expect("last transfer ended");
    assert_eq!(resumed.revision, 5);
    assert_eq!(resumed.idle_deadline_unix_ms, Some(900_070));
}

#[test]
fn authoritative_lifetime_requires_a_strictly_new_revision() {
    let mut lifetime = RoomLifetimeMachine::new(RoomLifetimeState::initial(100));
    let stale = lifetime.state.clone();
    assert!(matches!(
        lifetime.apply_authoritative(stale),
        Err(CoreError::Protocol(_))
    ));

    let invalid = RoomLifetimeState {
        revision: 2,
        policy: RoomLifetimePolicy::UntilForegroundEnds,
        idle_deadline_unix_ms: Some(123),
    };
    assert!(matches!(
        lifetime.apply_authoritative(invalid),
        Err(CoreError::Protocol(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_control_loopback_supports_alternating_offers_and_close() {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipping room control loopback: UDP bind denied ({error})");
            return;
        }
        Err(error) => panic!("room control transport pre-check failed: {error}"),
    }
    let broker = match build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("bind loopback broker: {error:#}"),
    };
    let broker_endpoint_addr = endpoint_addr(&broker);
    let broker_socket = *broker_endpoint_addr
        .ip_addrs()
        .next()
        .expect("loopback broker address");
    let broker_text = format!("{}@{broker_socket}", broker_endpoint_addr.id);
    let registry = Arc::new(RoomRegistry::new());
    let broker_task = tokio::spawn(serve_endpoint(broker.clone(), Arc::clone(&registry), None));
    let invite = RoomControlInvite::from_parts(
        "123456-a1b2-c3d4".into(),
        broker_text.clone(),
        None,
        u64::MAX,
    )
    .unwrap();
    let host_invite = invite.clone();
    let join_invite = invite;
    // A scanner can reach the broker before the creator has finished opening
    // its room. Exercise that real device ordering instead of requiring the UI
    // to coordinate two independent network operations.
    let joiner = tokio::spawn(async move {
        connect_room_control(
            join_invite,
            "Bob's Android".into(),
            false,
            test_config(),
            &TransferCancelToken::new(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while registry.metrics_snapshot().room_not_found_rejections == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("joiner should reach the broker before creator starts");
    let host = tokio::spawn(async move {
        connect_room_control(
            host_invite,
            "Alice's iPhone".into(),
            true,
            test_config(),
            &TransferCancelToken::new(),
        )
        .await
    });
    let (host, joiner) = tokio::time::timeout(Duration::from_secs(45), async {
        let (host, joiner) = tokio::join!(host, joiner);
        let host = host.expect("host task");
        let joiner = joiner.expect("joiner task");
        if let Err(error) = &host {
            eprintln!("host room-control error: {error}");
        }
        if let Err(error) = &joiner {
            eprintln!("joiner room-control error: {error}");
        }
        (
            host.expect("host connection"),
            joiner.expect("joiner connection"),
        )
    })
    .await
    .expect("room connections timed out");
    assert_eq!(host.peer_name(), "Bob's Android");
    assert_eq!(joiner.peer_name(), "Alice's iPhone");
    assert!(host.is_creator());
    assert!(!joiner.is_creator());
    assert!(
        registry.metrics_snapshot().room_not_found_rejections >= 1,
        "joiner-first connection should exercise the broker retry path"
    );
    assert_eq!(host.lifetime_state(), joiner.lifetime_state());
    assert_eq!(host.lifetime_state().revision, 1);

    let local_route_error = host
        .offer_transfer(offer("wrong_local_route", OTHER_BROKER, Vec::new()))
        .await
        .expect_err("local room offer must keep the control route");
    assert!(matches!(
        local_route_error,
        CoreError::InvalidInput(message)
            if message == "room offer transfer route differs from room control route"
    ));

    joiner
        .send(ControlMessage::TransferOffer(offer(
            "wrong_peer_route",
            OTHER_BROKER,
            Vec::new(),
        )))
        .await
        .expect("send malicious peer offer");
    let incoming_route_error = host
        .next_event()
        .await
        .expect_err("incoming room offer must keep the control route");
    assert!(matches!(
        incoming_route_error,
        CoreError::InvalidInput(message)
            if message == "room offer transfer route differs from room control route"
    ));
    assert!(matches!(
        joiner.receive().await.unwrap(),
        ControlMessage::OfferRejected {
            offer_id,
            reason: RoomOfferRejection::Invalid,
        } if offer_id == "wrong_peer_route"
    ));

    assert!(
        joiner
            .set_policy(RoomLifetimePolicy::UntilForegroundEnds)
            .await
            .is_err()
    );
    assert!(joiner.close(RoomCloseReason::IdleExpired).await.is_err());
    assert!(host.close(RoomCloseReason::IdleExpired).await.is_err());

    let paused = host
        .set_local_transfer_active(true)
        .await
        .unwrap()
        .expect("creator active edge");
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(paused.clone())
    );
    assert_eq!(paused.idle_deadline_unix_ms, None);
    let resumed = host
        .set_local_transfer_active(false)
        .await
        .unwrap()
        .expect("creator inactive edge");
    assert_eq!(resumed.revision, paused.revision + 1);
    assert!(resumed.idle_deadline_unix_ms.is_some());
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(resumed)
    );

    assert!(
        joiner
            .set_local_transfer_active(true)
            .await
            .unwrap()
            .is_none()
    );
    let peer_paused = match host.next_event().await.unwrap() {
        RoomControlEvent::LifetimeChanged(state) => state,
        other => panic!("expected peer-paused lifetime, got {other:?}"),
    };
    assert_eq!(peer_paused.idle_deadline_unix_ms, None);
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(peer_paused)
    );

    assert!(
        joiner
            .set_local_transfer_active(false)
            .await
            .unwrap()
            .is_none()
    );
    let peer_resumed = match host.next_event().await.unwrap() {
        RoomControlEvent::LifetimeChanged(state) => state,
        other => panic!("expected resumed lifetime, got {other:?}"),
    };
    assert!(peer_resumed.idle_deadline_unix_ms.is_some());
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(peer_resumed)
    );

    let from_host_lifetime = host
        .offer_transfer(offer("from_host", &broker_text, Vec::new()))
        .await
        .unwrap()
        .expect("creator offer activity");
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(from_host_lifetime)
    );
    assert!(matches!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_host"
    ));
    assert!(joiner.accept_offer("from_host").await.unwrap().is_none());
    let accepted_lifetime = match host.next_event().await.unwrap() {
        RoomControlEvent::LifetimeChanged(state) => state,
        other => panic!("expected accepted lifetime, got {other:?}"),
    };
    assert_eq!(
        host.next_event().await.unwrap(),
        RoomControlEvent::OfferAccepted {
            offer_id: "from_host".into()
        }
    );
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(accepted_lifetime)
    );

    assert!(
        joiner
            .offer_transfer(offer("from_joiner", &broker_text, Vec::new()))
            .await
            .unwrap()
            .is_none()
    );
    let from_joiner_lifetime = match host.next_event().await.unwrap() {
        RoomControlEvent::LifetimeChanged(state) => state,
        other => panic!("expected joiner-offer lifetime, got {other:?}"),
    };
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(from_joiner_lifetime)
    );
    assert!(matches!(
        host.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_joiner"
    ));
    let rejected_lifetime = host
        .reject_offer("from_joiner", RoomOfferRejection::Declined)
        .await
        .unwrap()
        .expect("creator rejection activity");
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(rejected_lifetime)
    );
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::OfferRejected {
            offer_id: "from_joiner".into(),
            reason: RoomOfferRejection::Declined,
        }
    );

    let kept_open = host
        .set_policy(RoomLifetimePolicy::UntilForegroundEnds)
        .await
        .unwrap()
        .expect("creator policy change");
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::LifetimeChanged(kept_open)
    );
    host.close(RoomCloseReason::UserEnded).await.unwrap();
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::PeerClosed(RoomCloseReason::UserEnded)
    );

    broker.close().await;
    broker_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remembered_room_control_is_equal_and_bidirectional_after_authentication() {
    match UdpSocket::bind(("127.0.0.1", 0)) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipping remembered room control loopback: UDP bind denied ({error})");
            return;
        }
        Err(error) => panic!("remembered room control transport pre-check failed: {error}"),
    }
    let broker = match build_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::generate(),
        RelayMode::Disabled,
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("bind loopback broker: {error:#}"),
    };
    let broker_endpoint_addr = endpoint_addr(&broker);
    let broker_socket = *broker_endpoint_addr
        .ip_addrs()
        .next()
        .expect("loopback broker address");
    let broker_text = format!("{}@{broker_socket}", broker_endpoint_addr.id);
    let registry = Arc::new(RoomRegistry::new());
    let broker_task = tokio::spawn(serve_endpoint(broker.clone(), Arc::clone(&registry), None));

    let responder_broker = broker_text.clone();
    let responder = tokio::spawn(async move {
        connect_remembered_room_control(
            remembered_session(7),
            responder_broker,
            None,
            "Alice's iPhone".into(),
            RememberedRoomControlRole::Responder,
            test_config(),
            &TransferCancelToken::new(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while registry.metrics_snapshot().waiting_creators == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("remembered responder should be waiting");
    let connector_broker = broker_text.clone();
    let connector = tokio::spawn(async move {
        connect_remembered_room_control(
            remembered_session(7),
            connector_broker,
            None,
            "Bob's Android".into(),
            RememberedRoomControlRole::Connector,
            test_config(),
            &TransferCancelToken::new(),
        )
        .await
    });
    let (responder, connector) = tokio::time::timeout(Duration::from_secs(45), async {
        let (responder, connector) = tokio::join!(responder, connector);
        (
            responder
                .expect("responder task")
                .expect("responder session"),
            connector
                .expect("connector task")
                .expect("connector session"),
        )
    })
    .await
    .expect("remembered room connections timed out");

    assert_eq!(responder.peer_name(), "Bob's Android");
    assert_eq!(connector.peer_name(), "Alice's iPhone");
    assert!(!responder.is_creator());
    assert!(!connector.is_creator());
    assert!(responder.is_remembered());
    assert!(connector.is_remembered());
    assert_eq!(responder.remembered_generation(), Some(7));
    assert_eq!(connector.remembered_generation(), Some(7));
    assert_eq!(responder.lifetime_state(), RoomLifetimeState::remembered());
    assert_eq!(responder.lifetime_state(), connector.lifetime_state());
    assert!(
        responder
            .set_policy(RoomLifetimePolicy::Idle15Minutes)
            .await
            .is_err()
    );
    assert!(
        connector
            .set_policy(RoomLifetimePolicy::Idle15Minutes)
            .await
            .is_err()
    );

    assert!(
        connector
            .offer_transfer(offer("from_connector", &broker_text, Vec::new()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        responder.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_connector"
    ));
    assert!(
        responder
            .accept_offer("from_connector")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        connector.next_event().await.unwrap(),
        RoomControlEvent::OfferAccepted {
            offer_id: "from_connector".into(),
        }
    );

    assert!(
        responder
            .offer_transfer(offer("from_responder", &broker_text, Vec::new()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        connector.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_responder"
    ));
    assert!(
        connector
            .reject_offer("from_responder", RoomOfferRejection::Declined)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        responder.next_event().await.unwrap(),
        RoomControlEvent::OfferRejected {
            offer_id: "from_responder".into(),
            reason: RoomOfferRejection::Declined,
        }
    );

    responder.close(RoomCloseReason::UserEnded).await.unwrap();
    assert_eq!(
        connector.next_event().await.unwrap(),
        RoomControlEvent::PeerClosed(RoomCloseReason::UserEnded)
    );

    broker.close().await;
    broker_task.abort();
}

fn test_config() -> SessionConfig {
    SessionConfig {
        identity: IdentityConfig::Ephemeral,
        relay: None,
        relay_only: false,
        direct_only: false,
        candidates: CandidateFilter::default(),
        data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
        rendezvous_retry: crate::RendezvousRetryPolicy::default(),
    }
}

fn remembered_session(generation: u64) -> RememberedSession {
    let mut opaque = b"ENVR".to_vec();
    opaque.push(1);
    opaque.extend_from_slice(&[0x5a; 32]);
    RememberedCredential::from_opaque(&opaque)
        .expect("test remembered credential")
        .derive_session(generation)
}

const TEST_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
const OTHER_BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8555";

fn offer(offer_id: &str, broker: &str, relay_urls: Vec<String>) -> RoomTransferOffer {
    let transfer_invite = InviteV2::create(
        broker.into(),
        relay_urls,
        TransferRole::Sender,
        envoix_invite::Capabilities::current(),
        now_unix_secs().expect("current time"),
    )
    .expect("sender invitation")
    .payload;
    RoomTransferOffer {
        offer_id: offer_id.into(),
        transfer_invite,
        root_names: vec!["Photos".into(), "report.pdf".into()],
        item_count: 2,
        directory_count: 1,
        total_bytes: 42,
    }
}
