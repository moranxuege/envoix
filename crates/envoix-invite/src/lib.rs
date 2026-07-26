//! Strict, carrier-neutral directional invitation contract.
//!
//! QR renderers, clipboards, deep links, and platform scanners carry the
//! encoded string unchanged. This crate is the only owner of its grammar,
//! normalization, commitments, role policy, and expiry checks.

use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use iroh::{EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Prefix for every complete V2 invitation.
pub const INVITE_V2_PREFIX: &str = "envoix://invite/v2/";
/// V2 invitation schema version.
pub const INVITE_VERSION: u32 = 2;
/// Current transfer protocol version.
pub const TRANSFER_PROTOCOL_VERSION: u32 = 1;
/// Generated invitation lifetime.
pub const INVITE_TTL_SECS: u64 = 5 * 60;
/// Maximum unpadded base64url payload length.
pub const MAX_ENCODED_PAYLOAD_LEN: usize = 8 * 1024;
/// Maximum decoded canonical JSON length.
pub const MAX_DECODED_PAYLOAD_LEN: usize = 4 * 1024;
/// The cryptographic suite implemented by the existing Rust SPAKE2 backend.
pub const PAKE_SUITE: &str = "spake2-ed25519-sha256-hkdf-hmac";
/// Complete-invitation bootstrap identifier.
pub const FULL_TICKET_METHOD: &str = "full-ticket-v1";
/// Human Room-Code bootstrap identifier.
pub const ROOM_CODE_METHOD: &str = "room-code-v1";
/// Broker locator namespace reserved for an authenticated foreground room.
pub const ROOM_CONTROL_LOCATOR_PREFIX: &str = "c1_";
/// Currently implemented optional transfer capability.
pub const MANIFEST_V1_CAPABILITY: &str = "manifest-v1";

const INVITE_ID_LEN: usize = 16;
const TICKET_LEN: usize = 32;
const COMMITMENT_LEN: usize = 32;
const ROOM_ID_LEN: usize = 6;
const ROOM_SECRET_LEN: usize = 8;
const REMEMBERED_ROOM_ID_PREFIX: &str = "r1_";
const REMEMBERED_ROOM_ID_ENCODED_LEN: usize = 43;

const FULL_CONTROL_PASSWORD_INFO: &[u8] = b"envoix invite v2 full-ticket control password";
const ROOM_CONTROL_PASSWORD_INFO: &[u8] = b"envoix invite v2 room-code control password";
const DATA_PASSWORD_INFO: &[u8] = b"envoix invite v2 data authentication password";

/// Stable machine-readable invitation failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InvitationErrorCode {
    Malformed,
    Oversized,
    Expired,
    UnsupportedVersion,
    UnsupportedCapability,
    RoleConflict,
    AuthenticationFailed,
    Replay,
}

impl InvitationErrorCode {
    /// Stable lowercase wire/status spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Oversized => "oversized",
            Self::Expired => "expired",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::RoleConflict => "role_conflict",
            Self::AuthenticationFailed => "authentication_failure",
            Self::Replay => "replay",
        }
    }
}

/// Strict invitation parsing, validation, and authentication errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InvitationError {
    #[error("malformed invitation: {0}")]
    Malformed(String),
    #[error("invitation exceeds the supported size")]
    Oversized,
    #[error("invitation has expired")]
    Expired,
    #[error("unsupported invitation or protocol version")]
    UnsupportedVersion,
    #[error("unsupported required invitation capability: {0}")]
    UnsupportedCapability(String),
    #[error("invitation transfer role conflicts with the selected flow")]
    RoleConflict,
    #[error("invitation authentication failed")]
    AuthenticationFailed,
    #[error("invitation has already been consumed")]
    Replay,
}

impl InvitationError {
    /// Stable category for FFI and frontend status mapping.
    pub const fn code(&self) -> InvitationErrorCode {
        match self {
            Self::Malformed(_) => InvitationErrorCode::Malformed,
            Self::Oversized => InvitationErrorCode::Oversized,
            Self::Expired => InvitationErrorCode::Expired,
            Self::UnsupportedVersion => InvitationErrorCode::UnsupportedVersion,
            Self::UnsupportedCapability(_) => InvitationErrorCode::UnsupportedCapability,
            Self::RoleConflict => InvitationErrorCode::RoleConflict,
            Self::AuthenticationFailed => InvitationErrorCode::AuthenticationFailed,
            Self::Replay => InvitationErrorCode::Replay,
        }
    }
}

/// The local file-transfer role, independent of connection and PAKE roles.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferRole {
    Sender,
    Receiver,
}

impl TransferRole {
    /// The only valid peer role for a directional transfer.
    pub const fn complement(self) -> Self {
        match self {
            Self::Sender => Self::Receiver,
            Self::Receiver => Self::Sender,
        }
    }
}

/// Whether a broker connection owns or consumes an invitation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InvitationSide {
    Creator,
    Joiner,
}

/// The carrier-selected bootstrap path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum BootstrapKind {
    #[serde(rename = "full-ticket-v1")]
    FullTicket,
    #[serde(rename = "room-code-v1")]
    RoomCode,
}

impl BootstrapKind {
    /// Stable method identifier included in both authentication transcripts.
    pub const fn id(self) -> &'static str {
        match self {
            Self::FullTicket => FULL_TICKET_METHOD,
            Self::RoomCode => ROOM_CODE_METHOD,
        }
    }
}

/// A fixed 128-bit invitation identifier.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct InviteId([u8; INVITE_ID_LEN]);

impl InviteId {
    fn random() -> Result<Self, InvitationError> {
        let mut value = [0_u8; INVITE_ID_LEN];
        fill_random(&mut value)?;
        Ok(Self(value))
    }

    /// Raw identifier bytes for transcript construction.
    pub const fn as_bytes(&self) -> &[u8; INVITE_ID_LEN] {
        &self.0
    }
}

