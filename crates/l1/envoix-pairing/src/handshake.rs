use std::fmt;

use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::Zeroizing;

use crate::PairingError;
use crate::bundle::{DescriptorPayload, PeerDescriptor, open_descriptor, seal_descriptor};
use crate::identifiers::{
    BUNDLE_KEY_CONTEXT, CONFIRM_KEY_CONTEXT, DATA_TOKEN_CONTEXT, INITIATOR_CONFIRM_LABEL,
    INITIATOR_IDENTITY, INITIATOR_SEAL_AAD, RESPONDER_CONFIRM_LABEL, RESPONDER_IDENTITY,
    RESPONDER_SEAL_AAD, SPAKE2_DOMAIN,
};
use crate::message::{
    Confirmation, MessageKind, PairingMessage, PakeResponse, PakeStart, decode_message,
    encode_message,
};
use crate::random::{EntropySource, SpakeRng};
use crate::secret::{DataPlaneToken, PairingCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Initiator,
    Responder,
}

impl Role {
    pub const fn peer(self) -> Self {
        match self {
            Self::Initiator => Self::Responder,
            Self::Responder => Self::Initiator,
        }
    }

    pub(crate) const fn seal_aad(self) -> &'static [u8] {
        match self {
            Self::Initiator => INITIATOR_SEAL_AAD,
            Self::Responder => RESPONDER_SEAL_AAD,
        }
    }

    pub(crate) const fn nonce_prefix(self) -> [u8; 4] {
        match self {
            Self::Initiator => [1, 0, 0, 0],
            Self::Responder => [2, 0, 0, 0],
        }
    }
}

pub struct InitiatorAwaitResponse {
    spake: Spake2<Ed25519Group>,
    start: PakeStart,
}

pub struct InitiatorConfirming {
    keys: KeySchedule,
    transcript: Vec<u8>,
}

pub struct ResponderAwaitConfirm {
    keys: KeySchedule,
    transcript: Vec<u8>,
}

pub struct Paired {
    role: Role,
    bundle_key: Zeroizing<[u8; 32]>,
    data_token: DataPlaneToken,
    next_nonce: u64,
}

impl Paired {
    fn new(role: Role, keys: KeySchedule) -> Self {
        let KeySchedule {
            confirmation_key,
            bundle_key,
            data_token,
        } = keys;
        drop(confirmation_key);
        Self {
            role,
            bundle_key,
            data_token: DataPlaneToken::from_zeroizing(data_token),
            next_nonce: 0,
        }
    }

    pub const fn role(&self) -> Role {
        self.role
    }

    pub const fn data_token(&self) -> &DataPlaneToken {
        &self.data_token
    }

    pub fn seal_descriptor(
        &mut self,
        descriptor: &DescriptorPayload,
    ) -> Result<Vec<u8>, PairingError> {
        let nonce = self.take_nonce()?;
        seal_descriptor(
            &self.bundle_key,
            self.role,
            nonce,
            descriptor,
            self.data_token.expose(),
        )
    }

    pub fn open_peer_descriptor(&self, encoded: &[u8]) -> Result<PeerDescriptor, PairingError> {
        open_descriptor(
            &self.bundle_key,
            self.role.peer(),
            encoded,
            self.data_token.expose(),
        )
    }

    fn take_nonce(&mut self) -> Result<[u8; 12], PairingError> {
        let next = self
            .next_nonce
            .checked_add(1)
            .ok_or(PairingError::NonceExhausted)?;
        let mut nonce = [0; 12];
        nonce[..4].copy_from_slice(&self.role.nonce_prefix());
        nonce[4..].copy_from_slice(&self.next_nonce.to_be_bytes());
        self.next_nonce = next;
        Ok(nonce)
    }
}

impl fmt::Debug for Paired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Paired")
            .field("role", &self.role)
            .field("secrets", &"[redacted]")
            .finish()
    }
}

pub fn initiator_start(
    code: &PairingCode,
    entropy: &mut impl EntropySource,
) -> Result<(InitiatorAwaitResponse, Vec<u8>), PairingError> {
    let rng = SpakeRng::from_entropy(entropy)?;
    let (initiator_id, responder_id) = spake_identities();
    let password = Password::new(code.as_bytes());
    let (spake, message) =
        Spake2::<Ed25519Group>::start_a_with_rng(&password, &initiator_id, &responder_id, rng);
    let start = PakeStart::new(message)?;
    let encoded = encode_message(&PairingMessage::Start(start.clone()))?;
    Ok((InitiatorAwaitResponse { spake, start }, encoded))
}

pub fn responder_respond(
    code: &PairingCode,
    encoded_start: &[u8],
    entropy: &mut impl EntropySource,
) -> Result<(ResponderAwaitConfirm, Vec<u8>), PairingError> {
    let start = expect_start(decode_message(encoded_start)?)?;
    let rng = SpakeRng::from_entropy(entropy)?;
    let (initiator_id, responder_id) = spake_identities();
    let password = Password::new(code.as_bytes());
    let (spake, message) =
        Spake2::<Ed25519Group>::start_b_with_rng(&password, &initiator_id, &responder_id, rng);
    let shared = Zeroizing::new(
        spake
            .finish(start.message())
            .map_err(|_| PairingError::SpakeRejected)?,
    );
    let response = PakeResponse::new(message)?;
    let transcript = transcript(&start, &response);
    let keys = KeySchedule::derive(&shared, &transcript);
    let encoded = encode_message(&PairingMessage::Response(response))?;
    Ok((ResponderAwaitConfirm { keys, transcript }, encoded))
}

