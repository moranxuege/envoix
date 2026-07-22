use envoix_outcomes::OutcomeCode;
use envoix_types::Secret;

use crate::bundle::{open_sealed, seal_with_nonce};
use crate::handshake::{KeySchedule, transcript};
use crate::identifiers::{INITIATOR_CONFIRM_LABEL, INITIATOR_SEAL_AAD};
use crate::message::{
    MAX_MESSAGE_BODY, MAX_SEALED_CIPHERTEXT_SIZE, PairingMessage, PakeResponse, PakeStart,
    SealedDescriptor, decode_message,
};
use crate::{
    DescriptorPayload, EntropyError, EntropySource, Paired, PairingCode, PairingError,
    decode_message as decode_wire_message, initiator_start, responder_respond,
};

struct FixedEntropy {
    byte: u8,
    calls: u8,
}

impl FixedEntropy {
    const fn new(byte: u8) -> Self {
        Self { byte, calls: 0 }
    }
}

impl EntropySource for FixedEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        for (index, byte) in destination.iter_mut().enumerate() {
            *byte = self.byte.wrapping_add(self.calls).wrapping_add(index as u8);
        }
        self.calls = self.calls.wrapping_add(1);
        Ok(())
    }
}

fn complete_pair(code: &str) -> Result<(Paired, Paired), PairingError> {
    let initiator_code = PairingCode::new(code.as_bytes())?;
    let responder_code = PairingCode::new(code.as_bytes())?;
    let (initiator, start) = initiator_start(&initiator_code, &mut FixedEntropy::new(0x10))?;
    let (responder, response) =
        responder_respond(&responder_code, &start, &mut FixedEntropy::new(0x80))?;
    let (initiator, initiator_confirmation) = initiator.receive_response(&response)?;
    let (responder, responder_confirmation) =
        responder.verify_initiator(&initiator_confirmation)?;
    let initiator = initiator.verify_responder(&responder_confirmation)?;
    Ok((initiator, responder))
}

#[test]
fn pairing_v1_conformance() {
    let (mut initiator, mut responder) = complete_pair("42-galaxy-pencil").unwrap();
    assert_eq!(initiator.data_token(), responder.data_token());

    let initiator_descriptor = DescriptorPayload::new(b"initiator endpoint".to_vec()).unwrap();
    let sealed = initiator.seal_descriptor(&initiator_descriptor).unwrap();
    let opened = responder.open_peer_descriptor(&sealed).unwrap();
    assert_eq!(opened.payload().as_bytes(), initiator_descriptor.as_bytes());
    assert_eq!(opened.data_token(), responder.data_token());

    let responder_descriptor = DescriptorPayload::new(b"responder endpoint".to_vec()).unwrap();
    let sealed_back = responder.seal_descriptor(&responder_descriptor).unwrap();
    let opened_back = initiator.open_peer_descriptor(&sealed_back).unwrap();
    assert_eq!(
        opened_back.payload().as_bytes(),
        responder_descriptor.as_bytes()
    );
    assert_eq!(opened_back.data_token(), initiator.data_token());

    let first_nonce = sealed_nonce(&initiator.seal_descriptor(&initiator_descriptor).unwrap());
    let second_nonce = sealed_nonce(&initiator.seal_descriptor(&initiator_descriptor).unwrap());
    let responder_nonce = sealed_nonce(&responder.seal_descriptor(&responder_descriptor).unwrap());
    assert_ne!(first_nonce, second_nonce);
    assert_ne!(&first_nonce[..4], &responder_nonce[..4]);

    let initiator_code = PairingCode::new(b"correct-code".to_vec()).unwrap();
    let responder_code = PairingCode::new(b"wrong-code".to_vec()).unwrap();
    let (initiator_state, start) =
        initiator_start(&initiator_code, &mut FixedEntropy::new(1)).unwrap();
    let (responder_state, response) =
        responder_respond(&responder_code, &start, &mut FixedEntropy::new(2)).unwrap();
    let (_, confirmation) = initiator_state.receive_response(&response).unwrap();
    let wrong_code = responder_state.verify_initiator(&confirmation).unwrap_err();
    assert_eq!(wrong_code, PairingError::ConfirmationFailed);
    assert_eq!(
        wrong_code.outcome_code(),
        Some(OutcomeCode::Unauthenticated)
    );

    let code = PairingCode::new(b"same-code".to_vec()).unwrap();
    let (initiator_state, mut start) = initiator_start(&code, &mut FixedEntropy::new(3)).unwrap();
    *start.last_mut().unwrap() ^= 1;
    let tampered_result = (|| {
        let (responder_state, response) =
            responder_respond(&code, &start, &mut FixedEntropy::new(4))?;
        let (_, confirmation) = initiator_state.receive_response(&response)?;
        responder_state.verify_initiator(&confirmation).map(|_| ())
    })();
    assert!(tampered_result.is_err());

    deterministic_fixtures();
    tamper_rejection();
    secret_redaction();
}