impl fmt::Debug for InviteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InviteId(")?;
        formatter.write_str(&URL_SAFE_NO_PAD.encode(self.0))?;
        formatter.write_str(")")
    }
}

/// A SHA-256 commitment. Debug output is deliberately redacted.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Commitment([u8; COMMITMENT_LEN]);

impl Commitment {
    /// SHA-256 commitment to authenticated transcript bytes.
    pub fn sha256(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Commitment bytes for authenticated transcript construction.
    pub const fn as_bytes(&self) -> &[u8; COMMITMENT_LEN] {
        &self.0
    }
}

impl fmt::Debug for Commitment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Commitment(<redacted>)")
    }
}

/// A 256-bit full-invitation ticket. All formatting is redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct TicketSecret([u8; TICKET_LEN]);

impl TicketSecret {
    fn random() -> Result<Self, InvitationError> {
        let mut value = [0_u8; TICKET_LEN];
        fill_random(&mut value)?;
        Ok(Self(value))
    }

    fn commitment(&self) -> Commitment {
        Commitment(Sha256::digest(self.0).into())
    }

    /// Derive the full-ticket control-plane SPAKE2 password.
    pub fn control_pake_password(&self, context: &Commitment) -> SecretString {
        derive_password(
            &self.0,
            context.as_bytes(),
            FULL_CONTROL_PASSWORD_INFO,
            FULL_TICKET_METHOD.as_bytes(),
        )
    }

    /// Derive the exporter-bound data authentication password.
    pub fn data_auth_password(&self, context: &Commitment, invite_id: &InviteId) -> SecretString {
        derive_password(
            &self.0,
            context.as_bytes(),
            DATA_PASSWORD_INFO,
            invite_id.as_bytes(),
        )
    }
}

impl fmt::Debug for TicketSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TicketSecret(<redacted>)")
    }
}

/// A derived password which must not appear in Debug output.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    /// Borrow for immediate use by the SPAKE2 implementation.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<redacted>)")
    }
}

/// A human-enterable code whose complete normalized value is the PAKE input.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RoomCode(String);

impl RoomCode {
    /// Generate `dddddd-xxxx-xxxx` with uniform decimal and lowercase Base36
    /// sampling.
    pub fn generate() -> Result<Self, InvitationError> {
        let mut room_id = String::with_capacity(ROOM_ID_LEN);
        for _ in 0..(ROOM_ID_LEN / 2) {
            let value = sample_below(100)?;
            room_id.push(char::from(b'0' + value / 10));
            room_id.push(char::from(b'0' + value % 10));
        }
        let mut secret = String::with_capacity(ROOM_SECRET_LEN);
        for _ in 0..ROOM_SECRET_LEN {
            let value = sample_below(36)?;
            secret.push(match value {
                0..=9 => char::from(b'0' + value),
                10..=35 => char::from(b'a' + value - 10),
                _ => unreachable!(),
            });
        }
        Ok(Self(format!("{room_id}-{}-{}", &secret[..4], &secret[4..])))
    }

    /// Parse only canonical or separator-free ASCII input. ASCII uppercase is
    /// normalized; whitespace, Unicode, extra separators, and suffixes fail.
    pub fn parse(input: &str) -> Result<Self, InvitationError> {
        if !input.is_ascii() {
            return Err(malformed("Room Code must contain only ASCII"));
        }
        let compact = match input.len() {
            14 if !input.as_bytes().contains(&b'-') => input.to_ascii_lowercase(),
            16 if input.as_bytes().get(6) == Some(&b'-')
                && input.as_bytes().get(11) == Some(&b'-') =>
            {
                let mut value = String::with_capacity(14);
                value.push_str(&input[..6]);
                value.push_str(&input[7..11]);
                value.push_str(&input[12..16]);
                value.to_ascii_lowercase()
            }
            _ => return Err(malformed("Room Code must be dddddd-xxxx-xxxx")),
        };
        let bytes = compact.as_bytes();
        if bytes.len() != ROOM_ID_LEN + ROOM_SECRET_LEN
            || !bytes[..ROOM_ID_LEN].iter().all(u8::is_ascii_digit)
            || !bytes[ROOM_ID_LEN..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
        {
            return Err(malformed("Room Code contains invalid characters"));
        }
        Ok(Self(format!(
            "{}-{}-{}",
            &compact[..6],
            &compact[6..10],
            &compact[10..14]
        )))
    }

    /// Canonical `dddddd-xxxx-xxxx` display and wire representation.
    pub fn canonical(&self) -> &str {
        &self.0
    }

    /// Six-digit broker lookup locator. No secret suffix is exposed.
    pub fn room_id(&self) -> &str {
        &self.0[..ROOM_ID_LEN]
    }

    /// Derive the Room control-plane SPAKE2 password from the complete code.
    pub fn control_pake_password(&self) -> SecretString {
        derive_password(
            self.0.as_bytes(),
            self.room_id().as_bytes(),
            ROOM_CONTROL_PASSWORD_INFO,
            ROOM_CODE_METHOD.as_bytes(),
        )
    }
}

impl fmt::Debug for RoomCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoomCode")
            .field("room_id", &self.room_id())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for RoomCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Required and optional protocol capabilities.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

impl Capabilities {
    /// The capabilities emitted by this build.
    pub fn current() -> Self {
        Self {
            required: Vec::new(),
            optional: vec![MANIFEST_V1_CAPABILITY.to_string()],
        }
    }

    fn validate(&self) -> Result<(), InvitationError> {
        validate_capability_list(&self.required)?;
        validate_capability_list(&self.optional)?;
        let required = self.required.iter().collect::<HashSet<_>>();
        if self.optional.iter().any(|name| required.contains(name)) {
            return Err(malformed("capability is both required and optional"));
        }
        for capability in &self.required {
            if capability != MANIFEST_V1_CAPABILITY {
                return Err(InvitationError::UnsupportedCapability(capability.clone()));
            }
        }
        Ok(())
    }
}

/// Public bootstrap descriptor authenticated by the invitation context.
#[derive(Clone, Eq, PartialEq)]
pub enum BootstrapMethod {
    FullTicket { ticket_commitment: Commitment },
    RoomCode { room_id: String },
}

impl BootstrapMethod {
    /// Stable method identifier.
    pub const fn id(&self) -> &'static str {
        match self {
            Self::FullTicket { .. } => FULL_TICKET_METHOD,
            Self::RoomCode { .. } => ROOM_CODE_METHOD,
        }
    }

