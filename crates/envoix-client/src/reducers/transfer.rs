use std::collections::BTreeMap;

use crate::model::{
    EntityKind, Relationship, RelationshipId, RelationshipState, Room, RoomId, RoomState, Transfer,
    TransferFailure, TransferId, TransferRejection, TransferState,
};
use crate::snapshot::ApplyError;

use super::{invalid_transition, missing};

pub(crate) fn create(
    relationships: &BTreeMap<RelationshipId, Relationship>,
    rooms: &BTreeMap<RoomId, Room>,
    existing: Option<&Transfer>,
    transfer: Transfer,
) -> Result<Transfer, ApplyError> {
    validate_new(
        relationships,
        rooms,
        existing,
        &transfer,
        "transfer_created",
    )?;
    Ok(transfer)
}

pub(crate) fn offer(
    relationships: &BTreeMap<RelationshipId, Relationship>,
    rooms: &BTreeMap<RoomId, Room>,
    existing: Option<&Transfer>,
    transfer: Transfer,
) -> Result<Transfer, ApplyError> {
    validate_new(
        relationships,
        rooms,
        existing,
        &transfer,
        "transfer_offered",
    )?;
    Ok(transfer)
}

fn validate_new(
    relationships: &BTreeMap<RelationshipId, Relationship>,
    rooms: &BTreeMap<RoomId, Room>,
    existing: Option<&Transfer>,
    transfer: &Transfer,
    operation: &'static str,
) -> Result<(), ApplyError> {
    let relationship = relationships
        .get(&transfer.relationship_id)
        .ok_or_else(|| missing(EntityKind::Relationship, &transfer.relationship_id))?;
    if relationship.state != RelationshipState::Trusted {
        return Err(invalid_transition(
            EntityKind::Relationship,
            &transfer.relationship_id,
            relationship.state.wire_name(),
            operation,
        ));
    }
    if let Some(room_id) = &transfer.room_id {
        let room = rooms
            .get(room_id)
            .ok_or_else(|| missing(EntityKind::Room, room_id))?;
        if room.state != RoomState::Connected {
            return Err(invalid_transition(
                EntityKind::Room,
                room_id,
                room.state.wire_name(),
                operation,
            ));
        }
        if room
            .relationship_id
            .as_ref()
            .is_some_and(|value| value != &transfer.relationship_id)
        {
            return Err(ApplyError::InvalidReference {
                entity: EntityKind::Room,
                id: room_id.to_string(),
                field: "relationship_id",
            });
        }
    }
    if let Some(existing) = existing {
        return Err(invalid_transition(
            EntityKind::Transfer,
            &transfer.id,
            existing.state.wire_name(),
            operation,
        ));
    }
    Ok(())
}

pub(crate) fn accept(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    require_state(&transfer, TransferState::Offered, "transfer_accepted")?;
    transfer.state = TransferState::Queued;
    Ok(transfer)
}

pub(crate) fn reject(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
    reason: TransferRejection,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    require_state(&transfer, TransferState::Offered, "transfer_rejected")?;
    transfer.state = TransferState::Rejected;
    transfer.rejection = Some(reason);
    Ok(transfer)
}

pub(crate) fn start(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    require_state(&transfer, TransferState::Queued, "transfer_started")?;
    transfer.state = TransferState::Connecting;
    Ok(transfer)
}

pub(crate) fn progress(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
    transferred_bytes: u64,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if !matches!(
        transfer.state,
        TransferState::Connecting | TransferState::Transferring
    ) {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_progressed",
        ));
    }
    if transferred_bytes < transfer.transferred_bytes || transferred_bytes > transfer.total_bytes {
        return Err(ApplyError::InvalidProgress {
            transfer_id: transfer_id.clone(),
            previous_bytes: transfer.transferred_bytes,
            transferred_bytes,
            total_bytes: transfer.total_bytes,
        });
    }
    transfer.state = TransferState::Transferring;
    transfer.transferred_bytes = transferred_bytes;
    Ok(transfer)
}

pub(crate) fn pause(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if !matches!(
        transfer.state,
        TransferState::Connecting | TransferState::Transferring
    ) {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_paused",
        ));
    }
    transfer.state = TransferState::Paused;
    Ok(transfer)
}

pub(crate) fn resume(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    require_state(&transfer, TransferState::Paused, "transfer_resumed")?;
    transfer.state = TransferState::Connecting;
    transfer.failure = None;
    Ok(transfer)
}

pub(crate) fn recover(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if transfer.state != TransferState::Failed
        || !transfer
            .failure
            .as_ref()
            .is_some_and(TransferFailure::is_recoverable)
    {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_recovery_started",
        ));
    }
    transfer.state = TransferState::Connecting;
    transfer.failure = None;
    Ok(transfer)
}

pub(crate) fn complete_payload(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if !matches!(
        transfer.state,
        TransferState::Connecting | TransferState::Transferring
    ) {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_payload_completed",
        ));
    }
    if transfer.transferred_bytes != transfer.total_bytes {
        return Err(ApplyError::InvalidProgress {
            transfer_id: transfer_id.clone(),
            previous_bytes: transfer.transferred_bytes,
            transferred_bytes: transfer.transferred_bytes,
            total_bytes: transfer.total_bytes,
        });
    }
    transfer.state = TransferState::AwaitingDeliveryProof;
    Ok(transfer)
}

pub(crate) fn prove_delivery(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    require_state(
        &transfer,
        TransferState::AwaitingDeliveryProof,
        "transfer_delivery_proof_verified",
    )?;
    transfer.state = TransferState::Delivered;
    transfer.failure = None;
    Ok(transfer)
}

pub(crate) fn fail(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
    failure: TransferFailure,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if transfer.state.is_terminal() {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_failed",
        ));
    }
    transfer.state = TransferState::Failed;
    transfer.failure = Some(failure);
    Ok(transfer)
}

pub(crate) fn cancel(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<Transfer, ApplyError> {
    let mut transfer = current(existing, transfer_id)?;
    if !transfer.state.can_cancel() {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_canceled",
        ));
    }
    transfer.state = TransferState::Canceled;
    transfer.failure = None;
    Ok(transfer)
}

pub(crate) fn remove(
    existing: Option<&Transfer>,
    transfer_id: &TransferId,
) -> Result<(), ApplyError> {
    let transfer = current(existing, transfer_id)?;
    if !transfer.state.is_terminal() {
        return Err(invalid_transition(
            EntityKind::Transfer,
            transfer_id,
            transfer.state.wire_name(),
            "transfer_removed",
        ));
    }
    Ok(())
}

fn current(existing: Option<&Transfer>, transfer_id: &TransferId) -> Result<Transfer, ApplyError> {
    existing
        .cloned()
        .ok_or_else(|| missing(EntityKind::Transfer, transfer_id))
}

fn require_state(
    transfer: &Transfer,
    expected: TransferState,
    event: &'static str,
) -> Result<(), ApplyError> {
    if transfer.state != expected {
        return Err(invalid_transition(
            EntityKind::Transfer,
            &transfer.id,
            transfer.state.wire_name(),
            event,
        ));
    }
    Ok(())
}