fn deterministic_fixtures() {
    let start = PakeStart::new(vec![0x41; 33]).unwrap();
    let response = PakeResponse::new(vec![0x42; 33]).unwrap();
    let transcript = transcript(&start, &response);
    let schedule = KeySchedule::derive(&[0x55; 32], &transcript);
    let confirmation = schedule.confirmation(&transcript, INITIATOR_CONFIRM_LABEL);

    assert_eq!(
        schedule.confirmation_key(),
        &[
            0xba, 0xbd, 0xfc, 0x75, 0x06, 0xf4, 0x35, 0x12, 0x52, 0xbe, 0xf0, 0x5e, 0xe2, 0x3a,
            0x6f, 0xd5, 0xbf, 0x82, 0xc8, 0x96, 0x9c, 0x1c, 0xdb, 0xe2, 0x1e, 0x57, 0x0b, 0x9b,
            0x15, 0x72, 0xee, 0xd7,
        ]
    );
    assert_eq!(
        schedule.bundle_key(),
        &[
            0xe5, 0x48, 0x78, 0x40, 0xec, 0xd2, 0x91, 0x19, 0xbb, 0x84, 0x54, 0x17, 0xea, 0xfb,
            0xff, 0x7b, 0x35, 0x2b, 0x26, 0x0f, 0x71, 0x11, 0x9d, 0x50, 0xbb, 0x25, 0x70, 0x26,
            0x94, 0x58, 0x91, 0x04,
        ]
    );
    assert_eq!(
        schedule.derived_data_token(),
        &[
            0x73, 0x55, 0xc4, 0x37, 0x86, 0xf0, 0x75, 0x3d, 0x11, 0x47, 0x90, 0xdd, 0xd4, 0x5b,
            0xff, 0xca, 0xd8, 0xea, 0x02, 0x4f, 0x81, 0x28, 0x72, 0xd0, 0x91, 0xbc, 0xee, 0xd9,
            0x22, 0x2d, 0xdf, 0xe3,
        ]
    );
    assert_eq!(
        confirmation.tag(),
        &[
            0x1a, 0x20, 0x70, 0x44, 0x86, 0xaf, 0x67, 0xa1, 0xd1, 0x71, 0x59, 0x67, 0x3e, 0x1d,
            0x3b, 0xa7, 0xdd, 0x5a, 0x4c, 0x5e, 0x68, 0x81, 0x8d, 0x5e, 0xe9, 0x57, 0x18, 0x72,
            0x24, 0xfd, 0xed, 0x2b,
        ]
    );

    let nonce = [0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7];
    let sealed = seal_with_nonce(
        schedule.bundle_key(),
        INITIATOR_SEAL_AAD,
        nonce,
        b"fixed descriptor plaintext",
    )
    .unwrap();
    assert_eq!(sealed.nonce(), &nonce);
    assert_eq!(
        sealed.ciphertext(),
        &[
            0xa1, 0x13, 0x7b, 0x56, 0x7e, 0xc3, 0x98, 0x41, 0x79, 0x51, 0x50, 0xab, 0x6b, 0x6f,
            0x33, 0x11, 0x51, 0x92, 0x09, 0x12, 0xb5, 0x93, 0x4b, 0x1b, 0xba, 0x8f, 0xe1, 0xd4,
            0xf9, 0xd1, 0xe2, 0xe9, 0x9e, 0x81, 0x43, 0x13, 0x59, 0x8d, 0xd0, 0x4b, 0xd7, 0x51,
        ]
    );
    let opened = open_sealed(schedule.bundle_key(), INITIATOR_SEAL_AAD, &sealed).unwrap();
    assert_eq!(opened.as_slice(), b"fixed descriptor plaintext");
}

