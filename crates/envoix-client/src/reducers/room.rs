use std::collections::BTreeMap;

use crate::model::{
    EntityKind, Relationship, RelationshipId, RelationshipState, Room, RoomCloseReason, RoomId,
    RoomState,
};
use crate::snapshot::ApplyError;

use super::{invalid_transition, missing};

pub(crate) fn open(
    relationships: &BTreeMap<RelationshipId, Relationship>,
    existing: Option<&Room>,
    room: Room,
) -> Result<Room, ApplyError> {
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
    if let Some(existing) = existing {
        return Err(invalid_transition(
            EntityKind::Room,
            &room.id,
            existing.state.wire_name(),
            "room_opened",
        ));
    }
    Ok(room)
}

pub(crate) fn connect(existing: Option<&Room>, room_id: &RoomId) -> Result<Room, ApplyError> {
    let mut room = current(existing, room_id)?;
    if room.state != RoomState::Connecting {
        return Err(invalid_transition(
            EntityKind::Room,
            room_id,
            room.state.wire_name(),
            "room_connected",
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
