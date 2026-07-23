use envoix_outcomes::OutcomeCode;
use envoix_pairing::{
    EntropyError, EntropySource, Paired, PairingCode, initiator_start, responder_respond,
};
use envoix_protocol::{DecodeError, IngressState, decode_frame};

use crate::message::{CONFIRMATION_SIZE, Confirmation, Start};
use crate::{
    AuthCodecError, AuthError, AuthField, AuthMessage, AuthMessageKind, Deadline,
    ExportedKeyingMaterial, MonotonicMillis, PeerRole, ReceiverAwaitConfirm, SenderAwaitConfirm,
    decode_auth_message, encode_auth_message, receiver_wait, sender_start,
};

const DEADLINE: Deadline = Deadline::at(MonotonicMillis(1_000));
const BEFORE_DEADLINE: MonotonicMillis = MonotonicMillis(999);
const AT_DEADLINE: MonotonicMillis = MonotonicMillis(1_000);

macro_rules! assert_wait_closure {
    ($state:expr) => {{
        assert!(matches!(
            $state.deadline_exceeded(AT_DEADLINE),
            Err(AuthError::Timeout)
        ));
        assert_eq!($state.peer_closed(), AuthError::PeerClosed);
        assert_eq!($state.cancel(), AuthError::Cancelled);
    }};
}

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

struct FailedEntropy;

impl EntropySource for FailedEntropy {
    fn fill(&mut self, _destination: &mut [u8]) -> Result<(), EntropyError> {
        Err(EntropyError::Unavailable)
    }
}

fn complete_pair(code: &[u8], initiator_seed: u8, responder_seed: u8) -> (Paired, Paired) {
    let initiator_code = PairingCode::new(code).unwrap();
    let responder_code = PairingCode::new(code).unwrap();
    let (initiator, start) =
        initiator_start(&initiator_code, &mut FixedEntropy::new(initiator_seed)).unwrap();
    let (responder, response) = responder_respond(
        &responder_code,
        &start,
        &mut FixedEntropy::new(responder_seed),
    )
    .unwrap();
    let (initiator, initiator_confirmation) = initiator.receive_response(&response).unwrap();
    let (responder, responder_confirmation) =
        responder.verify_initiator(&initiator_confirmation).unwrap();
    let initiator = initiator.verify_responder(&responder_confirmation).unwrap();
    (initiator, responder)
}

fn binding(byte: u8) -> ExportedKeyingMaterial {
    ExportedKeyingMaterial::new([byte; 32])
}

fn begin_matching() -> (
    Paired,
    Paired,
    crate::SenderAwaitResponse,
    crate::ReceiverAwaitStart,
    Vec<u8>,
) {
    let (sender_token, receiver_token) = complete_pair(b"m2-characterization", 0x10, 0x80);
    let (sender, start) = sender_start(
        sender_token.data_token(),
        binding(0x44),
        DEADLINE,
        &mut FixedEntropy::new(0x20),
    )
    .unwrap();
    let receiver = receiver_wait(binding(0x44), DEADLINE);
    (sender_token, receiver_token, sender, receiver, start)
}

fn make_confirm_states() -> (SenderAwaitConfirm, ReceiverAwaitConfirm, Vec<u8>, Vec<u8>) {
    let (_sender_token, receiver_token, sender, receiver, start) = begin_matching();
    let (receiver, response) = receiver
        .receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(0x90),
        )
        .unwrap();
    let (sender, sender_confirmation) =
        sender.receive_response(&response, BEFORE_DEADLINE).unwrap();
    (sender, receiver, response, sender_confirmation)
}

fn take_error<T>(result: Result<T, AuthError>) -> AuthError {
    match result {
        Ok(_) => panic!("expected authentication failure"),
        Err(error) => error,
    }
}

#[test]
fn auth_handshake_characterization() {
    matching_handshake_and_role_separation();
    receiver_rejects_before_revealing_proof();
    exporter_mismatch_fails_confirmation();
    invalid_role_nonce_and_message_kinds_are_typed();
}

