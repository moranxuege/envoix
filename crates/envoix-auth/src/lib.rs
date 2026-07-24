//! Data-plane peer authentication: SPAKE2 run over the live peer connection and
//! bound to it - the transcript is confirmed with HMAC-SHA256 keyed by the QUIC
//! TLS exporter, so a man-in-the-middle on the transport cannot pass.
//!
//! Distinct from `envoix-pairing`, which runs the same SPAKE2 primitive on the
//! *control* plane - over the untrusted rendezvous mailbox, before any
//! connection - to exchange sealed peer descriptors. Same code-to-key primitive,
//! two planes: this one binds to the channel it authenticates.

use envoix_error::CoreError;
use envoix_protocol::{
    AuthFrame, Frame, FrameConnection, Spake2Confirm, Spake2Message, Spake2Start,
};
use envoix_types::{PROTOCOL_VERSION, PeerRole, is_valid_shared_token};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub use envoix_types::MIN_SHARED_TOKEN_LEN;

/// Domain label used for SPAKE2 transcript and QUIC exporter binding.
pub const SPAKE2_DOMAIN: &[u8] = b"envoix-auth-spake2-v1";

/// User-facing warning for the current SPAKE2 backend.
pub const SPAKE2_EXPERIMENTAL_WARNING: &str = "warning: SPAKE2 shared-token pairing is experimental; the Rust SPAKE2 dependency is not independently audited";

const NONCE_LEN: usize = 32;
const SENDER_IDENTITY: &[u8] = b"envoix sender";
const RECEIVER_IDENTITY: &[u8] = b"envoix receiver";
const EXPORTER_CONTEXT: &[u8] = b"pairing";
const SENDER_CONFIRM_LABEL: &[u8] = b"sender-confirm";
const RECEIVER_CONFIRM_LABEL: &[u8] = b"receiver-confirm";

type HmacSha256 = Hmac<Sha256>;

/// Error type returned by pairing authentication.
pub type AuthError = CoreError;

/// Pairing method selected for a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingConfig {
    /// Experimental SPAKE2 pairing using a shared ASCII token.
    Spake2SharedToken {
        /// Shared token known to both peers.
        token: String,
    },
}

impl PairingConfig {
    /// Creates a validated experimental SPAKE2 shared-token config.
    pub fn spake2_shared_token(token: impl Into<String>) -> Result<Self, AuthError> {
        let config = Self::Spake2SharedToken {
            token: token.into(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates pairing config invariants that are independent of transport.
    pub fn validate(&self) -> Result<(), AuthError> {
        match self {
            Self::Spake2SharedToken { token } if is_valid_shared_token(token) => Ok(()),
            Self::Spake2SharedToken { .. } => Err(CoreError::InvalidInput(format!(
                "SPAKE2 shared token must be at least {MIN_SHARED_TOKEN_LEN} ASCII bytes"
            ))),
        }
    }
}

/// Authenticates the sender side before any transfer frames are sent.
pub async fn authenticate_sender(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
) -> Result<(), AuthError> {
    config.validate()?;
    let token = shared_token(config);
    let exporter = connection.export_keying_material(SPAKE2_DOMAIN, EXPORTER_CONTEXT)?;
    let sender_nonce = random_nonce()?;
    let (state, sender_message) = Spake2::<Ed25519Group>::start_a(
        &Password::new(token.as_bytes()),
        &Identity::new(SENDER_IDENTITY),
        &Identity::new(RECEIVER_IDENTITY),
    );

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Start(Spake2Start {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
            nonce: sender_nonce.to_vec(),
            message: sender_message.clone(),
        })))
        .await?;

    let response = expect_spake2_message(connection.recv_frame().await?)?;
    validate_nonce(&response.nonce)?;
    let shared_key = finish_spake2(state, &response.message)?;
    let transcript = ConfirmationTranscript {
        sender_nonce: &sender_nonce,
        receiver_nonce: &response.nonce,
        sender_message: &sender_message,
        receiver_message: &response.message,
        exporter: &exporter,
    };
    let sender_proof = confirmation_proof(&shared_key, &transcript, SENDER_CONFIRM_LABEL);

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Confirm(Spake2Confirm {
            proof: sender_proof,
        })))
        .await?;

    let receiver_confirm = expect_spake2_confirm(connection.recv_frame().await?)?;
    verify_confirmation(
        &shared_key,
        &transcript,
        RECEIVER_CONFIRM_LABEL,
        &receiver_confirm.proof,
    )
}

