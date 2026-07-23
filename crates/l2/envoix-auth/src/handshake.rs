use std::fmt;

use envoix_pairing::{DataPlaneToken, EntropySource};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::Zeroizing;

use crate::message::{
    AuthMessage, AuthMessageKind, Confirmation, NONCE_SIZE, RESPONSE_MESSAGE_SIZE, Response,
    START_MESSAGE_SIZE, Start, decode_auth_message, encode_auth_message,
};
use crate::random::AuthRng;
use crate::{AuthError, identifiers};

const CONFIRMATION_SIZE: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRole {
    Sender,
    Receiver,
}

impl PeerRole {
    pub(crate) const fn wire_id(self) -> u8 {
        match self {
            Self::Sender => 1,
            Self::Receiver => 2,
        }
    }

    pub(crate) fn from_wire_id(id: u8) -> Result<Self, crate::AuthCodecError> {
        match id {
            1 => Ok(Self::Sender),
            2 => Ok(Self::Receiver),
            wire_id => Err(crate::AuthCodecError::InvalidRole { wire_id }),
        }
    }
}

impl fmt::Display for PeerRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sender => formatter.write_str("sender"),
            Self::Receiver => formatter.write_str("receiver"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicMillis(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline(MonotonicMillis);

impl Deadline {
    pub const fn at(time: MonotonicMillis) -> Self {
        Self(time)
    }

    pub const fn instant(self) -> MonotonicMillis {
        self.0
    }

    fn elapsed(self, now: MonotonicMillis) -> bool {
        now >= self.0
    }
}

pub struct ExportedKeyingMaterial(Zeroizing<[u8; 32]>);

impl ExportedKeyingMaterial {
    pub fn new(material: [u8; 32]) -> Self {
        Self(Zeroizing::new(material))
    }

    pub const fn label() -> &'static [u8] {
        identifiers::EXPORTER_LABEL
    }

    pub const fn context() -> &'static [u8] {
        identifiers::EXPORTER_CONTEXT
    }

    fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ExportedKeyingMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExportedKeyingMaterial([REDACTED])")
    }
}

pub struct Authenticated {
    role: PeerRole,
}

impl Authenticated {
    pub const fn role(&self) -> PeerRole {
        self.role
    }
}

impl fmt::Debug for Authenticated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Authenticated")
            .field("role", &self.role)
            .finish()
    }
}

struct Wait<S> {
    state: S,
    deadline: Deadline,
}

impl<S> Wait<S> {
    fn new(state: S, deadline: Deadline) -> Self {
        Self { state, deadline }
    }

    fn into_live(self, now: MonotonicMillis) -> Result<(S, Deadline), AuthError> {
        if self.deadline.elapsed(now) {
            Err(AuthError::Timeout)
        } else {
            Ok((self.state, self.deadline))
        }
    }
}

macro_rules! bounded_wait {
    ($name:ident, $state:ident) => {
        pub struct $name(Wait<$state>);

        impl $name {
            pub const fn deadline(&self) -> Deadline {
                self.0.deadline
            }

            pub fn cancel(self) -> AuthError {
                AuthError::Cancelled
            }

            pub fn peer_closed(self) -> AuthError {
                AuthError::PeerClosed
            }

            pub fn deadline_exceeded(self, now: MonotonicMillis) -> Result<Self, AuthError> {
                if self.0.deadline.elapsed(now) {
                    Err(AuthError::Timeout)
                } else {
                    Ok(self)
                }
            }
        }
    };
}

struct SenderResponseState {
    spake: Spake2<Ed25519Group>,
    start: Start,
    binding: ExportedKeyingMaterial,
}

struct SenderConfirmState {
    confirmation: ConfirmationMaterial,
}

struct ReceiverStartState {
    binding: ExportedKeyingMaterial,
}

struct ReceiverConfirmState {
    confirmation: ConfirmationMaterial,
}

bounded_wait!(SenderAwaitResponse, SenderResponseState);
bounded_wait!(SenderAwaitConfirm, SenderConfirmState);
bounded_wait!(ReceiverAwaitStart, ReceiverStartState);
bounded_wait!(ReceiverAwaitConfirm, ReceiverConfirmState);

