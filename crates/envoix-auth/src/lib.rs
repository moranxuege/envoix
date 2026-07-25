//! Data-plane peer authentication: SPAKE2 run over the live peer connection and
//! bound to it - the transcript is confirmed with HMAC-SHA256 keyed by the QUIC
//! TLS exporter, so a man-in-the-middle on the transport cannot pass.
//!
//! Distinct from `envoix-pairing`, which runs the same SPAKE2 primitive on the
//! *control* plane - over the untrusted rendezvous mailbox, before any
//! connection - to exchange sealed peer descriptors. Same code-to-key primitive,
//! two planes: this one binds to the channel it authenticates.

use envoix_error::CoreError;
use envoix_invite::{InvitationAuthContext, SecretString, TransferRole};
use envoix_protocol::{
    AuthFrame, Frame, FrameConnection, Spake2Confirm, Spake2Message, Spake2Start,
};
use envoix_types::{PROTOCOL_VERSION, PeerRole, is_valid_shared_token};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub use envoix_types::MIN_SHARED_TOKEN_LEN;

/// Domain label used for SPAKE2 transcript and QUIC exporter binding.
pub const SPAKE2_DOMAIN: &[u8] = b"envoix-auth-spake2-v1";
/// V2 exporter label and transcript domain.
pub const INVITE_V2_SPAKE2_DOMAIN: &[u8] = b"envoix-auth-invite-v2";

/// User-facing warning for the current SPAKE2 backend.
pub const SPAKE2_EXPERIMENTAL_WARNING: &str = "warning: SPAKE2 shared-token pairing is experimental; the Rust SPAKE2 dependency is not independently audited";

const NONCE_LEN: usize = 32;
const SENDER_IDENTITY: &[u8] = b"envoix sender";
const RECEIVER_IDENTITY: &[u8] = b"envoix receiver";
const EXPORTER_CONTEXT: &[u8] = b"pairing";
const SENDER_CONFIRM_LABEL: &[u8] = b"sender-confirm";
const RECEIVER_CONFIRM_LABEL: &[u8] = b"receiver-confirm";
const REMEMBER_COMBINE_DOMAIN: &[u8] = b"envoix invite v2 remember combine";

type HmacSha256 = Hmac<Sha256>;

/// Error type returned by pairing authentication.
pub type AuthError = CoreError;

/// Fresh result of a mutually-consented Remember negotiation.
#[derive(Clone, Eq, PartialEq)]
pub struct RememberSecret([u8; 32]);

impl RememberSecret {
    /// Consume the value into platform-owned secure storage.
    pub fn into_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl std::fmt::Debug for RememberSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RememberSecret(<redacted>)")
    }
}

/// Result returned after exporter-bound data authentication.
#[derive(Debug, Eq, PartialEq)]
pub struct AuthenticationOutcome {
    pub remember_secret: Option<RememberSecret>,
}

/// Pairing method selected for a session.
#[derive(Clone, Eq, PartialEq)]
pub enum PairingConfig {
    /// Experimental SPAKE2 pairing using a shared ASCII token.
    Spake2SharedToken {
        /// Shared token known to both peers.
        token: String,
    },
    /// Directional InviteV2 authentication with an immutable transcript
    /// binding and a derived, redacted password.
    InvitationV2 {
        password: SecretString,
        context: InvitationAuthContext,
    },
}

impl std::fmt::Debug for PairingConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spake2SharedToken { .. } => formatter
                .debug_struct("Spake2SharedToken")
                .field("token", &"<redacted>")
                .finish(),
            Self::InvitationV2 { context, .. } => formatter
                .debug_struct("InvitationV2")
                .field("password", &"<redacted>")
                .field("context", context)
                .finish(),
        }
    }
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

    pub fn invitation_v2(
        password: SecretString,
        context: InvitationAuthContext,
    ) -> Result<Self, AuthError> {
        let config = Self::InvitationV2 { password, context };
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
            Self::InvitationV2 { password, context }
                if is_valid_shared_token(password.expose())
                    && context.creator_transfer_role.complement()
                        == context.joiner_transfer_role =>
            {
                Ok(())
            }
            Self::InvitationV2 { .. } => Err(CoreError::InvalidInput(
                "invalid InviteV2 authentication context".into(),
            )),
        }
    }
}

/// Authenticates the sender side before any transfer frames are sent.
pub async fn authenticate_sender(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
) -> Result<(), AuthError> {
    authenticate_sender_with_remember(connection, config, false)
        .await
        .map(|_| ())
}