fn matching_handshake_and_role_separation() {
    let (_sender_token, receiver_token, sender, receiver, start) = begin_matching();
    let (receiver, response) = receiver
        .receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(0x90),
        )
        .unwrap();
    let (sender, sender_confirmation) =
        sender.receive_response(&response, BEFORE_DEADLINE).unwrap();
    let sender_proof = confirmation_proof(&sender_confirmation);
    let (receiver_authenticated, receiver_confirmation) = receiver
        .receive_confirmation(&sender_confirmation, BEFORE_DEADLINE)
        .unwrap();
    let receiver_proof = confirmation_proof(&receiver_confirmation);
    let sender_authenticated = sender
        .receive_confirmation(&receiver_confirmation, BEFORE_DEADLINE)
        .unwrap();

    assert_eq!(sender_authenticated.role(), PeerRole::Sender);
    assert_eq!(receiver_authenticated.role(), PeerRole::Receiver);
    assert_ne!(sender_proof, receiver_proof);

    assert_eq!(
        decode_frame(&start, IngressState::AwaitHello),
        Err(DecodeError::UnknownFrameType { wire_id: 1 })
    );
    assert_start_wire_fixture();
}

fn receiver_rejects_before_revealing_proof() {
    let (sender_token, _) = complete_pair(b"sender-token", 1, 2);
    let (_, receiver_token) = complete_pair(b"receiver-token", 3, 4);
    let (sender, start) = sender_start(
        sender_token.data_token(),
        binding(0x55),
        DEADLINE,
        &mut FixedEntropy::new(5),
    )
    .unwrap();
    let (receiver, response) = receiver_wait(binding(0x55), DEADLINE)
        .receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(6),
        )
        .unwrap();
    let (_sender, sender_confirmation) =
        sender.receive_response(&response, BEFORE_DEADLINE).unwrap();

    let error = take_error(receiver.receive_confirmation(&sender_confirmation, BEFORE_DEADLINE));
    assert_eq!(error, AuthError::ConfirmationFailed);
    assert_eq!(error.outcome_code(), OutcomeCode::Unauthenticated);
    // On failure the API returns only AuthError, so no receiver confirmation can exist.
}

fn exporter_mismatch_fails_confirmation() {
    let (sender_token, receiver_token) = complete_pair(b"same-token", 7, 8);
    let (sender, start) = sender_start(
        sender_token.data_token(),
        binding(0x11),
        DEADLINE,
        &mut FixedEntropy::new(9),
    )
    .unwrap();
    let (receiver, response) = receiver_wait(binding(0x22), DEADLINE)
        .receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(10),
        )
        .unwrap();
    let (_sender, sender_confirmation) =
        sender.receive_response(&response, BEFORE_DEADLINE).unwrap();

    assert_eq!(
        take_error(receiver.receive_confirmation(&sender_confirmation, BEFORE_DEADLINE)),
        AuthError::ConfirmationFailed
    );
}

fn invalid_role_nonce_and_message_kinds_are_typed() {
    let (_sender_token, receiver_token, _sender, receiver, mut start) = begin_matching();
    start[13] = PeerRole::Receiver.wire_id();
    assert_eq!(
        take_error(receiver.receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(11),
        )),
        AuthError::InvalidStartRole {
            actual: PeerRole::Receiver
        }
    );

    let (_sender_token, receiver_token, _sender, receiver, mut start) = begin_matching();
    start[14..18].copy_from_slice(&31_u32.to_be_bytes());
    assert_eq!(
        take_error(receiver.receive_start(
            &start,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(12),
        )),
        AuthError::Codec(AuthCodecError::InvalidFieldLength {
            field: AuthField::Nonce,
            actual: 31,
            expected: 32,
        })
    );

    let (_sender_token, _receiver_token, sender, _receiver, start) = begin_matching();
    assert_eq!(
        take_error(sender.receive_response(&start, BEFORE_DEADLINE)),
        AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Response,
            actual: AuthMessageKind::Start,
        }
    );

    let (_sender_token, receiver_token, _sender, receiver, _start) = begin_matching();
    let confirmation = encode_auth_message(&AuthMessage::Confirm(Confirmation::new(
        [0; CONFIRMATION_SIZE],
    )));
    assert_eq!(
        take_error(receiver.receive_start(
            &confirmation,
            BEFORE_DEADLINE,
            receiver_token.data_token(),
            &mut FixedEntropy::new(13),
        )),
        AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Start,
            actual: AuthMessageKind::Confirm,
        }
    );

    let (sender, receiver, response, _sender_confirmation) = make_confirm_states();
    assert_eq!(
        take_error(receiver.receive_confirmation(&response, BEFORE_DEADLINE)),
        AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Confirm,
            actual: AuthMessageKind::Response,
        }
    );
    assert_eq!(
        take_error(sender.receive_confirmation(&response, BEFORE_DEADLINE)),
        AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Confirm,
            actual: AuthMessageKind::Response,
        }
    );
}

