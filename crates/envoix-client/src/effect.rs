//! Typed work selected by the pure application command decision layer.
//!
//! Effects are executed only for live commands. Event replay never executes
//! them. Invitation and verification values intentionally remain non-`Debug`
//! so routine diagnostics cannot disclose them.

use serde::{Deserialize, Serialize};

use crate::command::{RoomInvitation, VerificationCode};
use crate::model::{
    CommandId, ContentId, RecoveryAction, RelationshipId, RoomId, TransferDirection, TransferId,
    TransferRejection,
};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectEnvelope {
    pub contract_version: u16,
    pub command_id: CommandId,
    pub effect: EngineEffect,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "effect", rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineEffect {
    CreateRoom,
    JoinRoom {
        invitation: RoomInvitation,
    },
    VerifyPairing {
        room_id: RoomId,
        verification_code: VerificationCode,
    },
    ReconnectRelationship {
        relationship_id: RelationshipId,
        generation: u64,
        previous_generation: Option<u64>,
    },
    CreateTransfer {
        relationship_id: RelationshipId,
        content_id: ContentId,
        direction: TransferDirection,
    },
    AcceptTransfer {
        transfer_id: TransferId,
    },
    RejectTransfer {
        transfer_id: TransferId,
        reason: TransferRejection,
    },
    PauseTransfer {
        transfer_id: TransferId,
    },
    ResumeTransfer {
        transfer_id: TransferId,
    },
    RecoverTransfer {
        transfer_id: TransferId,
        action: RecoveryAction,
    },
    CancelTransfer {
        transfer_id: TransferId,
    },
    RemoveTransfer {
        transfer_id: TransferId,
    },
    RevokeRelationship {
        relationship_id: RelationshipId,
    },
}