fn tamper_rejection() {
    let (mut initiator, responder) = complete_pair("tamper-check-code").unwrap();
    let descriptor = DescriptorPayload::new(b"peer descriptor".to_vec()).unwrap();
    let encoded = initiator.seal_descriptor(&descriptor).unwrap();
    const BODY_OFFSET: usize = 5;
    const NONCE_SIZE: usize = 12;

    for index in BODY_OFFSET..BODY_OFFSET + NONCE_SIZE {
        let mut tampered = encoded.clone();
        tampered[index] ^= 1;
        assert_eq!(
            responder.open_peer_descriptor(&tampered),
            Err(PairingError::AuthenticationFailed)
        );
    }
    for index in BODY_OFFSET + NONCE_SIZE..encoded.len() {
        let mut tampered = encoded.clone();
        tampered[index] ^= 1;
        assert_eq!(
            responder.open_peer_descriptor(&tampered),
            Err(PairingError::AuthenticationFailed)
        );
    }

    let start = PakeStart::new(vec![0x41; 33]).unwrap();
    let response = PakeResponse::new(vec![0x42; 33]).unwrap();
    let transcript = transcript(&start, &response);
    let schedule = KeySchedule::derive(&[0x55; 32], &transcript);
    let sealed = seal_with_nonce(
        schedule.bundle_key(),
        INITIATOR_SEAL_AAD,
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        b"aad fixture",
    )
    .unwrap();
    for index in 0..INITIATOR_SEAL_AAD.len() {
        let mut tampered_aad = INITIATOR_SEAL_AAD.to_vec();
        tampered_aad[index] ^= 1;
        assert_eq!(
            open_sealed(schedule.bundle_key(), &tampered_aad, &sealed),
            Err(PairingError::AuthenticationFailed)
        );
    }
}

fn secret_redaction() {
    let code_text = "redaction-sentinel-code";
    let code = PairingCode::new(code_text.as_bytes()).unwrap();
    assert!(!format!("{code:?}").contains(code_text));
    assert!(!format!("{code}").contains(code_text));

    let secret = Secret::new("redaction-sentinel-key");
    assert!(!format!("{secret:?}").contains(secret.expose()));
    assert!(!format!("{secret}").contains(secret.expose()));

    let (initiator, _) = complete_pair("redaction-pair-code").unwrap();
    let token = initiator.data_token();
    let token_hex = token
        .expose()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(!format!("{token:?}").contains(&token_hex));
    assert!(!format!("{token}").contains(&token_hex));
}

fn sealed_nonce(encoded: &[u8]) -> [u8; 12] {
    match decode_message(encoded).unwrap() {
        PairingMessage::SealedDescriptor(sealed) => *sealed.nonce(),
        other => panic!("expected sealed descriptor, got {:?}", other.kind()),
    }
}

#[test]
fn bounded_message_codec_rejects_malformed_input() {
    let code = PairingCode::new(b"codec-test-code".to_vec()).unwrap();
    let (_, start) = initiator_start(&code, &mut FixedEntropy::new(9)).unwrap();
    for truncated_at in 0..start.len() {
        assert!(decode_wire_message(&start[..truncated_at]).is_err());
    }

    let mut oversized = vec![1, 0, 0, 0, 0];
    oversized[1..5].copy_from_slice(&((MAX_MESSAGE_BODY as u32) + 1).to_be_bytes());
    assert!(matches!(
        decode_wire_message(&oversized),
        Err(PairingError::MessageTooLarge { .. })
    ));

    let mut unknown = start.clone();
    unknown[0] = 0xff;
    assert_eq!(
        decode_wire_message(&unknown),
        Err(PairingError::UnknownMessageType { wire_id: 0xff })
    );

    let oversized_sealed_body = 12 + MAX_SEALED_CIPHERTEXT_SIZE + 1;
    let mut oversized_sealed = vec![4];
    oversized_sealed.extend_from_slice(&(oversized_sealed_body as u32).to_be_bytes());
    oversized_sealed.resize(5 + oversized_sealed_body, 0);
    assert!(matches!(
        decode_wire_message(&oversized_sealed),
        Err(PairingError::InvalidMessageLength { .. })
    ));

    let mut wrong_kind = start;
    wrong_kind[0] = 2;
    assert!(matches!(
        responder_respond(&code, &wrong_kind, &mut FixedEntropy::new(10)),
        Err(PairingError::UnexpectedMessage { .. })
    ));

    let malformed_sealed = SealedDescriptor::new([0; 12], Vec::new());
    assert!(matches!(
        malformed_sealed,
        Err(PairingError::InvalidMessageLength { .. })
    ));
}
