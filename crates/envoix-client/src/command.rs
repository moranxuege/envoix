//! Typed application commands.
//!
//! Invitation and verification values intentionally do not implement
//! `Debug`; a command must never disclose them through routine diagnostics.

use serde::{Deserialize, Serialize};

use crate::model::{CommandId, ContentId, RelationshipId, RoomId, TransferDirection, TransferId};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    pub contract_version: u16,
    pub command_id: CommandId,
    pub command: EngineCommand,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineCommand {
    CreateRoom,
    JoinRoom {
        invitation: String,
    },
    VerifyPairing {
        room_id: RoomId,
        verification_code: String,
    },
    ReconnectRelationship {
        relationship_id: RelationshipId,
    },
    CreateTransfer {
        relationship_id: RelationshipId,
        content_id: ContentId,
        direction: TransferDirection,
    },
    PauseTransfer {
        transfer_id: TransferId,
    },
    ResumeTransfer {
        transfer_id: TransferId,
    },
    CancelTransfer {
        transfer_id: TransferId,
    },
    RevokeRelationship {
        relationship_id: RelationshipId,
    },
}
