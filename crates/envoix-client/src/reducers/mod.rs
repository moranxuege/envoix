//! Pure aggregate reducers used by the ordered application snapshot.
//!
//! These modules validate and return replacement values. They never perform
//! persistence, networking, or platform effects.

use std::fmt;

use crate::model::EntityKind;
use crate::snapshot::ApplyError;

pub(crate) mod relationship;
pub(crate) mod room;
pub(crate) mod transfer;

pub(crate) fn missing(entity: EntityKind, id: &impl fmt::Display) -> ApplyError {
    ApplyError::MissingEntity {
        entity,
        id: id.to_string(),
    }
}

pub(crate) fn invalid_transition(
    entity: EntityKind,
    id: &impl fmt::Display,
    state: &'static str,
    event: &'static str,
) -> ApplyError {
    ApplyError::InvalidTransition {
        entity,
        id: id.to_string(),
        state,
        event,
    }
}