/// Authenticates the receiver side before any transfer frames are accepted.
pub async fn authenticate_receiver(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
) -> Result<(), AuthError> {
    config.validate()?;
    let token = shared_token(config);
    let exporter = connection.export_keying_material(SPAKE2_DOMAIN, EXPORTER_CONTEXT)?;
    let start = expect_spake2_start(connection.recv_frame().await?)?;
    validate_start(&start)?;

    let receiver_nonce = random_nonce()?;
    let (state, receiver_message) = Spake2::<Ed25519Group>::start_b(
        &Password::new(token.as_bytes()),
        &Identity::new(SENDER_IDENTITY),
        &Identity::new(RECEIVER_IDENTITY),
    );

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Message(Spake2Message {
            nonce: receiver_nonce.to_vec(),
            message: receiver_message.clone(),
        })))
        .await?;

    let shared_key = finish_spake2(state, &start.message)?;
    let transcript = ConfirmationTranscript {
        sender_nonce: &start.nonce,
        receiver_nonce: &receiver_nonce,
        sender_message: &start.message,
        receiver_message: &receiver_message,
        exporter: &exporter,
    };

    let sender_confirm = expect_spake2_confirm(connection.recv_frame().await?)?;
    verify_confirmation(
        &shared_key,
        &transcript,
        SENDER_CONFIRM_LABEL,
        &sender_confirm.proof,
    )?;

    let receiver_proof = confirmation_proof(&shared_key, &transcript, RECEIVER_CONFIRM_LABEL);
    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Confirm(Spake2Confirm {
            proof: receiver_proof,
        })))
        .await?;

    Ok(())
}

fn shared_token(config: &PairingConfig) -> &str {
    match config {
        PairingConfig::Spake2SharedToken { token } => token,
    }
}

fn random_nonce() -> Result<[u8; NONCE_LEN], AuthError> {
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|error| CoreError::Crypto(error.to_string()))?;
    Ok(nonce)
}

fn finish_spake2(state: Spake2<Ed25519Group>, peer_message: &[u8]) -> Result<Vec<u8>, AuthError> {
    state
        .finish(peer_message)
        .map_err(|error| CoreError::Crypto(format!("SPAKE2 failed: {error:?}")))
}

fn expect_spake2_start(frame: Frame) -> Result<Spake2Start, AuthError> {
    match frame {
        Frame::Auth(AuthFrame::Spake2Start(start)) => Ok(start),
        frame => Err(CoreError::Protocol(format!(
            "expected SPAKE2 start, got {frame:?}"
        ))),
    }
}

fn expect_spake2_message(frame: Frame) -> Result<Spake2Message, AuthError> {
    match frame {
        Frame::Auth(AuthFrame::Spake2Message(message)) => Ok(message),
        frame => Err(CoreError::Protocol(format!(
            "expected SPAKE2 message, got {frame:?}"
        ))),
    }
}

fn expect_spake2_confirm(frame: Frame) -> Result<Spake2Confirm, AuthError> {
    match frame {
        Frame::Auth(AuthFrame::Spake2Confirm(confirm)) => Ok(confirm),
        frame => Err(CoreError::Protocol(format!(
            "expected SPAKE2 confirmation, got {frame:?}"
        ))),
    }
}

fn validate_start(start: &Spake2Start) -> Result<(), AuthError> {
    if start.protocol_version != PROTOCOL_VERSION {
        return Err(CoreError::Protocol(format!(
            "unsupported auth protocol version {}",
            start.protocol_version
        )));
    }
    if start.role != PeerRole::Sender {
        return Err(CoreError::Protocol(format!(
            "expected sender SPAKE2 role, got {:?}",
            start.role
        )));
    }
    validate_nonce(&start.nonce)
}

fn validate_nonce(nonce: &[u8]) -> Result<(), AuthError> {
    if nonce.len() != NONCE_LEN {
        return Err(CoreError::Protocol(format!(
            "SPAKE2 nonce must be {NONCE_LEN} bytes"
        )));
    }
    Ok(())
}

struct ConfirmationTranscript<'a> {
    sender_nonce: &'a [u8],
    receiver_nonce: &'a [u8],
    sender_message: &'a [u8],
    receiver_message: &'a [u8],
    exporter: &'a [u8],
}

fn confirmation_proof(
    shared_key: &[u8],
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
) -> Vec<u8> {
    confirmation_mac(shared_key, transcript, proof_label)
        .finalize()
        .into_bytes()
        .to_vec()
}

fn verify_confirmation(
    shared_key: &[u8],
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
    received_proof: &[u8],
) -> Result<(), AuthError> {
    confirmation_mac(shared_key, transcript, proof_label)
        .verify_slice(received_proof)
        .map_err(|_| CoreError::Crypto("SPAKE2 confirmation proof mismatch".into()))
}

fn confirmation_mac(
    shared_key: &[u8],
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
) -> HmacSha256 {
    let mut mac =
        HmacSha256::new_from_slice(shared_key).expect("HMAC-SHA256 accepts keys of any length");
    update_confirmation_mac(&mut mac, transcript, proof_label);
    mac
}

fn update_confirmation_mac(
    mac: &mut HmacSha256,
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
) {
    update_len_prefixed(mac, SPAKE2_DOMAIN);
    mac.update(&PROTOCOL_VERSION.to_be_bytes());
    update_len_prefixed(mac, SENDER_IDENTITY);
    update_len_prefixed(mac, RECEIVER_IDENTITY);
    update_len_prefixed(mac, transcript.sender_nonce);
    update_len_prefixed(mac, transcript.receiver_nonce);
    update_len_prefixed(mac, transcript.sender_message);
    update_len_prefixed(mac, transcript.receiver_message);
    update_len_prefixed(mac, transcript.exporter);
    update_len_prefixed(mac, proof_label);
}

fn update_len_prefixed(mac: &mut HmacSha256, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}
