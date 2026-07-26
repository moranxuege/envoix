use std::sync::Arc;
use std::time::Duration;
use std::{io::ErrorKind, net::UdpSocket};

use envoix_error::CoreError;
use envoix_rendezvous::RoomRegistry;
use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
use iroh::{RelayMode, SecretKey};

use super::*;
use crate::{
    CandidateFilter, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig, SessionConfig, TransferCancelToken,
};

#[test]
fn room_invitation_has_a_distinct_uri_code_and_broker_namespace() {
    let invite = RoomControlInvite::from_parts(
        "R123456-amber-comet".into(),
        "peer@example:8445".into(),
        Some("https://relay.example".into()),
        u64::MAX,
    )
    .unwrap();
    assert!(invite.payload().starts_with("envoix://room/R123456-"));
    assert_eq!(invite.room_id(), "c1_123456");
    assert!(!invite.payload().starts_with("envoix://pair/"));
}

#[test]
fn invitation_round_trips_reserved_transport_characters() {
    let invite = RoomControlInvite::from_parts(
        "R123456-amber-comet".into(),
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
fn legacy_pairing_codes_cannot_enter_control_namespace() {
    assert!(RoomControlInvite::parse("123456-amber-comet", "broker", None).is_err());
}

#[test]
fn typed_room_code_is_case_insensitive_and_canonicalized() {
    let invite =
        RoomControlInvite::parse("r123456-AMBER-Comet", "broker", None).expect("typed room code");
    assert_eq!(invite.code(), "R123456-amber-comet");
}

#[test]
fn offer_requires_three_bounded_sanitized_roots_and_sender_invite() {
    let valid = offer("offer_1", "123456-amber-comet");
    assert!(valid.validate().is_ok());
    let mut invalid = valid.clone();
    invalid.root_names.push("../secret".into());
    assert!(invalid.validate().is_err());
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
    let broker_task = tokio::spawn(serve_endpoint(
        broker.clone(),
        Arc::new(RoomRegistry::new()),
        None,
    ));
    let invite =
        RoomControlInvite::from_parts("R123456-amber-comet".into(), broker_text, None, u64::MAX)
            .unwrap();
    let host_invite = invite.clone();
    let join_invite = invite;
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
    tokio::time::sleep(Duration::from_millis(25)).await;
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
    assert_eq!(host.lifetime_state(), joiner.lifetime_state());
    assert_eq!(host.lifetime_state().revision, 1);
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
        .offer_transfer(offer("from_host", "123456-amber-comet"))
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
            .offer_transfer(offer("from_joiner", "654321-river-slate"))
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

fn offer(offer_id: &str, _code: &str) -> RoomTransferOffer {
    let transfer_invite = InviteV2::create(
        "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
            .into(),
        Vec::new(),
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
        total_bytes: 42,
    }
}