    /// Carrier-selectable method kind.
    pub const fn kind(&self) -> BootstrapKind {
        match self {
            Self::FullTicket { .. } => BootstrapKind::FullTicket,
            Self::RoomCode { .. } => BootstrapKind::RoomCode,
        }
    }
}

impl fmt::Debug for BootstrapMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FullTicket { .. } => formatter
                .debug_struct("FullTicket")
                .field("ticket_commitment", &"<redacted>")
                .finish(),
            Self::RoomCode { room_id } => formatter
                .debug_struct("RoomCode")
                .field("room_id", room_id)
                .finish(),
        }
    }
}

/// Public invitation fields committed with SHA-256 over their JCS encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvitationPublicContext {
    pub version: u32,
    pub invite_id: InviteId,
    pub protocol_version: u32,
    pub creator_transfer_role: TransferRole,
    pub joiner_transfer_role: TransferRole,
    pub broker: String,
    pub relay_urls: Vec<String>,
    pub capabilities: Capabilities,
    pub expires_at: u64,
    pub bootstrap_methods: Vec<BootstrapMethod>,
}

impl InvitationPublicContext {
    /// JCS encoding used for the context commitment and sealed Room-Code
    /// delivery.
    pub fn canonical_json(&self) -> Result<Vec<u8>, InvitationError> {
        jcs_context(self)
    }

    /// Parse and validate a sealed public context.
    pub fn parse_canonical(
        bytes: &[u8],
        expected_room_id: &str,
        local_joiner_role: TransferRole,
        now: u64,
    ) -> Result<(Self, Commitment), InvitationError> {
        if bytes.len() > MAX_DECODED_PAYLOAD_LEN {
            return Err(InvitationError::Oversized);
        }
        let document: PublicContextDocument = serde_json::from_slice(bytes)
            .map_err(|error| malformed(format!("invalid public context JSON: {error}")))?;
        let canonical = jcs_public_document(&document)?;
        if canonical != bytes {
            return Err(malformed("public context JSON is not canonical JCS"));
        }
        let context = document.into_context()?;
        validate_public_context(&context, now)?;
        if context.joiner_transfer_role != local_joiner_role {
            return Err(InvitationError::RoleConflict);
        }
        let room_id = context
            .bootstrap_methods
            .iter()
            .find_map(|method| match method {
                BootstrapMethod::RoomCode { room_id } => Some(room_id.as_str()),
                BootstrapMethod::FullTicket { .. } => None,
            })
            .ok_or_else(|| malformed("room-code bootstrap is missing"))?;
        if room_id != expected_room_id {
            return Err(InvitationError::AuthenticationFailed);
        }
        let commitment = commitment_for_context(&context)?;
        Ok((context, commitment))
    }
}

/// Immutable invitation binding supplied to data-plane authentication.
#[derive(Clone, Eq, PartialEq)]
pub struct InvitationAuthContext {
    pub invite_id: InviteId,
    pub context_commitment: Commitment,
    pub selected_bootstrap_method: BootstrapKind,
    pub creator_transfer_role: TransferRole,
    pub joiner_transfer_role: TransferRole,
    pub control_transcript_hash: Option<Commitment>,
}

/// Invitation fields known to both peers before a broker-relayed control PAKE.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvitationControlContext {
    pub room_id: String,
    pub selected_bootstrap_method: BootstrapKind,
    pub creator_transfer_role: TransferRole,
    pub joiner_transfer_role: TransferRole,
}

impl InvitationControlContext {
    pub fn new(
        room_id: String,
        selected_bootstrap_method: BootstrapKind,
        creator_transfer_role: TransferRole,
        joiner_transfer_role: TransferRole,
    ) -> Result<Self, InvitationError> {
        validate_room_id(&room_id)?;
        if creator_transfer_role.complement() != joiner_transfer_role {
            return Err(InvitationError::RoleConflict);
        }
        Ok(Self {
            room_id,
            selected_bootstrap_method,
            creator_transfer_role,
            joiner_transfer_role,
        })
    }

    /// Construct the fixed role mapping for a remembered-device rendezvous.
    ///
    /// The receiver advertises as the responder and the sender joins as the
    /// initiator. The high-entropy locator is derived from the remembered
    /// credential rather than from an invitation.
    pub fn remembered(room_id: String) -> Result<Self, InvitationError> {
        validate_remembered_room_id(&room_id)?;
        Ok(Self {
            room_id,
            selected_bootstrap_method: BootstrapKind::FullTicket,
            creator_transfer_role: TransferRole::Receiver,
            joiner_transfer_role: TransferRole::Sender,
        })
    }

    /// Construct the fixed creator/joiner mapping used only to bootstrap a
    /// direction-neutral foreground room control connection.
    ///
    /// These roles select deterministic SPAKE2 speaking order; they do not
    /// authorize a file direction inside the established room.
    pub fn room_control(room_id: String) -> Result<Self, InvitationError> {
        if !is_room_control_locator(&room_id) {
            return Err(malformed("room control locator is invalid"));
        }
        Ok(Self {
            room_id,
            selected_bootstrap_method: BootstrapKind::RoomCode,
            creator_transfer_role: TransferRole::Receiver,
            joiner_transfer_role: TransferRole::Sender,
        })
    }

    /// Deterministic framed bytes included in control-plane confirmation.
    pub fn framed_binding(&self) -> Vec<u8> {
        let mut output = Vec::new();
        append_len_prefixed(&mut output, self.room_id.as_bytes());
        append_len_prefixed(&mut output, self.selected_bootstrap_method.id().as_bytes());
        append_len_prefixed(&mut output, transfer_role_bytes(self.creator_transfer_role));
        append_len_prefixed(&mut output, transfer_role_bytes(self.joiner_transfer_role));
        output
    }
}

