use std::collections::BTreeMap;

use crate::model::{
    EntityKind, Relationship, RelationshipId, RelationshipState, Room, RoomCloseReason, RoomId,
    RoomState,
};
use crate::snapshot::ApplyError;

use super::{invalid_transition, missing};

pub(crate) struct OpenReduction {
    pub room: Room,
    pub replaced: Option<Room>,
}

pub(crate) fn open(
    relationships: &BTreeMap<RelationshipId, Relationship>,
    rooms: &BTreeMap<RoomId, Room>,
    room: Room,
    replaces_room_id: Option<&RoomId>,
) -> Result<OpenReduction, ApplyError> {
    if let Some(relationship_id) = &room.relationship_id {
        let relationship = relationships
            .get(relationship_id)
            .ok_or_else(|| missing(EntityKind::Relationship, relationship_id))?;
        if relationship.state != RelationshipState::Trusted {
            return Err(invalid_transition(
                EntityKind::Relationship,
                relationship_id,
                relationship.state.wire_name(),
                "room_opened",
            ));
        }
    }
    if let Some(existing) = rooms.get(&room.id) {
        return Err(invalid_transition(
            EntityKind::Room,
            &room.id,
            existing.state.wire_name(),
            "room_opened",
        ));
    }

    let replaced = match replaces_room_id {
        Some(replaced_id) => {
            let mut replaced = current(rooms.get(replaced_id), replaced_id)?;
            if replaced.state == RoomState::Closed {
                return Err(invalid_transition(
                    EntityKind::Room,
                    replaced_id,
                    replaced.state.wire_name(),
                    "room_opened",
                ));
            }
            if replaced.relationship_id != room.relationship_id {
                return Err(ApplyError::InvalidReference {
                    entity: EntityKind::Room,
                    id: replaced_id.to_string(),
                    field: "relationship_id",
                });
            }
            replaced.state = RoomState::Closed;
            replaced.close_reason = Some(RoomCloseReason::Replaced);
            replaced.replacement_room_id = Some(room.id.clone());
            Some(replaced)
        }
        None => {
            if let Some(relationship_id) = &room.relationship_id
                && let Some(existing) = rooms.values().find(|existing| {
                    existing.relationship_id.as_ref() == Some(relationship_id)
                        && existing.state != RoomState::Closed
                })
            {
                return Err(invalid_transition(
                    EntityKind::Room,
                    &existing.id,
                    existing.state.wire_name(),
                    "room_opened_without_replacement",
                ));
            }
            None
        }
    };
    Ok(OpenReduction { room, replaced })
}

pub(crate) fn admit(existing: Option<&Room>, room_id: &RoomId) -> Result<Room, ApplyError> {
    let mut room = current(existing, room_id)?;
    if room.state != RoomState::Connecting {
        return Err(invalid_transition(
            EntityKind::Room,
            room_id,
            room.state.wire_name(),
            "room_peer_admitted",
        ));
    }
    room.state = RoomState::Authenticating;
    Ok(room)
}

pub(crate) fn authenticate(existing: Option<&Room>, room_id: &RoomId) -> Result<Room, ApplyError> {
    let mut room = current(existing, room_id)?;
    if room.state != RoomState::Authenticating {
        return Err(invalid_transition(
            EntityKind::Room,
            room_id,
            room.state.wire_name(),
            "room_authenticated",
        ));
    }
    room.state = RoomState::Connected;
    Ok(room)
}

pub(crate) fn close(
    existing: Option<&Room>,
    room_id: &RoomId,
    reason: RoomCloseReason,
) -> Result<Room, ApplyError> {
    let mut room = current(existing, room_id)?;
    if room.state == RoomState::Closed {
        return Err(invalid_transition(
            EntityKind::Room,
            room_id,
            room.state.wire_name(),
            "room_closed",
        ));
    }
    room.state = RoomState::Closed;
    room.close_reason = Some(reason);
    Ok(room)
}

fn current(existing: Option<&Room>, room_id: &RoomId) -> Result<Room, ApplyError> {
    existing
        .cloned()
        .ok_or_else(|| missing(EntityKind::Room, room_id))
}
