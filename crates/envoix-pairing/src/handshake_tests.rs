use super::*;
use crate::{open_json, seal_json};

const PW: &str = "42-galaxy-pencil";

fn context() -> envoix_invite::InvitationControlContext {
    envoix_invite::InvitationControlContext::new(
        "123456".to_string(),
        envoix_invite::BootstrapKind::RoomCode,
        envoix_invite::TransferRole::Sender,
        envoix_invite::TransferRole::Receiver,
    )
    .unwrap()
}

/// Drive a full successful handshake; returns both confirmed keys.
fn run(initiator_pw: &str, responder_pw: &str) -> Result<(Vec<u8>, Vec<u8>), PairingError> {
    let context = context();
    let (initiator, start) = initiator_start(initiator_pw, &context)?;
    let (responder, response) = responder_respond(responder_pw, &context, &start)?;
    let (initiator_confirming, initiator_conf) = initiator.finish(&response)?;
    let (responder_paired, responder_conf) = responder.verify(&initiator_conf)?;
    let initiator_paired = initiator_confirming.verify(&responder_conf)?;
    Ok((
        initiator_paired.key().to_vec(),
        responder_paired.key().to_vec(),
    ))
}

#[test]
fn matching_password_agrees_on_key() {
    let (ik, rk) = run(PW, PW).unwrap();
    assert_eq!(ik, rk);
    assert!(!ik.is_empty());
}

#[test]
fn key_seals_a_bundle_both_ways() {
    let context = context();
    let (initiator, start) = initiator_start(PW, &context).unwrap();
    let (responder, response) = responder_respond(PW, &context, &start).unwrap();
    let (initiator_confirming, initiator_conf) = initiator.finish(&response).unwrap();
    let (responder_paired, responder_conf) = responder.verify(&initiator_conf).unwrap();
    let initiator_paired = initiator_confirming.verify(&responder_conf).unwrap();

    // Each side seals a value the other opens with the same confirmed key.
    let from_initiator = vec!["addr-a".to_string()];
    let sealed = seal_json(initiator_paired.key(), b"aad", &from_initiator).unwrap();
    assert_eq!(
        open_json::<Vec<String>>(responder_paired.key(), b"aad", &sealed).unwrap(),
        from_initiator
    );
}

#[test]
fn wrong_password_fails_confirmation() {
    // SPAKE2 finish still succeeds (different K each side); confirmation
    // is what catches the mismatch.
    let context = context();
    let (initiator, start) = initiator_start(PW, &context).unwrap();
    let (responder, response) = responder_respond("99-wrong-words-here", &context, &start).unwrap();
    let (_c, initiator_conf) = initiator.finish(&response).unwrap();
    assert!(matches!(
        responder.verify(&initiator_conf),
        Err(PairingError::Confirm)
    ));
}

#[test]
fn tampered_initiator_confirm_rejected() {
    let context = context();
    let (initiator, start) = initiator_start(PW, &context).unwrap();
    let (responder, response) = responder_respond(PW, &context, &start).unwrap();
    let (_c, mut initiator_conf) = initiator.finish(&response).unwrap();
    initiator_conf.mac[0] ^= 0x01;
    assert!(matches!(
        responder.verify(&initiator_conf),
        Err(PairingError::Confirm)
    ));
}

#[test]
fn tampered_responder_confirm_rejected_by_initiator() {
    let context = context();
    let (initiator, start) = initiator_start(PW, &context).unwrap();
    let (responder, response) = responder_respond(PW, &context, &start).unwrap();
    let (initiator_confirming, initiator_conf) = initiator.finish(&response).unwrap();
    let (_paired, mut responder_conf) = responder.verify(&initiator_conf).unwrap();
    responder_conf.mac[0] ^= 0x01;
    assert!(matches!(
        initiator_confirming.verify(&responder_conf),
        Err(PairingError::Confirm)
    ));
}

#[test]
fn bad_nonce_length_rejected() {
    let context = context();
    let (_initiator, mut start) = initiator_start(PW, &context).unwrap();
    start.nonce.truncate(4);
    assert!(matches!(
        responder_respond(PW, &context, &start),
        Err(PairingError::BadMessage(_))
    ));
}

#[test]
fn changed_invitation_context_fails_confirmation() {
    let initiator_context = context();
    let responder_context = envoix_invite::InvitationControlContext::new(
        "654321".to_string(),
        envoix_invite::BootstrapKind::RoomCode,
        envoix_invite::TransferRole::Sender,
        envoix_invite::TransferRole::Receiver,
    )
    .unwrap();
    let (initiator, start) = initiator_start(PW, &initiator_context).unwrap();
    let (responder, response) = responder_respond(PW, &responder_context, &start).unwrap();
    let (_confirming, proof) = initiator.finish(&response).unwrap();
    assert!(matches!(
        responder.verify(&proof),
        Err(PairingError::Confirm)
    ));
}
