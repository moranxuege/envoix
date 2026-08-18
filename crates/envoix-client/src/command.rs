//! Typed application commands.
//!
//! Invitation and verification values intentionally do not implement
//! `Debug`; a command must never disclose them through routine diagnostics.

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::model::{
    CommandId, ContentId, RelationshipId, RoomId, TransferDirection, TransferId, TransferRejection,
};

pub const MAX_ROOM_INVITATION_BYTES: usize = 16 * 1024;
const VERIFICATION_CODE_DIGITS: usize = 6;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CommandValueError {
    #[error("room invitation must contain 1 to {MAX_ROOM_INVITATION_BYTES} bytes")]
    InvalidRoomInvitation,
    #[error("verification code must contain exactly {VERIFICATION_CODE_DIGITS} ASCII digits")]
    InvalidVerificationCode,
}

/// Opaque room invitation which is cleared from memory on drop.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RoomInvitation(Zeroizing<String>);

impl RoomInvitation {
    pub fn parse(value: impl Into<String>) -> Result<Self, CommandValueError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > MAX_ROOM_INVITATION_BYTES {
            return Err(CommandValueError::InvalidRoomInvitation);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    /// Exposes the invitation only for the immediate parsing operation.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for RoomInvitation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Six-digit pairing confirmation which is cleared from memory on drop.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct VerificationCode(Zeroizing<String>);

impl VerificationCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, CommandValueError> {
        let value = value.into();
        if value.len() != VERIFICATION_CODE_DIGITS
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(CommandValueError::InvalidVerificationCode);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    /// Exposes the code only for the immediate verification operation.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for VerificationCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

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
        invitation: RoomInvitation,
    },
    VerifyPairing {
        room_id: RoomId,
        verification_code: VerificationCode,
    },
    ReconnectRelationship {
        relationship_id: RelationshipId,
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
    CancelTransfer {
        transfer_id: TransferId,
    },
    RevokeRelationship {
        relationship_id: RelationshipId,
    },
}
