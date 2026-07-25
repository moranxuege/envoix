//! Invitation-bound SPAKE2 control handshake and key confirmation.
//!
//! The invitation joiner is always the initiator and the creator is always the
//! responder. HMAC-SHA256 confirmation binds the selected bootstrap, locator,
//! transfer roles, both nonces, and both Ed25519 SPAKE2 contributions.

use envoix_invite::{Commitment, InvitationControlContext};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::PairingError;

const DOMAIN: &[u8] = b"envoix-invite-control-spake2-v2";
const INITIATOR_ID: &[u8] = b"envoix invitation joiner";
const RESPONDER_ID: &[u8] = b"envoix invitation creator";
const INITIATOR_CONFIRM_LABEL: &[u8] = b"joiner-confirm";
const RESPONDER_CONFIRM_LABEL: &[u8] = b"creator-confirm";
const NONCE_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// Invitation joiner's opening message.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PakeStart {
    pub nonce: Vec<u8>,
    pub msg: Vec<u8>,
}

/// Invitation creator's reply.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PakeResponse {
    pub nonce: Vec<u8>,
    pub msg: Vec<u8>,
}

/// A role-separated HMAC-SHA256 confirmation proof.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Confirm {
    pub mac: Vec<u8>,
}

/// Confirmed control-plane key and its authenticated PAKE transcript hash.
pub struct Paired {
    key: Vec<u8>,
    transcript_hash: Commitment,
}

impl Paired {
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn transcript_hash(&self) -> Commitment {
        self.transcript_hash
    }
}

fn random_nonce() -> Result<Vec<u8>, PairingError> {
    let mut nonce = vec![0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| PairingError::Entropy)?;
    Ok(nonce)
}

fn transcript(
    context: &InvitationControlContext,
    initiator: &PakeStart,
    responder: &PakeResponse,
) -> Vec<u8> {
    let binding = context.framed_binding();
    let mut output = Vec::new();
    for part in [
        DOMAIN,
        INITIATOR_ID,
        RESPONDER_ID,
        binding.as_slice(),
        &initiator.nonce,
        &responder.nonce,
        &initiator.msg,
        &responder.msg,
    ] {
        append_len_prefixed(&mut output, part);
    }
    output
}

fn proof(key: &[u8], transcript: &[u8], label: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(transcript);
    update_len_prefixed(&mut mac, label);
    mac.finalize().into_bytes().to_vec()
}

fn verify(
    key: &[u8],
    transcript: &[u8],
    label: &[u8],
    received: &[u8],
) -> Result<(), PairingError> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(transcript);
    update_len_prefixed(&mut mac, label);
    mac.verify_slice(received)
        .map_err(|_| PairingError::Confirm)
}

/// Begin pairing as the invitation joiner.
pub fn initiator_start(
    password: &str,
    context: &InvitationControlContext,
) -> Result<(InitiatorPending, PakeStart), PairingError> {
    let nonce = random_nonce()?;
    let (spake, msg) = Spake2::<Ed25519Group>::start_a(
        &Password::new(password.as_bytes()),
        &Identity::new(INITIATOR_ID),
        &Identity::new(RESPONDER_ID),
    );
    let start = PakeStart { nonce, msg };
    Ok((
        InitiatorPending {
            spake,
            start: start.clone(),
            context: context.clone(),
        },
        start,
    ))
}

pub struct InitiatorPending {
    spake: Spake2<Ed25519Group>,
    start: PakeStart,
    context: InvitationControlContext,
}

impl InitiatorPending {
    pub fn finish(
        self,
        response: &PakeResponse,
    ) -> Result<(InitiatorConfirming, Confirm), PairingError> {
        if response.nonce.len() != NONCE_LEN {
            return Err(PairingError::BadMessage("creator nonce length".into()));
        }
        let key = self
            .spake
            .finish(&response.msg)
            .map_err(|error| PairingError::Spake2(format!("{error:?}")))?;
        let transcript = transcript(&self.context, &self.start, response);
        let mac = proof(&key, &transcript, INITIATOR_CONFIRM_LABEL);
        Ok((InitiatorConfirming { key, transcript }, Confirm { mac }))
    }
}

pub struct InitiatorConfirming {
    key: Vec<u8>,
    transcript: Vec<u8>,
}

impl InitiatorConfirming {
    pub fn verify(self, creator_confirm: &Confirm) -> Result<Paired, PairingError> {
        verify(
            &self.key,
            &self.transcript,
            RESPONDER_CONFIRM_LABEL,
            &creator_confirm.mac,
        )?;
        Ok(Paired {
            key: self.key,
            transcript_hash: Commitment::sha256(&self.transcript),
        })
    }
}

/// Respond as the invitation creator.
pub fn responder_respond(
    password: &str,
    context: &InvitationControlContext,
    start: &PakeStart,
) -> Result<(ResponderConfirming, PakeResponse), PairingError> {
    if start.nonce.len() != NONCE_LEN {
        return Err(PairingError::BadMessage("joiner nonce length".into()));
    }
    let nonce = random_nonce()?;
    let (spake, msg) = Spake2::<Ed25519Group>::start_b(
        &Password::new(password.as_bytes()),
        &Identity::new(INITIATOR_ID),
        &Identity::new(RESPONDER_ID),
    );
    let key = spake
        .finish(&start.msg)
        .map_err(|error| PairingError::Spake2(format!("{error:?}")))?;
    let response = PakeResponse { nonce, msg };
    let transcript = transcript(context, start, &response);
    Ok((ResponderConfirming { key, transcript }, response))
}

pub struct ResponderConfirming {
    key: Vec<u8>,
    transcript: Vec<u8>,
}

impl ResponderConfirming {
    pub fn verify(self, joiner_confirm: &Confirm) -> Result<(Paired, Confirm), PairingError> {
        verify(
            &self.key,
            &self.transcript,
            INITIATOR_CONFIRM_LABEL,
            &joiner_confirm.mac,
        )?;
        let mac = proof(&self.key, &self.transcript, RESPONDER_CONFIRM_LABEL);
        Ok((
            Paired {
                key: self.key,
                transcript_hash: Commitment::sha256(&self.transcript),
            },
            Confirm { mac },
        ))
    }
}

fn append_len_prefixed(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

fn update_len_prefixed(mac: &mut HmacSha256, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
