//! Data-plane peer authentication: SPAKE2 run over the live peer connection and
//! bound to it - the transcript is confirmed with HMAC-SHA256 keyed by the QUIC
//! TLS exporter, so a man-in-the-middle on the transport cannot pass.
//!
//! Distinct from `envoix-pairing`, which runs the same SPAKE2 primitive on the
//! *control* plane - over the untrusted rendezvous mailbox, before any
//! connection - to exchange sealed peer descriptors. Same code-to-key primitive,
//! two planes: this one binds to the channel it authenticates.

use base64::Engine as _;
use envoix_error::CoreError;
use envoix_invite::{
    Commitment, InvitationAuthContext, InvitationControlContext, SecretString, TransferRole,
};
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
/// Remembered-session exporter label and transcript domain.
pub const REMEMBERED_V1_SPAKE2_DOMAIN: &[u8] = b"envoix-auth-remembered-v1";

/// User-facing warning for the current SPAKE2 backend.
pub const SPAKE2_EXPERIMENTAL_WARNING: &str = "warning: SPAKE2 shared-token pairing is experimental; the Rust SPAKE2 dependency is not independently audited";

const NONCE_LEN: usize = 32;
const SENDER_IDENTITY: &[u8] = b"envoix sender";
const RECEIVER_IDENTITY: &[u8] = b"envoix receiver";
const EXPORTER_CONTEXT: &[u8] = b"pairing";
const SENDER_CONFIRM_LABEL: &[u8] = b"sender-confirm";
const RECEIVER_CONFIRM_LABEL: &[u8] = b"receiver-confirm";
const REMEMBER_COMBINE_DOMAIN: &[u8] = b"envoix invite v2 remember combine";
const REMEMBERED_CREDENTIAL_MAGIC: &[u8; 4] = b"ENVR";
const REMEMBERED_CREDENTIAL_VERSION: u8 = 1;
const REMEMBERED_CREDENTIAL_LEN: usize = 4 + 1 + 32;
const REMEMBERED_ROOM_ID_LABEL: &[u8] = b"envoix room id";
const REMEMBERED_ROOM_AUTH_LABEL: &[u8] = b"envoix room auth";
const REMEMBERED_PRESENCE_TAG_LABEL: &[u8] = b"envoix presence tag";
const REMEMBERED_DATA_AUTH_LABEL: &[u8] = b"envoix remembered data auth";
const REMEMBERED_CONTROL_PAIRING_LABEL: &[u8] = b"envoix verified device from room control";