impl InvitationAuthContext {
    /// Construct after the selected bootstrap authenticated the public context.
    pub fn new(
        public: &InvitationPublicContext,
        context_commitment: Commitment,
        selected_bootstrap_method: BootstrapKind,
        control_transcript_hash: Option<Commitment>,
    ) -> Self {
        Self {
            invite_id: public.invite_id,
            context_commitment,
            selected_bootstrap_method,
            creator_transfer_role: public.creator_transfer_role,
            joiner_transfer_role: public.joiner_transfer_role,
            control_transcript_hash,
        }
    }

    /// Deterministic framed bytes used as the TLS exporter context and inside
    /// the confirmation transcript.
    pub fn framed_binding(&self) -> Vec<u8> {
        let mut output = Vec::new();
        append_len_prefixed(&mut output, self.invite_id.as_bytes());
        append_len_prefixed(&mut output, self.context_commitment.as_bytes());
        append_len_prefixed(&mut output, self.selected_bootstrap_method.id().as_bytes());
        append_len_prefixed(&mut output, transfer_role_bytes(self.creator_transfer_role));
        append_len_prefixed(&mut output, transfer_role_bytes(self.joiner_transfer_role));
        append_len_prefixed(
            &mut output,
            self.control_transcript_hash
                .as_ref()
                .map(Commitment::as_bytes)
                .map(<[u8; COMMITMENT_LEN]>::as_slice)
                .unwrap_or_default(),
        );
        output
    }
}

impl fmt::Debug for InvitationAuthContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvitationAuthContext")
            .field("invite_id", &self.invite_id)
            .field("context_commitment", &"<redacted>")
            .field("selected_bootstrap_method", &self.selected_bootstrap_method)
            .field("creator_transfer_role", &self.creator_transfer_role)
            .field("joiner_transfer_role", &self.joiner_transfer_role)
            .field(
                "control_transcript_hash",
                &self.control_transcript_hash.map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Complete V2 invitation with the carrier-presented full-ticket credential.
#[derive(Clone, Eq, PartialEq)]
pub struct InviteV2 {
    pub public_context: InvitationPublicContext,
    pub context_commitment: Commitment,
    ticket: TicketSecret,
}

impl fmt::Debug for InviteV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InviteV2")
            .field("public_context", &self.public_context)
            .field("context_commitment", &"<redacted>")
            .field("ticket", &"<redacted>")
            .finish()
    }
}

/// Creator output for intentional display and authenticated transfer startup.
#[derive(Clone, Eq, PartialEq)]
pub struct CreatedInvitation {
    pub payload: String,
    pub room_code: RoomCode,
    pub creator_role: TransferRole,
    pub joiner_role: TransferRole,
    pub expires_at: u64,
    invitation: InviteV2,
}

impl CreatedInvitation {
    /// The validated private invitation state for the creator transfer driver.
    pub fn invitation(&self) -> &InviteV2 {
        &self.invitation
    }

    /// Consume display output into the creator's private pairing state.
    pub fn into_bootstrap(self) -> InvitationBootstrap {
        InvitationBootstrap::Creator {
            invitation: self.invitation,
            room_code: self.room_code,
        }
    }
}

impl fmt::Debug for CreatedInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreatedInvitation")
            .field("payload", &"<redacted>")
            .field("room_code", &self.room_code)
            .field("creator_role", &self.creator_role)
            .field("joiner_role", &self.joiner_role)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A parsed invitation that passed syntax, capability, role, commitment, and
/// expiry validation.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedInvitation(InviteV2);

impl ValidatedInvitation {
    pub fn invitation(&self) -> &InviteV2 {
        &self.0
    }

    pub fn joiner_role(&self) -> TransferRole {
        self.0.public_context.joiner_transfer_role
    }

    pub fn require_local_role(&self, role: TransferRole) -> Result<(), InvitationError> {
        if self.joiner_role() == role {
            Ok(())
        } else {
            Err(InvitationError::RoleConflict)
        }
    }

    pub fn into_invitation(self) -> InviteV2 {
        self.0
    }

    /// Consume a complete parsed invitation into joiner pairing state.
    pub fn into_bootstrap(self) -> InvitationBootstrap {
        InvitationBootstrap::FullTicketJoiner(self.0)
    }
}

impl fmt::Debug for ValidatedInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedInvitation")
            .field("invite_id", &self.0.public_context.invite_id)
            .field("joiner_role", &self.joiner_role())
            .field("expires_at", &self.0.public_context.expires_at)
            .finish()
    }
}

impl InviteV2 {
    /// Generate a complete invitation and its independent human Room Code.
    pub fn create(
        broker: String,
        relay_urls: Vec<String>,
        creator_role: TransferRole,
        capabilities: Capabilities,
        now: u64,
    ) -> Result<CreatedInvitation, InvitationError> {
        let expires_at = now
            .checked_add(INVITE_TTL_SECS)
            .ok_or_else(|| malformed("invitation expiry overflow"))?;
        let invite_id = InviteId::random()?;
        let ticket = TicketSecret::random()?;
        let ticket_commitment = ticket.commitment();
        let room_code = RoomCode::generate()?;
        let joiner_role = creator_role.complement();
        let public_context = InvitationPublicContext {
            version: INVITE_VERSION,
            invite_id,
            protocol_version: TRANSFER_PROTOCOL_VERSION,
            creator_transfer_role: creator_role,
            joiner_transfer_role: joiner_role,
            broker,
            relay_urls,
            capabilities,
            expires_at,
            bootstrap_methods: vec![
                BootstrapMethod::FullTicket { ticket_commitment },
                BootstrapMethod::RoomCode {
                    room_id: room_code.room_id().to_string(),
                },
            ],
        };
        validate_public_context(&public_context, now)?;
        let context_commitment = commitment_for_context(&public_context)?;
        let invitation = Self {
            public_context,
            context_commitment,
            ticket,
        };
        let payload = invitation.encode()?;
        Ok(CreatedInvitation {
            payload,
            room_code,
            creator_role,
            joiner_role,
            expires_at,
            invitation,
        })
    }