impl InitiatorAwaitResponse {
    pub fn receive_response(
        self,
        encoded_response: &[u8],
    ) -> Result<(InitiatorConfirming, Vec<u8>), PairingError> {
        let response = expect_response(decode_message(encoded_response)?)?;
        let shared = Zeroizing::new(
            self.spake
                .finish(response.message())
                .map_err(|_| PairingError::SpakeRejected)?,
        );
        let transcript = transcript(&self.start, &response);
        let keys = KeySchedule::derive(&shared, &transcript);
        let confirmation = keys.confirmation(&transcript, INITIATOR_CONFIRM_LABEL);
        let encoded = encode_message(&PairingMessage::Confirm(confirmation))?;
        Ok((InitiatorConfirming { keys, transcript }, encoded))
    }
}

impl ResponderAwaitConfirm {
    pub fn verify_initiator(
        self,
        encoded_confirmation: &[u8],
    ) -> Result<(Paired, Vec<u8>), PairingError> {
        let confirmation = expect_confirmation(decode_message(encoded_confirmation)?)?;
        self.keys
            .verify_confirmation(&self.transcript, INITIATOR_CONFIRM_LABEL, &confirmation)?;
        let response = self
            .keys
            .confirmation(&self.transcript, RESPONDER_CONFIRM_LABEL);
        let encoded = encode_message(&PairingMessage::Confirm(response))?;
        Ok((Paired::new(Role::Responder, self.keys), encoded))
    }
}

impl InitiatorConfirming {
    pub fn verify_responder(self, encoded_confirmation: &[u8]) -> Result<Paired, PairingError> {
        let confirmation = expect_confirmation(decode_message(encoded_confirmation)?)?;
        self.keys
            .verify_confirmation(&self.transcript, RESPONDER_CONFIRM_LABEL, &confirmation)?;
        Ok(Paired::new(Role::Initiator, self.keys))
    }
}

pub(crate) struct KeySchedule {
    confirmation_key: Zeroizing<[u8; 32]>,
    bundle_key: Zeroizing<[u8; 32]>,
    data_token: Zeroizing<[u8; 32]>,
}

impl KeySchedule {
    pub(crate) fn derive(shared: &[u8], transcript: &[u8]) -> Self {
        let material = Zeroizing::new(encode_parts(&[shared, transcript]));
        Self {
            confirmation_key: Zeroizing::new(blake3::derive_key(CONFIRM_KEY_CONTEXT, &material)),
            bundle_key: Zeroizing::new(blake3::derive_key(BUNDLE_KEY_CONTEXT, &material)),
            data_token: Zeroizing::new(blake3::derive_key(DATA_TOKEN_CONTEXT, &material)),
        }
    }

    pub(crate) fn confirmation(&self, transcript: &[u8], label: &[u8]) -> Confirmation {
        let mut hasher = blake3::Hasher::new_keyed(&self.confirmation_key);
        hasher.update(&encode_parts(&[transcript, label]));
        Confirmation::new(*hasher.finalize().as_bytes())
    }

    #[cfg(test)]
    pub(crate) fn confirmation_key(&self) -> &[u8; 32] {
        &self.confirmation_key
    }

    #[cfg(test)]
    pub(crate) fn bundle_key(&self) -> &[u8; 32] {
        &self.bundle_key
    }

    #[cfg(test)]
    pub(crate) fn derived_data_token(&self) -> &[u8; 32] {
        &self.data_token
    }

    pub(crate) fn verify_confirmation(
        &self,
        transcript: &[u8],
        label: &[u8],
        received: &Confirmation,
    ) -> Result<(), PairingError> {
        let expected = blake3::Hash::from_bytes(*self.confirmation(transcript, label).tag());
        let received = blake3::Hash::from_bytes(*received.tag());
        // BLAKE3's fixed-size Hash equality is constant-time.
        if expected == received {
            Ok(())
        } else {
            Err(PairingError::ConfirmationFailed)
        }
    }
}

pub(crate) fn transcript(start: &PakeStart, response: &PakeResponse) -> Vec<u8> {
    encode_parts(&[
        SPAKE2_DOMAIN,
        INITIATOR_IDENTITY,
        RESPONDER_IDENTITY,
        start.message(),
        response.message(),
    ])
}

fn spake_identities() -> (Identity, Identity) {
    (
        Identity::new(&encode_parts(&[SPAKE2_DOMAIN, INITIATOR_IDENTITY])),
        Identity::new(&encode_parts(&[SPAKE2_DOMAIN, RESPONDER_IDENTITY])),
    )
}

fn encode_parts(parts: &[&[u8]]) -> Vec<u8> {
    let capacity = parts.iter().map(|part| 8 + part.len()).sum();
    let mut encoded = Vec::with_capacity(capacity);
    for part in parts {
        encoded.extend_from_slice(&(part.len() as u64).to_be_bytes());
        encoded.extend_from_slice(part);
    }
    encoded
}

fn expect_start(message: PairingMessage) -> Result<PakeStart, PairingError> {
    match message {
        PairingMessage::Start(start) => Ok(start),
        other => Err(PairingError::UnexpectedMessage {
            expected: MessageKind::Start,
            actual: other.kind(),
        }),
    }
}

fn expect_response(message: PairingMessage) -> Result<PakeResponse, PairingError> {
    match message {
        PairingMessage::Response(response) => Ok(response),
        other => Err(PairingError::UnexpectedMessage {
            expected: MessageKind::Response,
            actual: other.kind(),
        }),
    }
}

fn expect_confirmation(message: PairingMessage) -> Result<Confirmation, PairingError> {
    match message {
        PairingMessage::Confirm(confirmation) => Ok(confirmation),
        other => Err(PairingError::UnexpectedMessage {
            expected: MessageKind::Confirm,
            actual: other.kind(),
        }),
    }
}
