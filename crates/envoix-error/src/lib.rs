//! Shared error categories.

use thiserror::Error;

/// Stable machine cause for recovery decisions. Human-readable diagnostics
/// are carried separately and must never be parsed to recover this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferCause {
    NearbyHybridPreAuthTransportFailure,
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
}

impl TransferCause {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NearbyHybridPreAuthTransportFailure => "nearby_hybrid_pre_auth_transport_failure",
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
        }
    }
}

/// Stable broker-owned Room outcome. Recovery code must match this value
/// directly and must never parse a diagnostic string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendezvousCause {
    RoomNotFound,
    RoomExpired,
    RoomFull,
    RoomRateLimited,
    RoomUnderAttack,
    EndpointRateLimited,
    IpRateLimited,
    ServerBusy,
    MalformedJoin,
    UnsupportedVersion,
}

impl RendezvousCause {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RoomNotFound => "room_not_found",
            Self::RoomExpired => "room_expired",
            Self::RoomFull => "room_full",
            Self::RoomRateLimited => "room_rate_limited",
            Self::RoomUnderAttack => "room_under_attack",
            Self::EndpointRateLimited => "endpoint_rate_limited",
            Self::IpRateLimited => "ip_rate_limited",
            Self::ServerBusy => "server_busy",
            Self::MalformedJoin => "malformed_join",
            Self::UnsupportedVersion => "unsupported_version",
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("discovery error: {0}")]
    Discovery(String),
    #[error("transfer error: {0}")]
    Transfer(String),
    #[error("{cause}: {detail}", cause = .cause.code())]
    Cause {
        cause: TransferCause,
        detail: String,
    },
    #[error(
        "rendezvous {cause_code}{retry}",
        cause_code = .cause.code(),
        retry = retry_suffix(*retry_after)
    )]
    Rendezvous {
        cause: RendezvousCause,
        retry_after: Option<u64>,
    },
    #[error("one-time invitation was consumed after authentication: {0}")]
    InvitationConsumed(#[source] Box<CoreError>),
    #[error("operation cancelled")]
    Cancelled,
}

fn retry_suffix(retry_after: Option<u64>) -> String {
    retry_after.map_or_else(String::new, |seconds| format!(" (retry after {seconds}s)"))
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