    /// Decode and validate a complete invitation for routing.
    pub fn parse(input: &str, now: u64) -> Result<ValidatedInvitation, InvitationError> {
        if input.starts_with("envoix:") && !input.starts_with(INVITE_V2_PREFIX)
            || input.starts_with("envoix://pair/")
        {
            return Err(InvitationError::UnsupportedVersion);
        }
        let encoded = input
            .strip_prefix(INVITE_V2_PREFIX)
            .ok_or_else(|| malformed("missing InviteV2 prefix"))?;
        if encoded.len() > MAX_ENCODED_PAYLOAD_LEN {
            return Err(InvitationError::Oversized);
        }
        if encoded.is_empty() || encoded.as_bytes().contains(&b'=') {
            return Err(malformed("payload must be unpadded base64url"));
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| malformed("invalid base64url payload"))?;
        if decoded.len() > MAX_DECODED_PAYLOAD_LEN {
            return Err(InvitationError::Oversized);
        }
        let document: InviteDocument = serde_json::from_slice(&decoded)
            .map_err(|error| malformed(format!("invalid invitation JSON: {error}")))?;
        let canonical = jcs_document(&document)?;
        if canonical != decoded {
            return Err(malformed("invitation JSON is not canonical JCS"));
        }
        let invitation = document.into_invitation()?;
        validate_public_context(&invitation.public_context, now)?;
        let expected_context = commitment_for_context(&invitation.public_context)?;
        if expected_context != invitation.context_commitment {
            return Err(InvitationError::AuthenticationFailed);
        }
        let expected_ticket = invitation.ticket.commitment();
        let advertised_ticket = invitation
            .public_context
            .bootstrap_methods
            .iter()
            .find_map(|method| match method {
                BootstrapMethod::FullTicket { ticket_commitment } => Some(*ticket_commitment),
                BootstrapMethod::RoomCode { .. } => None,
            })
            .ok_or_else(|| malformed("full-ticket bootstrap is missing"))?;
        if expected_ticket != advertised_ticket {
            return Err(InvitationError::AuthenticationFailed);
        }
        Ok(ValidatedInvitation(invitation))
    }

    /// Parse and require the role supplied by an existing Send/Receive flow.
    pub fn parse_for_role(
        input: &str,
        local_role: TransferRole,
        now: u64,
    ) -> Result<ValidatedInvitation, InvitationError> {
        let invitation = Self::parse(input, now)?;
        invitation.require_local_role(local_role)?;
        Ok(invitation)
    }

    fn encode(&self) -> Result<String, InvitationError> {
        let document = InviteDocument::from_invitation(self);
        let bytes = jcs_document(&document)?;
        if bytes.len() > MAX_DECODED_PAYLOAD_LEN {
            return Err(InvitationError::Oversized);
        }
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        if encoded.len() > MAX_ENCODED_PAYLOAD_LEN {
            return Err(InvitationError::Oversized);
        }
        Ok(format!("{INVITE_V2_PREFIX}{encoded}"))
    }

    /// Full-ticket credential for immediate password derivation only.
    pub fn ticket(&self) -> &TicketSecret {
        &self.ticket
    }
}

/// Derive the data authentication password after a Room control PAKE.
pub fn derive_room_data_auth_password(
    control_shared_key: &[u8],
    context: &Commitment,
    invite_id: &InviteId,
) -> SecretString {
    derive_password(
        control_shared_key,
        context.as_bytes(),
        DATA_PASSWORD_INFO,
        invite_id.as_bytes(),
    )
}

/// Private state required to run exactly one selected invitation bootstrap.
#[derive(Clone, Eq, PartialEq)]
pub enum InvitationBootstrap {
    Creator {
        invitation: InviteV2,
        room_code: RoomCode,
    },
    FullTicketJoiner(InviteV2),
    RoomCodeJoiner {
        room_code: RoomCode,
        local_role: TransferRole,
    },
}

impl InvitationBootstrap {
    pub fn room_code_joiner(room_code: RoomCode, local_role: TransferRole) -> Self {
        Self::RoomCodeJoiner {
            room_code,
            local_role,
        }
    }

    pub const fn side(&self) -> InvitationSide {
        match self {
            Self::Creator { .. } => InvitationSide::Creator,
            Self::FullTicketJoiner(_) | Self::RoomCodeJoiner { .. } => InvitationSide::Joiner,
        }
    }

    pub fn local_role(&self) -> TransferRole {
        match self {
            Self::Creator { invitation, .. } => invitation.public_context.creator_transfer_role,
            Self::FullTicketJoiner(invitation) => invitation.public_context.joiner_transfer_role,
            Self::RoomCodeJoiner { local_role, .. } => *local_role,
        }
    }

    pub fn room_id(&self) -> &str {
        match self {
            Self::Creator { room_code, .. } | Self::RoomCodeJoiner { room_code, .. } => {
                room_code.room_id()
            }
            Self::FullTicketJoiner(invitation) => room_id_from_context(&invitation.public_context)
                .expect("validated full invitation has room bootstrap"),
        }
    }

    pub fn advertised_methods(&self) -> Vec<BootstrapKind> {
        match self {
            Self::Creator { .. } => vec![BootstrapKind::FullTicket, BootstrapKind::RoomCode],
            Self::FullTicketJoiner(_) | Self::RoomCodeJoiner { .. } => Vec::new(),
        }
    }

    pub const fn selected_method(&self) -> Option<BootstrapKind> {
        match self {
            Self::Creator { .. } => None,
            Self::FullTicketJoiner(_) => Some(BootstrapKind::FullTicket),
            Self::RoomCodeJoiner { .. } => Some(BootstrapKind::RoomCode),
        }
    }