pub const REMEMBERED_PRESENCE_TAG_PREFIX: &str = "p1_";
pub const REMEMBERED_PRESENCE_TAG_LEN: usize = REMEMBERED_PRESENCE_TAG_PREFIX.len() + 43;

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

    /// Convert the mutually negotiated value into the versioned credential
    /// owned by the Rust core.
    pub fn into_credential(self) -> RememberedCredential {
        RememberedCredential { secret: self.0 }
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

/// Versioned long-term secret for one remembered relationship.
#[derive(Clone, Eq, PartialEq)]
pub struct RememberedCredential {
    secret: [u8; 32],
}

impl RememberedCredential {
    /// Derive a long-term relationship credential from an already confirmed
    /// Room-control PAKE. The transcript binding makes independently completed
    /// pairings distinct even when users happen to enter the same short code.
    pub fn from_control_pairing(control_key: &[u8], transcript: Commitment) -> Self {
        let mut mac = HmacSha256::new_from_slice(control_key)
            .expect("HMAC-SHA256 accepts keys of any length");
        update_len_prefixed(&mut mac, REMEMBERED_CONTROL_PAIRING_LABEL);
        update_len_prefixed(&mut mac, transcript.as_bytes());
        Self {
            secret: mac.finalize().into_bytes().into(),
        }
    }

    /// Parse the opaque credential bytes loaded by a platform secure store.
    pub fn from_opaque(bytes: &[u8]) -> Result<Self, AuthError> {
        if bytes.len() != REMEMBERED_CREDENTIAL_LEN
            || &bytes[..REMEMBERED_CREDENTIAL_MAGIC.len()] != REMEMBERED_CREDENTIAL_MAGIC
        {
            return Err(CoreError::Storage(
                "remembered credential is corrupt or unsupported".into(),
            ));
        }
        if bytes[REMEMBERED_CREDENTIAL_MAGIC.len()] != REMEMBERED_CREDENTIAL_VERSION {
            return Err(CoreError::Storage(
                "remembered credential version is unsupported".into(),
            ));
        }
        let secret = bytes[REMEMBERED_CREDENTIAL_MAGIC.len() + 1..]
            .try_into()
            .expect("credential length checked");
        Ok(Self { secret })
    }

    /// Serialize for opaque platform storage. Callers must never decode or log
    /// the returned bytes.
    pub fn to_opaque(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(REMEMBERED_CREDENTIAL_LEN);
        output.extend_from_slice(REMEMBERED_CREDENTIAL_MAGIC);
        output.push(REMEMBERED_CREDENTIAL_VERSION);
        output.extend_from_slice(&self.secret);
        output
    }

    /// Derive the unlinkable locator and control authenticator for one
    /// credential generation.
    pub fn derive_session(&self, generation: u64) -> RememberedSession {
        let room_id =
            derive_remembered_value(&self.secret, generation, REMEMBERED_ROOM_ID_LABEL, &[]);
        let room_auth =
            derive_remembered_value(&self.secret, generation, REMEMBERED_ROOM_AUTH_LABEL, &[]);
        RememberedSession {
            generation,
            room_id: format!(
                "r1_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(room_id)
            ),
            room_auth,
        }
    }

    /// Derive a generation-scoped presence tag safe to publish on an
    /// untrusted local discovery carrier.
    ///
    /// The tag is domain-separated from the hidden broker locator and
    /// authenticator. It proves no identity or room ownership by itself.
    pub fn derive_presence_tag(&self, generation: u64) -> String {
        let tag =
            derive_remembered_value(&self.secret, generation, REMEMBERED_PRESENCE_TAG_LABEL, &[]);
        format!(
            "{REMEMBERED_PRESENCE_TAG_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag)
        )
    }
}

impl std::fmt::Debug for RememberedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RememberedCredential(<redacted>)")
    }
}

/// Per-generation remembered rendezvous material.
#[derive(Clone, Eq, PartialEq)]
pub struct RememberedSession {
    generation: u64,
    room_id: String,
    room_auth: [u8; 32],
}

impl RememberedSession {
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    pub fn control_context(&self) -> Result<InvitationControlContext, AuthError> {
        InvitationControlContext::remembered(self.room_id.clone())
            .map_err(|error| CoreError::InvalidInput(error.to_string()))
    }

    /// Base64 is only the SPAKE2 input encoding. It is never a user-facing or
    /// durable representation.
    pub fn control_password(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.room_auth)
    }

    /// Derive a fresh data-plane password from the authenticated control
    /// exchange, then bind it to the remembered generation and broker.
    pub fn finish_pairing(
        &self,
        broker: String,
        control_key: &[u8],
        control_transcript_hash: Commitment,
    ) -> Result<PairingConfig, AuthError> {
        let context = RememberedAuthContext {
            generation: self.generation,
            room_id: self.room_id.clone(),
            broker,
            control_transcript_hash,
        };
        let binding = context.framed_binding();
        let mut extra = Vec::new();
        append_len_prefixed(&mut extra, control_key);
        append_len_prefixed(&mut extra, control_transcript_hash.as_bytes());
        append_len_prefixed(&mut extra, &binding);
        let password =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(derive_remembered_value(
                &self.room_auth,
                self.generation,
                REMEMBERED_DATA_AUTH_LABEL,
                &extra,
            ));
        PairingConfig::remembered_v1(password, context)
    }
}

impl std::fmt::Debug for RememberedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RememberedSession")
            .field("generation", &self.generation)
            .field("room_id", &"<redacted>")
            .field("room_auth", &"<redacted>")
            .finish()
    }
}

/// Immutable data-plane binding for a remembered session.
#[derive(Clone, Eq, PartialEq)]
pub struct RememberedAuthContext {
    pub generation: u64,
    pub room_id: String,
    pub broker: String,
    pub control_transcript_hash: Commitment,
}

