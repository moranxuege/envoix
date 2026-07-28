//! Strict V2 control protocol between a peer and the broker.

use envoix_invite::{BootstrapKind, InvitationSide, TransferRole};
use serde::{Deserialize, Serialize};

/// Current rendezvous join protocol version.
pub const RENDEZVOUS_PROTOCOL_VERSION: u32 = 2;

/// SPAKE2 connection role assigned from invitation side.
///
/// The invitation joiner is always the initiator and the creator is always the
/// responder, independent of arrival order and transfer direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Initiator,
    Responder,
}

/// A peer's strict V2 opening message.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub version: u32,
    pub room_id: String,
    pub invitation_side: InvitationSide,
    pub transfer_role: TransferRole,
    /// Creator advertisement. Joiners send an empty list.
    pub bootstrap_methods: Vec<BootstrapKind>,
    /// Carrier selection. Creators send `None`.
    pub selected_bootstrap_method: Option<BootstrapKind>,
}

/// The broker's reply once a compatible partner is present.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Paired {
    pub role: Role,
    pub selected_bootstrap_method: BootstrapKind,
}

/// Stable broker-owned outcome for a rejected Room operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerOutcome {
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

impl BrokerOutcome {
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

/// Structured rejection. `retry_after` is whole seconds and is always bounded
/// by both server and client policy.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, thiserror::Error)]
#[error("{outcome_code}", outcome_code = .outcome.code())]
#[serde(deny_unknown_fields)]
pub struct BrokerRejection {
    pub outcome: BrokerOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

/// The broker's reply to a [`Join`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Reply {
    Paired(Paired),
    Rejected(BrokerRejection),
    /// Retained on the wire for protocol-V2 clients. New clients normalize it
    /// to `room_expired`.
    Expired,
}