    pub fn control_context(
        &self,
        selected: BootstrapKind,
    ) -> Result<InvitationControlContext, InvitationError> {
        if let Some(expected) = self.selected_method()
            && expected != selected
        {
            return Err(InvitationError::AuthenticationFailed);
        }
        let local_role = self.local_role();
        let (creator_role, joiner_role) = match self.side() {
            InvitationSide::Creator => (local_role, local_role.complement()),
            InvitationSide::Joiner => (local_role.complement(), local_role),
        };
        InvitationControlContext::new(
            self.room_id().to_string(),
            selected,
            creator_role,
            joiner_role,
        )
    }

    pub fn control_pake_password(
        &self,
        selected: BootstrapKind,
    ) -> Result<SecretString, InvitationError> {
        match (self, selected) {
            (
                Self::Creator {
                    invitation,
                    room_code: _,
                }
                | Self::FullTicketJoiner(invitation),
                BootstrapKind::FullTicket,
            ) => Ok(invitation
                .ticket
                .control_pake_password(&invitation.context_commitment)),
            (
                Self::Creator { room_code, .. } | Self::RoomCodeJoiner { room_code, .. },
                BootstrapKind::RoomCode,
            ) => Ok(room_code.control_pake_password()),
            _ => Err(InvitationError::AuthenticationFailed),
        }
    }

    /// Only creators deliver public context through the sealed control bundle.
    pub fn creator_public_context(&self) -> Result<Option<Vec<u8>>, InvitationError> {
        match self {
            Self::Creator { invitation, .. } => {
                Ok(Some(invitation.public_context.canonical_json()?))
            }
            Self::FullTicketJoiner(_) | Self::RoomCodeJoiner { .. } => Ok(None),
        }
    }

    /// Authenticate creator-delivered context and derive the data-plane inputs.
    pub fn validate_control_context(
        &self,
        selected: BootstrapKind,
        peer_public_context: Option<&[u8]>,
        now: u64,
    ) -> Result<(), InvitationError> {
        self.authenticated_public_context(selected, peer_public_context, now)
            .map(|_| ())
    }

    fn authenticated_public_context(
        &self,
        selected: BootstrapKind,
        peer_public_context: Option<&[u8]>,
        now: u64,
    ) -> Result<(InvitationPublicContext, Commitment), InvitationError> {
        let (public, context_commitment) = match self {
            Self::Creator { invitation, .. } => {
                if peer_public_context.is_some() {
                    return Err(InvitationError::AuthenticationFailed);
                }
                (
                    invitation.public_context.clone(),
                    invitation.context_commitment,
                )
            }
            Self::FullTicketJoiner(invitation) => {
                let received = peer_public_context.ok_or(InvitationError::AuthenticationFailed)?;
                if received != invitation.public_context.canonical_json()?.as_slice() {
                    return Err(InvitationError::AuthenticationFailed);
                }
                (
                    invitation.public_context.clone(),
                    invitation.context_commitment,
                )
            }
            Self::RoomCodeJoiner {
                room_code,
                local_role,
            } => {
                let received = peer_public_context.ok_or(InvitationError::AuthenticationFailed)?;
                InvitationPublicContext::parse_canonical(
                    received,
                    room_code.room_id(),
                    *local_role,
                    now,
                )?
            }
        };
        validate_public_context(&public, now)?;
        if selected != self.selected_method().unwrap_or(selected) {
            return Err(InvitationError::AuthenticationFailed);
        }
        validate_public_context(&public, now)?;
        Ok((public, context_commitment))
    }

    /// Derive data-plane inputs after the selected bootstrap and descriptor
    /// exchange have both been authenticated.
    pub fn finish_control(
        &self,
        selected: BootstrapKind,
        peer_public_context: Option<&[u8]>,
        control_key: &[u8],
        control_transcript_hash: Commitment,
        now: u64,
    ) -> Result<(SecretString, InvitationAuthContext), InvitationError> {
        let (public, context_commitment) =
            self.authenticated_public_context(selected, peer_public_context, now)?;
        let data_password = match (self, selected) {
            (
                Self::Creator { invitation, .. } | Self::FullTicketJoiner(invitation),
                BootstrapKind::FullTicket,
            ) => invitation
                .ticket
                .data_auth_password(&context_commitment, &public.invite_id),
            (_, BootstrapKind::RoomCode) => {
                derive_room_data_auth_password(control_key, &context_commitment, &public.invite_id)
            }
            _ => return Err(InvitationError::AuthenticationFailed),
        };
        let auth_context = InvitationAuthContext::new(
            &public,
            context_commitment,
            selected,
            Some(control_transcript_hash),
        );
        Ok((data_password, auth_context))
    }
}

impl fmt::Debug for InvitationBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvitationBootstrap")
            .field("side", &self.side())
            .field("local_role", &self.local_role())
            .field("room_id", &self.room_id())
            .field("credential", &"<redacted>")
            .finish()
    }
}