/// Authenticate the sender and optionally negotiate a fresh Remember value
/// inside the existing confirmation exchange.
pub async fn authenticate_sender_with_remember(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
    remember_consent: bool,
) -> Result<AuthenticationOutcome, AuthError> {
    config.validate()?;
    validate_data_role(config, TransferRole::Sender)?;
    let remember_consent = remember_consent && matches!(config, PairingConfig::InvitationV2 { .. });
    let profile = auth_profile(config);
    let exporter =
        connection.export_keying_material(profile.domain, profile.exporter_context.as_slice())?;
    let sender_nonce = random_nonce()?;
    let (state, sender_message) = Spake2::<Ed25519Group>::start_a(
        &Password::new(profile.password.as_bytes()),
        &Identity::new(SENDER_IDENTITY),
        &Identity::new(RECEIVER_IDENTITY),
    );

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Start(Spake2Start {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
            nonce: sender_nonce.to_vec(),
            message: sender_message.clone(),
            remember_consent,
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
        domain: profile.domain,
        invitation_binding: profile.invitation_binding.as_slice(),
        sender_remember_consent: remember_consent,
        receiver_remember_consent: response.remember_consent,
    };
    let mutual_remember = remember_consent && response.remember_consent;
    let sender_contribution = mutual_remember.then(random_nonce).transpose()?;
    let sender_proof = confirmation_proof_with_contributions(
        &shared_key,
        &transcript,
        SENDER_CONFIRM_LABEL,
        sender_contribution.as_ref(),
        None,
    );

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Confirm(Spake2Confirm {
            proof: sender_proof,
            remember_contribution: sender_contribution.map(Vec::from),
        })))
        .await?;

    let receiver_confirm = expect_spake2_confirm(connection.recv_frame().await?)?;
    let receiver_contribution =
        validate_remember_contribution(mutual_remember, receiver_confirm.remember_contribution)?;
    verify_confirmation_with_contributions(
        &shared_key,
        &transcript,
        RECEIVER_CONFIRM_LABEL,
        &receiver_confirm.proof,
        sender_contribution.as_ref(),
        receiver_contribution.as_ref(),
    )?;
    Ok(AuthenticationOutcome {
        remember_secret: combine_remember(
            mutual_remember,
            sender_contribution.as_ref(),
            receiver_contribution.as_ref(),
            profile.invitation_binding.as_slice(),
        ),
    })
}

/// Authenticates the receiver side before any transfer frames are accepted.
pub async fn authenticate_receiver(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
) -> Result<(), AuthError> {
    authenticate_receiver_with_remember(connection, config, false)
        .await
        .map(|_| ())
}

/// Authenticate the receiver and optionally negotiate a fresh Remember value
/// inside the existing confirmation exchange.
pub async fn authenticate_receiver_with_remember(
    connection: &mut dyn FrameConnection,
    config: &PairingConfig,
    remember_consent: bool,
) -> Result<AuthenticationOutcome, AuthError> {
    config.validate()?;
    validate_data_role(config, TransferRole::Receiver)?;
    let remember_consent = remember_consent && matches!(config, PairingConfig::InvitationV2 { .. });
    let profile = auth_profile(config);
    let exporter =
        connection.export_keying_material(profile.domain, profile.exporter_context.as_slice())?;
    let start = expect_spake2_start(connection.recv_frame().await?)?;
    validate_start(&start)?;

    let receiver_nonce = random_nonce()?;
    let (state, receiver_message) = Spake2::<Ed25519Group>::start_b(
        &Password::new(profile.password.as_bytes()),
        &Identity::new(SENDER_IDENTITY),
        &Identity::new(RECEIVER_IDENTITY),
    );

    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Message(Spake2Message {
            nonce: receiver_nonce.to_vec(),
            message: receiver_message.clone(),
            remember_consent,
        })))
        .await?;

    let shared_key = finish_spake2(state, &start.message)?;
    let transcript = ConfirmationTranscript {
        sender_nonce: &start.nonce,
        receiver_nonce: &receiver_nonce,
        sender_message: &start.message,
        receiver_message: &receiver_message,
        exporter: &exporter,
        domain: profile.domain,
        invitation_binding: profile.invitation_binding.as_slice(),
        sender_remember_consent: start.remember_consent,
        receiver_remember_consent: remember_consent,
    };

    let sender_confirm = expect_spake2_confirm(connection.recv_frame().await?)?;
    let mutual_remember = start.remember_consent && remember_consent;
    let sender_contribution =
        validate_remember_contribution(mutual_remember, sender_confirm.remember_contribution)?;
    verify_confirmation_with_contributions(
        &shared_key,
        &transcript,
        SENDER_CONFIRM_LABEL,
        &sender_confirm.proof,
        sender_contribution.as_ref(),
        None,
    )?;

    let receiver_contribution = mutual_remember.then(random_nonce).transpose()?;
    let receiver_proof = confirmation_proof_with_contributions(
        &shared_key,
        &transcript,
        RECEIVER_CONFIRM_LABEL,
        sender_contribution.as_ref(),
        receiver_contribution.as_ref(),
    );
    connection
        .send_frame(Frame::Auth(AuthFrame::Spake2Confirm(Spake2Confirm {
            proof: receiver_proof,
            remember_contribution: receiver_contribution.map(Vec::from),
        })))
        .await?;

    Ok(AuthenticationOutcome {
        remember_secret: combine_remember(
            mutual_remember,
            sender_contribution.as_ref(),
            receiver_contribution.as_ref(),
            profile.invitation_binding.as_slice(),
        ),
    })
}

