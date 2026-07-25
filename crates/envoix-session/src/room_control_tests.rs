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
    assert_eq!(invite.room_id(), "control-v1:123456");
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

    host.offer_transfer(offer("from_host", "123456-amber-comet"))
        .await
        .unwrap();
    assert!(matches!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_host"
    ));
    joiner.accept_offer("from_host").await.unwrap();
    assert_eq!(
        host.next_event().await.unwrap(),
        RoomControlEvent::OfferAccepted {
            offer_id: "from_host".into()
        }
    );

    joiner
        .offer_transfer(offer("from_joiner", "654321-river-slate"))
        .await
        .unwrap();
    assert!(matches!(
        host.next_event().await.unwrap(),
        RoomControlEvent::IncomingOffer(RoomTransferOffer { offer_id, .. })
            if offer_id == "from_joiner"
    ));
    host.reject_offer("from_joiner", RoomOfferRejection::Declined)
        .await
        .unwrap();
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::OfferRejected {
            offer_id: "from_joiner".into(),
            reason: RoomOfferRejection::Declined,
        }
    );

    host.set_policy(RoomLifetimePolicy::UntilForegroundEnds)
        .await
        .unwrap();
    assert_eq!(
        joiner.next_event().await.unwrap(),
        RoomControlEvent::PolicyChanged(RoomLifetimePolicy::UntilForegroundEnds)
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
    }
}

fn offer(offer_id: &str, code: &str) -> RoomTransferOffer {
    RoomTransferOffer {
        offer_id: offer_id.into(),
        transfer_invite: format!("envoix://pair/{code}?role=send"),
        root_names: vec!["Photos".into(), "report.pdf".into()],
        item_count: 2,
        total_bytes: 42,
    }
}