fn room_id_from_context(context: &InvitationPublicContext) -> Option<&str> {
    context
        .bootstrap_methods
        .iter()
        .find_map(|method| match method {
            BootstrapMethod::RoomCode { room_id } => Some(room_id.as_str()),
            BootstrapMethod::FullTicket { .. } => None,
        })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InviteDocument {
    version: u32,
    invite_id: String,
    protocol_version: u32,
    creator_transfer_role: TransferRole,
    joiner_transfer_role: TransferRole,
    broker: String,
    relay_urls: Vec<String>,
    capabilities: Capabilities,
    expires_at: u64,
    bootstrap_methods: Vec<BootstrapDocument>,
    context_commitment: String,
    presented_credential: PresentedCredential,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapDocument {
    id: String,
    pake: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket_commitment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room_id: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PresentedCredential {
    method: String,
    ticket: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicContextDocument {
    version: u32,
    invite_id: String,
    protocol_version: u32,
    creator_transfer_role: TransferRole,
    joiner_transfer_role: TransferRole,
    broker: String,
    relay_urls: Vec<String>,
    capabilities: Capabilities,
    expires_at: u64,
    bootstrap_methods: Vec<BootstrapDocument>,
}

impl PublicContextDocument {
    fn from_context(context: &InvitationPublicContext) -> Self {
        Self {
            version: context.version,
            invite_id: URL_SAFE_NO_PAD.encode(context.invite_id.0),
            protocol_version: context.protocol_version,
            creator_transfer_role: context.creator_transfer_role,
            joiner_transfer_role: context.joiner_transfer_role,
            broker: context.broker.clone(),
            relay_urls: context.relay_urls.clone(),
            capabilities: context.capabilities.clone(),
            expires_at: context.expires_at,
            bootstrap_methods: context
                .bootstrap_methods
                .iter()
                .map(BootstrapDocument::from_method)
                .collect(),
        }
    }

    fn into_context(self) -> Result<InvitationPublicContext, InvitationError> {
        Ok(InvitationPublicContext {
            version: self.version,
            invite_id: InviteId(decode_fixed(&self.invite_id, "invite_id")?),
            protocol_version: self.protocol_version,
            creator_transfer_role: self.creator_transfer_role,
            joiner_transfer_role: self.joiner_transfer_role,
            broker: self.broker,
            relay_urls: self.relay_urls,
            capabilities: self.capabilities,
            expires_at: self.expires_at,
            bootstrap_methods: self
                .bootstrap_methods
                .into_iter()
                .map(BootstrapDocument::into_method)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl InviteDocument {
    fn from_invitation(invitation: &InviteV2) -> Self {
        let context = &invitation.public_context;
        Self {
            version: context.version,
            invite_id: URL_SAFE_NO_PAD.encode(context.invite_id.0),
            protocol_version: context.protocol_version,
            creator_transfer_role: context.creator_transfer_role,
            joiner_transfer_role: context.joiner_transfer_role,
            broker: context.broker.clone(),
            relay_urls: context.relay_urls.clone(),
            capabilities: context.capabilities.clone(),
            expires_at: context.expires_at,
            bootstrap_methods: context
                .bootstrap_methods
                .iter()
                .map(BootstrapDocument::from_method)
                .collect(),
            context_commitment: URL_SAFE_NO_PAD.encode(invitation.context_commitment.0),
            presented_credential: PresentedCredential {
                method: FULL_TICKET_METHOD.to_string(),
                ticket: URL_SAFE_NO_PAD.encode(invitation.ticket.0),
            },
        }
    }

    fn into_invitation(self) -> Result<InviteV2, InvitationError> {
        if self.presented_credential.method != FULL_TICKET_METHOD {
            return Err(malformed("full invitation selected an invalid credential"));
        }
        let invite_id = InviteId(decode_fixed(&self.invite_id, "invite_id")?);
        let context_commitment = Commitment(decode_fixed(
            &self.context_commitment,
            "context_commitment",
        )?);
        let ticket = TicketSecret(decode_fixed(&self.presented_credential.ticket, "ticket")?);
        let bootstrap_methods = self
            .bootstrap_methods
            .into_iter()
            .map(BootstrapDocument::into_method)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(InviteV2 {
            public_context: InvitationPublicContext {
                version: self.version,
                invite_id,
                protocol_version: self.protocol_version,
                creator_transfer_role: self.creator_transfer_role,
                joiner_transfer_role: self.joiner_transfer_role,
                broker: self.broker,
                relay_urls: self.relay_urls,
                capabilities: self.capabilities,
                expires_at: self.expires_at,
                bootstrap_methods,
            },
            context_commitment,
            ticket,
        })
    }
}

impl BootstrapDocument {
    fn from_method(method: &BootstrapMethod) -> Self {
        match method {
            BootstrapMethod::FullTicket { ticket_commitment } => Self {
                id: FULL_TICKET_METHOD.to_string(),
                pake: PAKE_SUITE.to_string(),
                ticket_commitment: Some(URL_SAFE_NO_PAD.encode(ticket_commitment.0)),
                room_id: None,
            },
            BootstrapMethod::RoomCode { room_id } => Self {
                id: ROOM_CODE_METHOD.to_string(),
                pake: PAKE_SUITE.to_string(),
                ticket_commitment: None,
                room_id: Some(room_id.clone()),
            },
        }
    }

    fn into_method(self) -> Result<BootstrapMethod, InvitationError> {
        if self.pake != PAKE_SUITE {
            return Err(InvitationError::UnsupportedCapability(self.pake));
        }
        match self.id.as_str() {
            FULL_TICKET_METHOD => {
                if self.room_id.is_some() {
                    return Err(malformed("full-ticket bootstrap contains room_id"));
                }
                let commitment = self
                    .ticket_commitment
                    .ok_or_else(|| malformed("full-ticket commitment is missing"))?;
                Ok(BootstrapMethod::FullTicket {
                    ticket_commitment: Commitment(decode_fixed(&commitment, "ticket_commitment")?),
                })
            }
            ROOM_CODE_METHOD => {
                if self.ticket_commitment.is_some() {
                    return Err(malformed("room-code bootstrap contains ticket commitment"));
                }
                let room_id = self
                    .room_id
                    .ok_or_else(|| malformed("room-code room_id is missing"))?;
                validate_room_id(&room_id)?;
                Ok(BootstrapMethod::RoomCode { room_id })
            }
            _ => Err(InvitationError::UnsupportedCapability(self.id)),
        }
    }
}

fn validate_public_context(
    context: &InvitationPublicContext,
    now: u64,
) -> Result<(), InvitationError> {
    if context.version != INVITE_VERSION || context.protocol_version != TRANSFER_PROTOCOL_VERSION {
        return Err(InvitationError::UnsupportedVersion);
    }
    if context.creator_transfer_role.complement() != context.joiner_transfer_role {
        return Err(InvitationError::RoleConflict);
    }
    if context.expires_at <= now {
        return Err(InvitationError::Expired);
    }
    validate_broker(&context.broker)?;
    validate_relays(&context.relay_urls)?;
    context.capabilities.validate()?;
    if context.bootstrap_methods.len() != 2
        || context.bootstrap_methods[0].id() != FULL_TICKET_METHOD
        || context.bootstrap_methods[1].id() != ROOM_CODE_METHOD
    {
        return Err(malformed(
            "bootstrap methods must be full-ticket-v1 then room-code-v1",
        ));
    }
    let mut ids = HashSet::new();
    for method in &context.bootstrap_methods {
        if !ids.insert(method.id()) {
            return Err(malformed("duplicate bootstrap method"));
        }
        if let BootstrapMethod::RoomCode { room_id } = method {
            validate_room_id(room_id)?;
        }
    }
    Ok(())
}

fn validate_broker(value: &str) -> Result<(), InvitationError> {
    let (endpoint_id, address) = value
        .split_once('@')
        .ok_or_else(|| malformed("broker must be <endpoint-id>@<ip:port>"))?;
    endpoint_id
        .parse::<EndpointId>()
        .map_err(|_| malformed("invalid broker endpoint id"))?;
    address
        .parse::<SocketAddr>()
        .map_err(|_| malformed("invalid broker socket address"))?;
    Ok(())
}

fn validate_relays(relays: &[String]) -> Result<(), InvitationError> {
    let mut seen = HashSet::new();
    for relay in relays {
        let parsed = relay
            .parse::<RelayUrl>()
            .map_err(|_| malformed("invalid relay URL"))?;
        if !seen.insert(parsed) {
            return Err(malformed("duplicate relay URL"));
        }
    }
    Ok(())
}

fn validate_capability_list(capabilities: &[String]) -> Result<(), InvitationError> {
    if !capabilities.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(malformed(
            "capabilities must be unique and sorted by ASCII byte order",
        ));
    }
    for name in capabilities {
        let bytes = name.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 64
            || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        {
            return Err(malformed("invalid capability name"));
        }
    }
    Ok(())
}

fn validate_room_id(value: &str) -> Result<(), InvitationError> {
    if value.len() == ROOM_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(malformed("room_id must contain exactly six digits"))
    }
}

/// Whether `value` is the bounded, non-transfer namespace used by a
/// foreground room control rendezvous.
pub fn is_room_control_locator(value: &str) -> bool {
    value
        .strip_prefix(ROOM_CONTROL_LOCATOR_PREFIX)
        .is_some_and(|room_id| {
            room_id.len() == ROOM_ID_LEN && room_id.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn validate_remembered_room_id(value: &str) -> Result<(), InvitationError> {
    let Some(encoded) = value.strip_prefix(REMEMBERED_ROOM_ID_PREFIX) else {
        return Err(malformed("remembered room_id has an invalid prefix"));
    };
    if encoded.len() == REMEMBERED_ROOM_ID_ENCODED_LEN
        && encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(())
    } else {
        Err(malformed("remembered room_id is invalid"))
    }
}

fn commitment_for_context(
    context: &InvitationPublicContext,
) -> Result<Commitment, InvitationError> {
    Ok(Commitment(Sha256::digest(jcs_context(context)?).into()))
}

fn jcs_context(context: &InvitationPublicContext) -> Result<Vec<u8>, InvitationError> {
    jcs_public_document(&PublicContextDocument::from_context(context))
}

fn jcs_public_document(document: &PublicContextDocument) -> Result<Vec<u8>, InvitationError> {
    let value = serde_json::to_value(document).map_err(|error| malformed(error.to_string()))?;
    jcs_value(&value)
}

fn jcs_document(document: &InviteDocument) -> Result<Vec<u8>, InvitationError> {
    let value = serde_json::to_value(document).map_err(|error| malformed(error.to_string()))?;
    jcs_value(&value)
}

// All InviteV2 numbers are non-negative integers and property names are ASCII.
// Sorting object keys plus serde_json's minimal string/integer representation is
// therefore the exact RFC 8785 JCS encoding for the accepted schema subset.
fn jcs_value(value: &Value) -> Result<Vec<u8>, InvitationError> {
    serde_json::to_vec(value).map_err(|error| malformed(error.to_string()))
}

fn decode_fixed<const N: usize>(value: &str, field: &str) -> Result<[u8; N], InvitationError> {
    if value.as_bytes().contains(&b'=') {
        return Err(malformed(format!("{field} must use unpadded base64url")));
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| malformed(format!("invalid {field}")))?;
    decoded
        .try_into()
        .map_err(|_| malformed(format!("{field} has the wrong length")))
}

fn derive_password(ikm: &[u8], salt: &[u8], info: &[u8], binding: &[u8]) -> SecretString {
    let mut output = [0_u8; 32];
    let hkdf = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut framed_info = Vec::with_capacity(info.len() + binding.len() + 16);
    append_len_prefixed(&mut framed_info, info);
    append_len_prefixed(&mut framed_info, binding);
    hkdf.expand(&framed_info, &mut output)
        .expect("32-byte HKDF-SHA256 output is valid");
    SecretString(URL_SAFE_NO_PAD.encode(output))
}

fn append_len_prefixed(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

const fn transfer_role_bytes(role: TransferRole) -> &'static [u8] {
    match role {
        TransferRole::Sender => b"sender",
        TransferRole::Receiver => b"receiver",
    }
}

fn fill_random(output: &mut [u8]) -> Result<(), InvitationError> {
    getrandom::fill(output)
        .map_err(|_| InvitationError::Malformed("entropy source unavailable".into()))
}

fn sample_below(upper: u8) -> Result<u8, InvitationError> {
    let accepted = u8::MAX - (u8::MAX % upper);
    loop {
        let mut byte = [0_u8; 1];
        fill_random(&mut byte)?;
        if byte[0] < accepted {
            return Ok(byte[0] % upper);
        }
    }
}

fn malformed(message: impl Into<String>) -> InvitationError {
    InvitationError::Malformed(message.into())
}

#[cfg(test)]
mod tests;