impl RememberedAuthContext {
    pub fn framed_binding(&self) -> Vec<u8> {
        let mut output = Vec::new();
        append_len_prefixed(&mut output, b"remembered-peer-v1");
        output.extend_from_slice(&self.generation.to_be_bytes());
        append_len_prefixed(&mut output, self.room_id.as_bytes());
        append_len_prefixed(&mut output, self.broker.as_bytes());
        append_len_prefixed(&mut output, b"receiver-responder");
        append_len_prefixed(&mut output, b"sender-initiator");
        append_len_prefixed(&mut output, self.control_transcript_hash.as_bytes());
        output
    }
}

impl std::fmt::Debug for RememberedAuthContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RememberedAuthContext")
            .field("generation", &self.generation)
            .field("room_id", &"<redacted>")
            .field("broker", &self.broker)
            .field("control_transcript_hash", &"<redacted>")
            .finish()
    }
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
    /// Remembered-device authentication, freshly derived for one generation
    /// and the authenticated control exchange that disclosed the endpoint.
    RememberedV1 {
        password: String,
        context: RememberedAuthContext,
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
            Self::RememberedV1 { context, .. } => formatter
                .debug_struct("RememberedV1")
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

    pub fn remembered_v1(
        password: String,
        context: RememberedAuthContext,
    ) -> Result<Self, AuthError> {
        let config = Self::RememberedV1 { password, context };
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
            Self::RememberedV1 { password, context }
                if is_valid_shared_token(password)
                    && context.room_id.starts_with("r1_")
                    && !context.broker.is_empty() =>
            {
                Ok(())
            }
            Self::RememberedV1 { .. } => Err(CoreError::InvalidInput(
                "invalid remembered-device authentication context".into(),
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
        PairingConfig::RememberedV1 { password, context } => {
            let binding = context.framed_binding();
            AuthProfile {
                password,
                domain: REMEMBERED_V1_SPAKE2_DOMAIN,
                exporter_context: binding.clone(),
                invitation_binding: binding,
            }
        }
    }
}

fn validate_data_role(config: &PairingConfig, local_role: TransferRole) -> Result<(), AuthError> {
    match config {
        PairingConfig::InvitationV2 { context, .. }
            if context.creator_transfer_role != local_role
                && context.joiner_transfer_role != local_role =>
        {
            Err(CoreError::InvalidInput(
                "InviteV2 authentication role conflict".into(),
            ))
        }
        _ => Ok(()),
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

fn append_len_prefixed(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

fn derive_remembered_value(secret: &[u8], generation: u64, label: &[u8], extra: &[u8]) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC-SHA256 accepts keys of any length");
    update_len_prefixed(&mut mac, label);
    mac.update(&generation.to_be_bytes());
    update_len_prefixed(&mut mac, extra);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use envoix_invite::{BootstrapKind, Capabilities, InviteV2};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    struct ChannelConnection {
        outgoing: UnboundedSender<Frame>,
        incoming: UnboundedReceiver<Frame>,
    }

    fn connection_pair() -> (ChannelConnection, ChannelConnection) {
        let (left_to_right, right_incoming) = unbounded_channel();
        let (right_to_left, left_incoming) = unbounded_channel();
        (
            ChannelConnection {
                outgoing: left_to_right,
                incoming: left_incoming,
            },
            ChannelConnection {
                outgoing: right_to_left,
                incoming: right_incoming,
            },
        )
    }

    #[async_trait]
    impl FrameConnection for ChannelConnection {
        async fn send_frame(&mut self, frame: Frame) -> Result<(), CoreError> {
            self.outgoing
                .send(frame)
                .map_err(|_| CoreError::Transport("test peer closed".into()))
        }

        async fn recv_frame(&mut self) -> Result<Frame, CoreError> {
            self.incoming
                .recv()
                .await
                .ok_or_else(|| CoreError::Transport("test peer closed".into()))
        }

        fn export_keying_material(
            &self,
            _label: &[u8],
            _context: &[u8],
        ) -> Result<[u8; 32], CoreError> {
            Ok([0x91; 32])
        }

        async fn close(&mut self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn invitation_config() -> PairingConfig {
        let bootstrap = InviteV2::create(
            "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya@127.0.0.1:8445".into(),
            Vec::new(),
            TransferRole::Receiver,
            Capabilities::current(),
            1,
        )
        .expect("create invitation")
        .into_bootstrap();
        let (password, context) = bootstrap
            .finish_control(
                BootstrapKind::FullTicket,
                None,
                &[0x33; 32],
                Commitment::sha256(b"control transcript"),
                1,
            )
            .expect("finish control");
        PairingConfig::invitation_v2(password, context).expect("pairing config")
    }

    fn credential() -> RememberedCredential {
        RememberSecret([0x5a; 32]).into_credential()
    }

    #[test]
    fn opaque_credential_round_trips_without_debug_disclosure() {
        let credential = credential();
        let opaque = credential.to_opaque();
        assert_eq!(
            RememberedCredential::from_opaque(&opaque).expect("decode credential"),
            credential
        );
        assert_eq!(
            format!("{credential:?}"),
            "RememberedCredential(<redacted>)"
        );
        assert!(!format!("{credential:?}").contains("5a"));
    }

    #[test]
    fn opaque_credential_rejects_wrong_version_and_size() {
        let mut opaque = credential().to_opaque();
        opaque[4] = 2;
        assert!(RememberedCredential::from_opaque(&opaque).is_err());
        assert!(RememberedCredential::from_opaque(&opaque[..opaque.len() - 1]).is_err());
    }

    #[test]
    fn confirmed_control_pairing_derives_one_transcript_bound_credential() {
        let transcript = Commitment::sha256(b"verified room control");
        let first = RememberedCredential::from_control_pairing(&[0x41; 32], transcript);
        let same = RememberedCredential::from_control_pairing(&[0x41; 32], transcript);
        let other_key = RememberedCredential::from_control_pairing(&[0x42; 32], transcript);
        let other_transcript = RememberedCredential::from_control_pairing(
            &[0x41; 32],
            Commitment::sha256(b"different room control"),
        );

        assert_eq!(first, same);
        assert_ne!(first, other_key);
        assert_ne!(first, other_transcript);
        assert_eq!(
            RememberedCredential::from_opaque(&first.to_opaque()).unwrap(),
            first
        );
    }

    #[test]
    fn generations_and_kdf_labels_are_separated() {
        let credential = credential();
        let current = credential.derive_session(7);
        let next = credential.derive_session(8);

        assert_ne!(current.room_id, next.room_id);
        assert_ne!(current.room_auth, next.room_auth);
        assert_ne!(&current.room_id["r1_".len()..], current.control_password());
        assert_eq!(current.room_id.len(), 46);
    }

    #[test]
    fn presence_tags_are_bounded_and_separated_by_generation_and_credential() {
        let credential = credential();
        let current = credential.derive_presence_tag(7);
        let next = credential.derive_presence_tag(8);
        let other = RememberSecret([0x5b; 32])
            .into_credential()
            .derive_presence_tag(7);

        assert_eq!(current.len(), REMEMBERED_PRESENCE_TAG_LEN);
        assert!(current.starts_with(REMEMBERED_PRESENCE_TAG_PREFIX));
        assert!(
            current
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
        assert_ne!(current, next);
        assert_ne!(current, other);
        assert_ne!(current, credential.derive_session(7).room_id());
    }

    #[tokio::test]
    async fn remember_secret_exists_only_after_mutual_authenticated_consent() {
        let config = invitation_config();
        let (mut sender, mut receiver) = connection_pair();
        let (sender_result, receiver_result) = tokio::join!(
            authenticate_sender_with_remember(&mut sender, &config, true),
            authenticate_receiver_with_remember(&mut receiver, &config, true),
        );
        let sender_credential = sender_result
            .expect("sender authenticates")
            .remember_secret
            .expect("sender remembers")
            .into_credential()
            .to_opaque();
        let receiver_credential = receiver_result
            .expect("receiver authenticates")
            .remember_secret
            .expect("receiver remembers")
            .into_credential()
            .to_opaque();
        assert_eq!(sender_credential, receiver_credential);

        let (mut sender, mut receiver) = connection_pair();
        let (sender_result, receiver_result) = tokio::join!(
            authenticate_sender_with_remember(&mut sender, &config, true),
            authenticate_receiver_with_remember(&mut receiver, &config, false),
        );
        assert!(
            sender_result
                .expect("sender authenticates")
                .remember_secret
                .is_none()
        );
        assert!(
            receiver_result
                .expect("receiver authenticates")
                .remember_secret
                .is_none()
        );
    }
}