pub fn sender_start(
    token: &DataPlaneToken,
    binding: ExportedKeyingMaterial,
    deadline: Deadline,
    entropy: &mut impl EntropySource,
) -> Result<(SenderAwaitResponse, Vec<u8>), AuthError> {
    let nonce = nonce(entropy)?;
    let mut rng = AuthRng::from_entropy(entropy)?;
    let (spake, message) = Spake2::<Ed25519Group>::start_a_with_rng(
        &Password::new(token.expose()),
        &Identity::new(identifiers::SENDER_IDENTITY),
        &Identity::new(identifiers::RECEIVER_IDENTITY),
        &mut rng,
    );
    let message = fixed_array::<START_MESSAGE_SIZE>(message)?;
    let start = Start::new(PeerRole::Sender, nonce, message);
    let encoded = encode_auth_message(&AuthMessage::Start(start.clone()));
    let state = SenderResponseState {
        spake,
        start,
        binding,
    };
    Ok((SenderAwaitResponse(Wait::new(state, deadline)), encoded))
}

pub fn receiver_wait(binding: ExportedKeyingMaterial, deadline: Deadline) -> ReceiverAwaitStart {
    ReceiverAwaitStart(Wait::new(ReceiverStartState { binding }, deadline))
}

impl ReceiverAwaitStart {
    pub fn receive_start(
        self,
        encoded: &[u8],
        now: MonotonicMillis,
        token: &DataPlaneToken,
        entropy: &mut impl EntropySource,
    ) -> Result<(ReceiverAwaitConfirm, Vec<u8>), AuthError> {
        let (state, deadline) = self.0.into_live(now)?;
        let start = expect_start(decode_auth_message(encoded)?)?;
        if start.role() != PeerRole::Sender {
            return Err(AuthError::InvalidStartRole {
                actual: start.role(),
            });
        }

        let receiver_nonce = nonce(entropy)?;
        let mut rng = AuthRng::from_entropy(entropy)?;
        let (spake, message) = Spake2::<Ed25519Group>::start_b_with_rng(
            &Password::new(token.expose()),
            &Identity::new(identifiers::SENDER_IDENTITY),
            &Identity::new(identifiers::RECEIVER_IDENTITY),
            &mut rng,
        );
        let message = fixed_array::<RESPONSE_MESSAGE_SIZE>(message)?;
        let response = Response::new(receiver_nonce, message);
        let shared = finish_spake(spake, start.message())?;
        let transcript = transcript(&start, &response, &state.binding);
        let confirmation = ConfirmationMaterial { shared, transcript };
        let encoded = encode_auth_message(&AuthMessage::Response(response));
        let next = ReceiverConfirmState { confirmation };
        Ok((ReceiverAwaitConfirm(Wait::new(next, deadline)), encoded))
    }
}

impl SenderAwaitResponse {
    pub fn receive_response(
        self,
        encoded: &[u8],
        now: MonotonicMillis,
    ) -> Result<(SenderAwaitConfirm, Vec<u8>), AuthError> {
        let (state, deadline) = self.0.into_live(now)?;
        let response = expect_response(decode_auth_message(encoded)?)?;
        let shared = finish_spake(state.spake, response.message())?;
        let transcript = transcript(&state.start, &response, &state.binding);
        let confirmation = ConfirmationMaterial { shared, transcript };
        let proof = confirmation.proof(identifiers::SENDER_CONFIRM_LABEL);
        let encoded = encode_auth_message(&AuthMessage::Confirm(Confirmation::new(proof)));
        let next = SenderConfirmState { confirmation };
        Ok((SenderAwaitConfirm(Wait::new(next, deadline)), encoded))
    }
}

impl ReceiverAwaitConfirm {
    pub fn receive_confirmation(
        self,
        encoded: &[u8],
        now: MonotonicMillis,
    ) -> Result<(Authenticated, Vec<u8>), AuthError> {
        let (state, _deadline) = self.0.into_live(now)?;
        let confirmation = expect_confirmation(decode_auth_message(encoded)?)?;
        state
            .confirmation
            .verify(identifiers::SENDER_CONFIRM_LABEL, confirmation.proof())?;

        // The receiver proof is not produced until the sender proof is authenticated.
        let proof = state
            .confirmation
            .proof(identifiers::RECEIVER_CONFIRM_LABEL);
        let encoded = encode_auth_message(&AuthMessage::Confirm(Confirmation::new(proof)));
        Ok((
            Authenticated {
                role: PeerRole::Receiver,
            },
            encoded,
        ))
    }
}

