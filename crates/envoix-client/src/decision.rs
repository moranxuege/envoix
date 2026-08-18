//! Pure command validation and effect selection.

use crate::APPLICATION_CONTRACT_VERSION;
use crate::command::{CommandEnvelope, EngineCommand};
use crate::effect::{EffectEnvelope, EngineEffect};
use crate::model::{
    EntityKind, Relationship, RelationshipId, RelationshipState, RoomId, RoomState, Transfer,
    TransferId, TransferState,
};
use crate::reducers::{invalid_transition, missing};
use crate::snapshot::{ApplyError, EngineSnapshot};

pub fn decide(
    snapshot: &EngineSnapshot,
    envelope: CommandEnvelope,
) -> Result<EffectEnvelope, ApplyError> {
    snapshot.validate_contract()?;
    if envelope.contract_version != APPLICATION_CONTRACT_VERSION {
        return Err(ApplyError::UnsupportedContractVersion {
            expected: APPLICATION_CONTRACT_VERSION,
            actual: envelope.contract_version,
        });
    }

    let effect = match envelope.command {
        EngineCommand::CreateRoom => EngineEffect::CreateRoom,
        EngineCommand::JoinRoom { invitation } => EngineEffect::JoinRoom { invitation },
        EngineCommand::VerifyPairing {
            room_id,
            verification_code,
        } => {
            require_room_state(snapshot, &room_id, RoomState::Connected, "verify_pairing")?;
            EngineEffect::VerifyPairing {
                room_id,
                verification_code,
            }
        }
        EngineCommand::ReconnectRelationship { relationship_id } => {
            let relationship = trusted_relationship(snapshot, &relationship_id, "reconnect")?;
            EngineEffect::ReconnectRelationship {
                relationship_id,
                generation: relationship.generation,
                previous_generation: relationship.previous_generation,
            }
        }
        EngineCommand::CreateTransfer {
            relationship_id,
            content_id,
            direction,
        } => {
            trusted_relationship(snapshot, &relationship_id, "create_transfer")?;
            EngineEffect::CreateTransfer {
                relationship_id,
                content_id,
                direction,
            }
        }
        EngineCommand::AcceptTransfer { transfer_id } => {
            require_transfer_state(
                snapshot,
                &transfer_id,
                TransferState::Offered,
                "accept_transfer",
            )?;
            EngineEffect::AcceptTransfer { transfer_id }
        }
        EngineCommand::RejectTransfer {
            transfer_id,
            reason,
        } => {
            require_transfer_state(
                snapshot,
                &transfer_id,
                TransferState::Offered,
                "reject_transfer",
            )?;
            EngineEffect::RejectTransfer {
                transfer_id,
                reason,
            }
        }
        EngineCommand::PauseTransfer { transfer_id } => {
            let transfer = transfer(snapshot, &transfer_id)?;
            if !matches!(
                transfer.state,
                TransferState::Connecting | TransferState::Transferring
            ) {
                return Err(invalid_transition(
                    EntityKind::Transfer,
                    &transfer_id,
                    transfer.state.wire_name(),
                    "pause_transfer",
                ));
            }
            EngineEffect::PauseTransfer { transfer_id }
        }
        EngineCommand::ResumeTransfer { transfer_id } => {
            require_transfer_state(
                snapshot,
                &transfer_id,
                TransferState::Paused,
                "resume_transfer",
            )?;
            EngineEffect::ResumeTransfer { transfer_id }
        }
        EngineCommand::CancelTransfer { transfer_id } => {
            let transfer = transfer(snapshot, &transfer_id)?;
            if transfer.state.is_terminal() {
                return Err(invalid_transition(
                    EntityKind::Transfer,
                    &transfer_id,
                    transfer.state.wire_name(),
                    "cancel_transfer",
                ));
            }
            EngineEffect::CancelTransfer { transfer_id }
        }
        EngineCommand::RevokeRelationship { relationship_id } => {
            trusted_relationship(snapshot, &relationship_id, "revoke_relationship")?;
            EngineEffect::RevokeRelationship { relationship_id }
        }
    };

    Ok(EffectEnvelope {
        contract_version: APPLICATION_CONTRACT_VERSION,
        command_id: envelope.command_id,
        effect,
    })
}

fn trusted_relationship<'a>(
    snapshot: &'a EngineSnapshot,
    relationship_id: &RelationshipId,
    command: &'static str,
) -> Result<&'a Relationship, ApplyError> {
    let relationship = snapshot
        .relationships
        .get(relationship_id)
        .ok_or_else(|| missing(EntityKind::Relationship, relationship_id))?;
    if relationship.state != RelationshipState::Trusted {
        return Err(invalid_transition(
            EntityKind::Relationship,
            relationship_id,
            relationship.state.wire_name(),
            command,
        ));
    }
    Ok(relationship)
}

fn require_room_state(
    snapshot: &EngineSnapshot,
    room_id: &RoomId,
    expected: RoomState,
    command: &'static str,
) -> Result<(), ApplyError> {
    let room = snapshot
        .rooms
        .get(room_id)
        .ok_or_else(|| missing(EntityKind::Room, room_id))?;
    if room.state != expected {
        return Err(invalid_transition(
            EntityKind::Room,
            room_id,
            room.state.wire_name(),
            command,
        ));
    }
    Ok(())
}

fn transfer<'a>(
    snapshot: &'a EngineSnapshot,
    transfer_id: &TransferId,
) -> Result<&'a Transfer, ApplyError> {
    snapshot
        .transfers
        .get(transfer_id)
        .ok_or_else(|| missing(EntityKind::Transfer, transfer_id))
}

fn require_transfer_state(
    snapshot: &EngineSnapshot,
    transfer_id: &TransferId,
    expected: TransferState,
    command: &'static str,
) -> Result<(), ApplyError> {
    let transfer = transfer(snapshot, transfer_id)?;
    if transfer.state != expected {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            command,
        ));
    }
    Ok(())
}
