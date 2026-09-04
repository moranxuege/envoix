use std::collections::BTreeMap;

use crate::model::{Device, DeviceId, EntityKind, Relationship, RelationshipId, RelationshipState};
use crate::snapshot::ApplyError;

use super::{invalid_transition, missing};

pub(crate) fn trust(
    devices: &BTreeMap<DeviceId, Device>,
    existing: Option<&Relationship>,
    relationship: Relationship,
) -> Result<Relationship, ApplyError> {
    if !devices.contains_key(&relationship.device_id) {
        return Err(missing(EntityKind::Device, &relationship.device_id));
    }
    if let Some(existing) = existing {
        return Err(invalid_transition(
            EntityKind::Relationship,
            &relationship.id,
            existing.state.wire_name(),
            "relationship_trusted",
        ));
    }
    Ok(relationship)
}

pub(crate) fn revoke(
    existing: Option<&Relationship>,
    relationship_id: &RelationshipId,
) -> Result<Relationship, ApplyError> {
    let mut relationship = existing
        .cloned()
        .ok_or_else(|| missing(EntityKind::Relationship, relationship_id))?;
    if relationship.state != RelationshipState::Trusted {
        return Err(invalid_transition(
            EntityKind::Relationship,
            relationship_id,
            relationship.state.wire_name(),
            "relationship_revoked",
        ));
    }
    relationship.state = RelationshipState::Revoked;
    Ok(relationship)
}

pub(crate) fn rotate(
    existing: Option<&Relationship>,
    relationship_id: &RelationshipId,
    generation: u64,
) -> Result<Relationship, ApplyError> {
    let mut relationship = existing
        .cloned()
        .ok_or_else(|| missing(EntityKind::Relationship, relationship_id))?;
    if relationship.state != RelationshipState::Trusted {
        return Err(invalid_transition(
            EntityKind::Relationship,
            relationship_id,
            relationship.state.wire_name(),
            "relationship_rotated",
        ));
    }
    if generation < relationship.generation {
        return Err(ApplyError::GenerationMismatch {
            relationship_id: relationship_id.clone(),
            current_generation: relationship.generation,
            attempted_generation: generation,
        });
    }
    if generation > relationship.generation {
        relationship.previous_generation = Some(relationship.generation);
        relationship.generation = generation;
    }
    Ok(relationship)
}
