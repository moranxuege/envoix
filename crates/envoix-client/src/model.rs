//! Product identifiers and immutable snapshot records.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{kind} must contain 1 to {MAX_IDENTIFIER_BYTES} ASCII letters, digits, '-' or '_'")]
pub struct IdentifierError {
    kind: &'static str,
}

fn validate_identifier(value: String, kind: &'static str) -> Result<String, IdentifierError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(IdentifierError { kind });
    }
    Ok(value)
}

macro_rules! identifier {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentifierError> {
                validate_identifier(value.into(), $kind).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentifierError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

identifier!(CommandId, "command ID");
identifier!(ContentId, "content ID");
identifier!(DeviceId, "device ID");
identifier!(RelationshipId, "relationship ID");
identifier!(RoomId, "room ID");
identifier!(TransferId, "transfer ID");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Device,
    Relationship,
    Room,
    Transfer,
}

impl EntityKind {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Relationship => "relationship",
            Self::Room => "room",
            Self::Transfer => "transfer",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipState {
    Trusted,
    Revoked,
}

/// Rendezvous side used to align remembered-generation recovery attempts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RememberedGenerationRole {
    Connector,
    Responder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RememberedAttemptOutcome {
    pub succeeded: bool,
    pub authenticated: bool,
    pub canceled: bool,
}

impl RememberedAttemptOutcome {
    pub const fn should_stop_fallback(self) -> bool {
        self.succeeded || self.authenticated || self.canceled
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("remembered previous generation {previous} must be less than current generation {current}")]
pub struct RememberedGenerationError {
    current: u64,
    previous: u64,
}

/// Returns the bounded attempt order that lets peers recover from one
/// generation of persistence skew.
pub fn remembered_generation_attempts(
    current: u64,
    previous: Option<u64>,
    role: RememberedGenerationRole,
) -> Result<Vec<u64>, RememberedGenerationError> {
    if let Some(previous) = previous
        && previous >= current
    {
        return Err(RememberedGenerationError { current, previous });
    }

    let mut attempts = match role {
        RememberedGenerationRole::Connector => vec![current, current],
        RememberedGenerationRole::Responder => vec![current],
    };
    if let Some(previous) = previous {
        match role {
            RememberedGenerationRole::Connector => attempts.push(previous),
            RememberedGenerationRole::Responder => attempts.extend([previous, current]),
        }
    }
    Ok(attempts)
}

impl RelationshipState {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomState {
    Connecting,
    Authenticating,
    Connected,
    Closed,
}

impl RoomState {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Authenticating => "authenticating",
            Self::Connected => "connected",
            Self::Closed => "closed",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomCloseReason {
    UserEnded,
    Expired,
    PeerEnded,
    Backgrounded,
    NetworkLost,
    ProtocolFailure,
    Replaced,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Offered,
    Queued,
    Connecting,
    Transferring,
    Paused,
    AwaitingDeliveryProof,
    Delivered,
    Rejected,
    Failed,
    Canceled,
}

impl TransferState {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Queued => "queued",
            Self::Connecting => "connecting",
            Self::Transferring => "transferring",
            Self::Paused => "paused",
            Self::AwaitingDeliveryProof => "awaiting_delivery_proof",
            Self::Delivered => "delivered",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Delivered | Self::Rejected | Self::Failed | Self::Canceled
        )
    }

    pub const fn can_cancel(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Connecting | Self::Transferring | Self::Paused
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferRejection {
    UserDeclined,
    Busy,
    InsufficientSpace,
    UnsupportedContent,
    InvalidOffer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    UserCanceled,
    NetworkLost,
    AuthenticationFailed,
    RoomNotFound,
    RoomExpired,
    RoomFull,
    RoomRateLimited,
    RoomUnderAttack,
    EndpointRateLimited,
    IpRateLimited,
    ServerBusy,
    MalformedJoin,
    UnsupportedRendezvousVersion,
    UnsupportedFeature,
    InternalError,
    SenderSourceUnavailable,
    SenderPermissionLost,
    SenderSourceChanged,
    SenderItemRemoved,
    SenderCanceled,
    ProtocolOrIntegrityFailure,
    ReceiverSpaceInsufficient,
    ReceiverDestinationDecisionRequired,
    ReceiverDestinationUnavailable,
    ReceiverSaveFailed,
    ReceiverReusedObjectLost,
    ReceiverFinalizationOutcomeUnknown,
    // Compatibility values retained for application-contract fixtures v1-v5.
    SourceUnavailable,
    DestinationUnavailable,
    IntegrityFailure,
    UnsupportedVersion,
    Internal,
}

impl FailureCode {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::UserCanceled => "user_canceled",
            Self::NetworkLost => "network_lost",
            Self::AuthenticationFailed => "authentication_failed",
            Self::RoomNotFound => "room_not_found",
            Self::RoomExpired => "room_expired",
            Self::RoomFull => "room_full",
            Self::RoomRateLimited => "room_rate_limited",
            Self::RoomUnderAttack => "room_under_attack",
            Self::EndpointRateLimited => "endpoint_rate_limited",
            Self::IpRateLimited => "ip_rate_limited",
            Self::ServerBusy => "server_busy",
            Self::MalformedJoin => "malformed_join",
            Self::UnsupportedRendezvousVersion => "unsupported_rendezvous_version",
            Self::UnsupportedFeature => "unsupported_feature",
            Self::InternalError => "internal_error",
            Self::SenderSourceUnavailable => "sender_source_unavailable",
            Self::SenderPermissionLost => "sender_permission_lost",
            Self::SenderSourceChanged => "sender_source_changed",
            Self::SenderItemRemoved => "sender_item_removed",
            Self::SenderCanceled => "sender_canceled",
            Self::ProtocolOrIntegrityFailure => "protocol_or_integrity_failure",
            Self::ReceiverSpaceInsufficient => "receiver_space_insufficient",
            Self::ReceiverDestinationDecisionRequired => "receiver_destination_decision_required",
            Self::ReceiverDestinationUnavailable => "receiver_destination_unavailable",
            Self::ReceiverSaveFailed => "receiver_save_failed",
            Self::ReceiverReusedObjectLost => "receiver_reused_object_lost",
            Self::ReceiverFinalizationOutcomeUnknown => "receiver_finalization_outcome_unknown",
            Self::SourceUnavailable => "source_unavailable",
            Self::DestinationUnavailable => "destination_unavailable",
            Self::IntegrityFailure => "integrity_failure",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Internal => "internal",
        }
    }

    pub const fn category(self) -> FailureCategory {
        match self {
            Self::UserCanceled | Self::SenderItemRemoved | Self::SenderCanceled => {
                FailureCategory::User
            }
            Self::NetworkLost
            | Self::RoomNotFound
            | Self::RoomExpired
            | Self::RoomFull
            | Self::RoomRateLimited
            | Self::RoomUnderAttack
            | Self::EndpointRateLimited
            | Self::IpRateLimited
            | Self::ServerBusy => FailureCategory::Network,
            Self::AuthenticationFailed => FailureCategory::Authentication,
            Self::SenderPermissionLost => FailureCategory::Permission,
            Self::SenderSourceUnavailable
            | Self::ReceiverSpaceInsufficient
            | Self::ReceiverDestinationDecisionRequired
            | Self::ReceiverDestinationUnavailable
            | Self::ReceiverSaveFailed
            | Self::ReceiverReusedObjectLost
            | Self::ReceiverFinalizationOutcomeUnknown
            | Self::SourceUnavailable
            | Self::DestinationUnavailable => FailureCategory::Storage,
            Self::SenderSourceChanged
            | Self::ProtocolOrIntegrityFailure
            | Self::IntegrityFailure => FailureCategory::Integrity,
            Self::MalformedJoin
            | Self::UnsupportedRendezvousVersion
            | Self::UnsupportedFeature
            | Self::UnsupportedVersion => FailureCategory::Unsupported,
            Self::InternalError | Self::Internal => FailureCategory::Internal,
        }
    }

    pub const fn user_message_key(self) -> &'static str {
        match self {
            Self::UserCanceled => "transfer.user_canceled",
            Self::NetworkLost => "transfer.network_lost",
            Self::AuthenticationFailed => "transfer.authentication_failed",
            Self::RoomNotFound => "transfer.room_not_found",
            Self::RoomExpired => "transfer.room_expired",
            Self::RoomFull => "transfer.room_full",
            Self::RoomRateLimited => "transfer.room_rate_limited",
            Self::RoomUnderAttack => "transfer.room_under_attack",
            Self::EndpointRateLimited => "transfer.endpoint_rate_limited",
            Self::IpRateLimited => "transfer.ip_rate_limited",
            Self::ServerBusy => "transfer.server_busy",
            Self::MalformedJoin => "transfer.malformed_join",
            Self::UnsupportedRendezvousVersion => "transfer.unsupported_rendezvous_version",
            Self::UnsupportedFeature => "transfer.unsupported_feature",
            Self::InternalError => "transfer.internal_error",
            Self::SenderSourceUnavailable => "transfer.sender_source_unavailable",
            Self::SenderPermissionLost => "transfer.sender_permission_lost",
            Self::SenderSourceChanged => "transfer.sender_source_changed",
            Self::SenderItemRemoved => "transfer.sender_item_removed",
            Self::SenderCanceled => "transfer.sender_canceled",
            Self::ProtocolOrIntegrityFailure => "transfer.protocol_or_integrity_failure",
            Self::ReceiverSpaceInsufficient => "transfer.receiver_space_insufficient",
            Self::ReceiverDestinationDecisionRequired => {
                "transfer.receiver_destination_decision_required"
            }
            Self::ReceiverDestinationUnavailable => "transfer.receiver_destination_unavailable",
            Self::ReceiverSaveFailed => "transfer.receiver_save_failed",
            Self::ReceiverReusedObjectLost => "transfer.receiver_reused_object_lost",
            Self::ReceiverFinalizationOutcomeUnknown => {
                "transfer.receiver_finalization_outcome_unknown"
            }
            Self::SourceUnavailable => "transfer.source_unavailable",
            Self::DestinationUnavailable => "transfer.destination_unavailable",
            Self::IntegrityFailure => "transfer.integrity_failure",
            Self::UnsupportedVersion => "transfer.unsupported_version",
            Self::Internal => "transfer.internal",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Unsupported,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureOrigin {
    Local,
    Peer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Setup,
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
}

impl FailurePhase {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Pairing => "pairing",
            Self::Connecting => "connecting",
            Self::Authenticating => "authenticating",
            Self::Negotiating => "negotiating",
            Self::Transferring => "transferring",
            Self::Verifying => "verifying",
            Self::Committing => "committing",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    None,
}

impl RecoveryAction {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Resume => "resume",
            Self::ChooseFolder => "choose_folder",
            Self::OpenSettings => "open_settings",
            Self::RePair => "re_pair",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransferFailure {
    pub code: FailureCode,
    pub phase: FailurePhase,
    pub retryable: bool,
    pub recovery_action: RecoveryAction,
}

impl TransferFailure {
    pub const fn is_recoverable(&self) -> bool {
        self.retryable && !matches!(self.recovery_action, RecoveryAction::None)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub id: DeviceId,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub id: RelationshipId,
    pub device_id: DeviceId,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_generation: Option<u64>,
    pub state: RelationshipState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    pub id: RoomId,
    pub relationship_id: Option<RelationshipId>,
    pub state: RoomState,
    pub close_reason: Option<RoomCloseReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_room_id: Option<RoomId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub id: TransferId,
    pub relationship_id: RelationshipId,
    pub room_id: Option<RoomId>,
    pub content_id: ContentId,
    pub direction: TransferDirection,
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub failure: Option<TransferFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<TransferRejection>,
}