#[test]
fn handshake_wait_closure() {
    assert_wait_closure!(sender_response_state());
    assert_wait_closure!(receiver_start_state());
    assert_wait_closure!(sender_confirm_state());
    assert_wait_closure!(receiver_confirm_state());

    assert_eq!(AuthError::Timeout.outcome_code(), OutcomeCode::Timeout);
    assert_eq!(AuthError::PeerClosed.outcome_code(), OutcomeCode::PeerLost);
    assert_eq!(AuthError::Cancelled.outcome_code(), OutcomeCode::Cancelled);
}

fn sender_response_state() -> crate::SenderAwaitResponse {
    begin_matching().2
}

fn receiver_start_state() -> crate::ReceiverAwaitStart {
    receiver_wait(binding(0x44), DEADLINE)
}

fn sender_confirm_state() -> SenderAwaitConfirm {
    make_confirm_states().0
}

fn receiver_confirm_state() -> ReceiverAwaitConfirm {
    make_confirm_states().1
}

#[test]
fn codec_bounds_entropy_and_redaction() {
    let start = AuthMessage::Start(Start::new(PeerRole::Sender, [0xab; 32], [0xcd; 33]));
    let debug = format!("{start:?}");
    assert!(!debug.contains("171"));
    assert!(!debug.contains("205"));
    assert!(debug.contains("Start"));

    let exported = binding(0xef);
    assert_eq!(
        format!("{exported:?}"),
        "ExportedKeyingMaterial([REDACTED])"
    );
    assert!(!format!("{exported:?}").contains("239"));

    let mut oversized = vec![0_u8; 12];
    oversized[..4].copy_from_slice(envoix_protocol::identifiers::DATA_MAGIC);
    oversized[4..6].copy_from_slice(&envoix_protocol::identifiers::DATA_WIRE_VERSION.to_be_bytes());
    oversized[6] = crate::AUTH_WIRE_ID;
    oversized[8..12].copy_from_slice(&((crate::MAX_AUTH_PAYLOAD + 1) as u32).to_be_bytes());
    assert_eq!(
        decode_auth_message(&oversized),
        Err(AuthCodecError::PayloadTooLarge {
            declared: crate::MAX_AUTH_PAYLOAD + 1,
            maximum: crate::MAX_AUTH_PAYLOAD,
        })
    );

    let (paired, _) = complete_pair(b"entropy-failure", 14, 15);
    let error = take_error(sender_start(
        paired.data_token(),
        binding(1),
        DEADLINE,
        &mut FailedEntropy,
    ));
    assert_eq!(error, AuthError::EntropyUnavailable);
    assert_eq!(error.outcome_code(), OutcomeCode::Internal);
}

fn confirmation_proof(encoded: &[u8]) -> [u8; CONFIRMATION_SIZE] {
    match decode_auth_message(encoded).unwrap() {
        AuthMessage::Confirm(confirmation) => *confirmation.proof(),
        other => panic!("expected confirmation, received {}", other.kind()),
    }
}

fn assert_start_wire_fixture() {
    let start = Start::new(PeerRole::Sender, [0x11; 32], [0x22; 33]);
    let encoded = encode_auth_message(&AuthMessage::Start(start.clone()));
    let mut expected = vec![
        b'E', b'N', b'V', b'X', 0, 2, 1, 0, 0, 0, 0, 75, 1, 1, 0, 0, 0, 32,
    ];
    expected.extend_from_slice(&[0x11; 32]);
    expected.extend_from_slice(&[0, 0, 0, 33]);
    expected.extend_from_slice(&[0x22; 33]);
    assert_eq!(encoded, expected);
    assert_eq!(decode_auth_message(&encoded), Ok(AuthMessage::Start(start)));
}