struct AuthProfile<'a> {
    password: &'a str,
    domain: &'static [u8],
    exporter_context: Vec<u8>,
    invitation_binding: Vec<u8>,
}

fn auth_profile(config: &PairingConfig) -> AuthProfile<'_> {
    match config {
        PairingConfig::Spake2SharedToken { token } => AuthProfile {
            password: token,
            domain: SPAKE2_DOMAIN,
            exporter_context: EXPORTER_CONTEXT.to_vec(),
            invitation_binding: Vec::new(),
        },
        PairingConfig::InvitationV2 { password, context } => {
            let binding = context.framed_binding();
            AuthProfile {
                password: password.expose(),
                domain: INVITE_V2_SPAKE2_DOMAIN,
                exporter_context: binding.clone(),
                invitation_binding: binding,
            }
        }
    }
}

fn validate_data_role(config: &PairingConfig, local_role: TransferRole) -> Result<(), AuthError> {
    let PairingConfig::InvitationV2 { context, .. } = config else {
        return Ok(());
    };
    if context.creator_transfer_role == local_role || context.joiner_transfer_role == local_role {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(
            "InviteV2 authentication role conflict".into(),
        ))
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
    domain: &'a [u8],
    invitation_binding: &'a [u8],
    sender_remember_consent: bool,
    receiver_remember_consent: bool,
}

fn confirmation_proof_with_contributions(
    shared_key: &[u8],
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
    sender_contribution: Option<&[u8; NONCE_LEN]>,
    receiver_contribution: Option<&[u8; NONCE_LEN]>,
) -> Vec<u8> {
    let mut mac = confirmation_mac(shared_key, transcript, proof_label);
    update_optional_contribution(&mut mac, sender_contribution);
    update_optional_contribution(&mut mac, receiver_contribution);
    mac.finalize().into_bytes().to_vec()
}

fn verify_confirmation_with_contributions(
    shared_key: &[u8],
    transcript: &ConfirmationTranscript<'_>,
    proof_label: &[u8],
    received_proof: &[u8],
    sender_contribution: Option<&[u8; NONCE_LEN]>,
    receiver_contribution: Option<&[u8; NONCE_LEN]>,
) -> Result<(), AuthError> {
    let mut mac = confirmation_mac(shared_key, transcript, proof_label);
    update_optional_contribution(&mut mac, sender_contribution);
    update_optional_contribution(&mut mac, receiver_contribution);
    mac.verify_slice(received_proof)
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
    update_len_prefixed(mac, transcript.domain);
    mac.update(&PROTOCOL_VERSION.to_be_bytes());
    update_len_prefixed(mac, SENDER_IDENTITY);
    update_len_prefixed(mac, RECEIVER_IDENTITY);
    update_len_prefixed(mac, transcript.sender_nonce);
    update_len_prefixed(mac, transcript.receiver_nonce);
    update_len_prefixed(mac, transcript.sender_message);
    update_len_prefixed(mac, transcript.receiver_message);
    update_len_prefixed(mac, transcript.exporter);
    update_len_prefixed(mac, transcript.invitation_binding);
    mac.update(&[
        u8::from(transcript.sender_remember_consent),
        u8::from(transcript.receiver_remember_consent),
    ]);
    update_len_prefixed(mac, proof_label);
}

fn update_optional_contribution(mac: &mut HmacSha256, value: Option<&[u8; NONCE_LEN]>) {
    update_len_prefixed(
        mac,
        value.map(<[u8; NONCE_LEN]>::as_slice).unwrap_or_default(),
    );
}

fn validate_remember_contribution(
    expected: bool,
    value: Option<Vec<u8>>,
) -> Result<Option<[u8; NONCE_LEN]>, AuthError> {
    match (expected, value) {
        (false, None) => Ok(None),
        (true, Some(value)) if value.len() == NONCE_LEN => {
            Ok(Some(value.try_into().expect("length checked")))
        }
        _ => Err(CoreError::Protocol(
            "invalid Remember contribution in authentication confirmation".into(),
        )),
    }
}

fn combine_remember(
    mutual: bool,
    sender: Option<&[u8; NONCE_LEN]>,
    receiver: Option<&[u8; NONCE_LEN]>,
    invitation_binding: &[u8],
) -> Option<RememberSecret> {
    if !mutual {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(REMEMBER_COMBINE_DOMAIN);
    hash.update(sender.expect("mutual consent requires sender contribution"));
    hash.update(receiver.expect("mutual consent requires receiver contribution"));
    hash.update(invitation_binding);
    Some(RememberSecret(hash.finalize().into()))
}

fn update_len_prefixed(mac: &mut HmacSha256, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}