impl SenderAwaitConfirm {
    pub fn receive_confirmation(
        self,
        encoded: &[u8],
        now: MonotonicMillis,
    ) -> Result<Authenticated, AuthError> {
        let (state, _deadline) = self.0.into_live(now)?;
        let confirmation = expect_confirmation(decode_auth_message(encoded)?)?;
        state
            .confirmation
            .verify(identifiers::RECEIVER_CONFIRM_LABEL, confirmation.proof())?;
        Ok(Authenticated {
            role: PeerRole::Sender,
        })
    }
}

struct ConfirmationMaterial {
    shared: Zeroizing<[u8; 32]>,
    transcript: Zeroizing<Vec<u8>>,
}

impl ConfirmationMaterial {
    fn proof(&self, label: &[u8]) -> [u8; CONFIRMATION_SIZE] {
        let mut hasher = blake3::Hasher::new_keyed(&self.shared);
        hasher.update(self.transcript.as_ref());
        append_field_to_hasher(&mut hasher, label);
        *hasher.finalize().as_bytes()
    }

    fn verify(&self, label: &[u8], received: &[u8; CONFIRMATION_SIZE]) -> Result<(), AuthError> {
        let expected = blake3::Hash::from_bytes(self.proof(label));
        let received = blake3::Hash::from_bytes(*received);
        if expected == received {
            Ok(())
        } else {
            Err(AuthError::ConfirmationFailed)
        }
    }
}

fn nonce(entropy: &mut impl EntropySource) -> Result<[u8; NONCE_SIZE], AuthError> {
    let mut value = [0_u8; NONCE_SIZE];
    entropy
        .fill(&mut value)
        .map_err(|_| AuthError::EntropyUnavailable)?;
    Ok(value)
}

fn finish_spake(
    spake: Spake2<Ed25519Group>,
    peer_message: &[u8],
) -> Result<Zeroizing<[u8; 32]>, AuthError> {
    let shared = Zeroizing::new(
        spake
            .finish(peer_message)
            .map_err(|_| AuthError::SpakeRejected)?,
    );
    let mut key = Zeroizing::new([0_u8; 32]);
    if shared.len() != key.len() {
        return Err(AuthError::SpakeRejected);
    }
    key.copy_from_slice(shared.as_slice());
    Ok(key)
}

fn fixed_array<const N: usize>(bytes: Vec<u8>) -> Result<[u8; N], AuthError> {
    bytes.try_into().map_err(|_| AuthError::SpakeRejected)
}

fn transcript(
    start: &Start,
    response: &Response,
    binding: &ExportedKeyingMaterial,
) -> Zeroizing<Vec<u8>> {
    let mut transcript = Zeroizing::new(Vec::new());
    append_field(&mut transcript, identifiers::SPAKE2_DOMAIN);
    append_field(&mut transcript, identifiers::SENDER_IDENTITY);
    append_field(&mut transcript, identifiers::RECEIVER_IDENTITY);
    append_field(&mut transcript, identifiers::EXPORTER_LABEL);
    append_field(&mut transcript, identifiers::EXPORTER_CONTEXT);
    append_field(&mut transcript, start.nonce());
    append_field(&mut transcript, response.nonce());
    append_field(&mut transcript, start.message());
    append_field(&mut transcript, response.message());
    append_field(&mut transcript, binding.expose());
    transcript
}

fn append_field(output: &mut Vec<u8>, field: &[u8]) {
    output.extend_from_slice(&(field.len() as u64).to_be_bytes());
    output.extend_from_slice(field);
}

fn append_field_to_hasher(hasher: &mut blake3::Hasher, field: &[u8]) {
    hasher.update(&(field.len() as u64).to_be_bytes());
    hasher.update(field);
}

fn expect_start(message: AuthMessage) -> Result<Start, AuthError> {
    match message {
        AuthMessage::Start(start) => Ok(start),
        other => Err(AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Start,
            actual: other.kind(),
        }),
    }
}

fn expect_response(message: AuthMessage) -> Result<Response, AuthError> {
    match message {
        AuthMessage::Response(response) => Ok(response),
        other => Err(AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Response,
            actual: other.kind(),
        }),
    }
}

fn expect_confirmation(message: AuthMessage) -> Result<Confirmation, AuthError> {
    match message {
        AuthMessage::Confirm(confirmation) => Ok(confirmation),
        other => Err(AuthError::UnexpectedMessage {
            expected: AuthMessageKind::Confirm,
            actual: other.kind(),
        }),
    }
}
