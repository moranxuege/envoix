//! Immutable application snapshots and their ordered event reducer.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::APPLICATION_CONTRACT_VERSION;
use crate::event::{EngineEvent, EventEnvelope};
use crate::model::{
    Device, DeviceId, EntityKind, Relationship, RelationshipId, RelationshipState, Room, RoomId,
    RoomState, Transfer, TransferId, TransferState,
};
use crate::ports::PlatformCapabilities;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    IgnoredDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationErrorCode {
    UnsupportedContractVersion,
    InvalidSequence,
    EventGap,
    EntityNotFound,
    InvalidReference,
    InvalidTransition,
    InvalidProgress,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ApplyError {
    #[error("unsupported application contract version {actual}; expected {expected}")]
    UnsupportedContractVersion { expected: u16, actual: u16 },
    #[error("application event sequence must be non-zero, got {actual}")]
    InvalidSequence { actual: u64 },
    #[error("application event gap: expected sequence {expected}, got {actual}")]
    SequenceGap { expected: u64, actual: u64 },
    #[error("{entity:?} {id} does not exist")]
    MissingEntity { entity: EntityKind, id: String },
    #[error("{entity:?} {id} has an invalid {field} reference")]
    InvalidReference {
        entity: EntityKind,
        id: String,
        field: &'static str,
    },
    #[error("cannot apply {event} to {entity:?} {id} in state {state}")]
    InvalidTransition {
        entity: EntityKind,
        id: String,
        state: &'static str,
        event: &'static str,
    },
    #[error(
        "transfer {transfer_id} progress {transferred_bytes} is invalid after {previous_bytes} of {total_bytes}"
    )]
    InvalidProgress {
        transfer_id: TransferId,
        previous_bytes: u64,
        transferred_bytes: u64,
        total_bytes: u64,
    },
}

impl ApplyError {
    pub const fn code(&self) -> ApplicationErrorCode {
        match self {
            Self::UnsupportedContractVersion { .. } => {
                ApplicationErrorCode::UnsupportedContractVersion
            }
            Self::InvalidSequence { .. } => ApplicationErrorCode::InvalidSequence,
            Self::SequenceGap { .. } => ApplicationErrorCode::EventGap,
            Self::MissingEntity { .. } => ApplicationErrorCode::EntityNotFound,
            Self::InvalidReference { .. } => ApplicationErrorCode::InvalidReference,
            Self::InvalidTransition { .. } => ApplicationErrorCode::InvalidTransition,
            Self::InvalidProgress { .. } => ApplicationErrorCode::InvalidProgress,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSnapshot {
    pub contract_version: u16,
    pub last_sequence: u64,
    pub capabilities: PlatformCapabilities,
    pub devices: BTreeMap<DeviceId, Device>,
    pub relationships: BTreeMap<RelationshipId, Relationship>,
    pub rooms: BTreeMap<RoomId, Room>,
    pub transfers: BTreeMap<TransferId, Transfer>,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineSnapshot {
    pub fn new() -> Self {
        Self {
            contract_version: APPLICATION_CONTRACT_VERSION,
            last_sequence: 0,
            capabilities: PlatformCapabilities::default(),
            devices: BTreeMap::new(),
            relationships: BTreeMap::new(),
            rooms: BTreeMap::new(),
            transfers: BTreeMap::new(),
        }
    }

    pub fn validate_contract(&self) -> Result<(), ApplyError> {
        validate_contract_version(self.contract_version)
    }

    pub fn apply(&mut self, envelope: EventEnvelope) -> Result<ApplyOutcome, ApplyError> {
        self.validate_contract()?;
        validate_contract_version(envelope.contract_version)?;
        if envelope.sequence == 0 {
            return Err(ApplyError::InvalidSequence {
                actual: envelope.sequence,
            });
        }
        if envelope.sequence <= self.last_sequence {
            return Ok(ApplyOutcome::IgnoredDuplicate);
        }
        let expected = self.last_sequence + 1;
        if envelope.sequence > expected {
            return Err(ApplyError::SequenceGap {
                expected,
                actual: envelope.sequence,
            });
        }

        self.reduce(envelope.event)?;
        self.last_sequence = envelope.sequence;
        Ok(ApplyOutcome::Applied)
    }

    fn reduce(&mut self, event: EngineEvent) -> Result<(), ApplyError> {
        match event {
            EngineEvent::CapabilitiesChanged { capabilities } => {
                self.capabilities = capabilities;
            }
            EngineEvent::DeviceObserved {
                device_id,
                display_name,
            } => {
                self.devices.insert(
                    device_id.clone(),
                    Device {
                        id: device_id,
                        display_name,
                    },
                );
            }
            EngineEvent::RelationshipTrusted {
                relationship_id,
                device_id,
                generation,
            } => {
                if !self.devices.contains_key(&device_id) {
                    return Err(missing(EntityKind::Device, &device_id));
                }
                if let Some(existing) = self.relationships.get(&relationship_id) {
                    return Err(invalid_transition(
                        EntityKind::Relationship,
                        &relationship_id,
                        existing.state.wire_name(),
                        "relationship_trusted",
                    ));
                }
                self.relationships.insert(
                    relationship_id.clone(),
                    Relationship {
                        id: relationship_id,
                        device_id,
                        generation,
                        state: RelationshipState::Trusted,
                    },
                );
            }
            EngineEvent::RelationshipRevoked { relationship_id } => {
                let relationship = self
                    .relationships
                    .get_mut(&relationship_id)
                    .ok_or_else(|| missing(EntityKind::Relationship, &relationship_id))?;
                if relationship.state != RelationshipState::Trusted {
                    return Err(invalid_transition(
                        EntityKind::Relationship,
                        &relationship_id,
                        relationship.state.wire_name(),
                        "relationship_revoked",
                    ));
                }
                relationship.state = RelationshipState::Revoked;
            }
            EngineEvent::RoomOpened {
                room_id,
                relationship_id,
            } => {
                if let Some(relationship_id) = &relationship_id {
                    let relationship = self
                        .relationships
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
                if let Some(existing) = self.rooms.get(&room_id) {
                    return Err(invalid_transition(
                        EntityKind::Room,
                        &room_id,
                        existing.state.wire_name(),
                        "room_opened",
                    ));
                }
                self.rooms.insert(
                    room_id.clone(),
                    Room {
                        id: room_id,
                        relationship_id,
                        state: RoomState::Connecting,
                        close_reason: None,
                    },
                );
            }
            EngineEvent::RoomConnected { room_id } => {
                let room = self
                    .rooms
                    .get_mut(&room_id)
                    .ok_or_else(|| missing(EntityKind::Room, &room_id))?;
                if room.state != RoomState::Connecting {
                    return Err(invalid_transition(
                        EntityKind::Room,
                        &room_id,
                        room.state.wire_name(),
                        "room_connected",
                    ));
                }
                room.state = RoomState::Connected;
            }
            EngineEvent::RoomClosed { room_id, reason } => {
                let room = self
                    .rooms
                    .get_mut(&room_id)
                    .ok_or_else(|| missing(EntityKind::Room, &room_id))?;
                if room.state == RoomState::Closed {
                    return Err(invalid_transition(
                        EntityKind::Room,
                        &room_id,
                        room.state.wire_name(),
                        "room_closed",
                    ));
                }
                room.state = RoomState::Closed;
                room.close_reason = Some(reason);
            }
            EngineEvent::TransferCreated {
                transfer_id,
                relationship_id,
                room_id,
                content_id,
                direction,
                total_bytes,
            } => {
                let relationship = self
                    .relationships
                    .get(&relationship_id)
                    .ok_or_else(|| missing(EntityKind::Relationship, &relationship_id))?;
                if relationship.state != RelationshipState::Trusted {
                    return Err(invalid_transition(
                        EntityKind::Relationship,
                        &relationship_id,
                        relationship.state.wire_name(),
                        "transfer_created",
                    ));
                }
                if let Some(room_id) = &room_id {
                    let room = self
                        .rooms
                        .get(room_id)
                        .ok_or_else(|| missing(EntityKind::Room, room_id))?;
                    if room.state != RoomState::Connected {
                        return Err(invalid_transition(
                            EntityKind::Room,
                            room_id,
                            room.state.wire_name(),
                            "transfer_created",
                        ));
                    }
                    if room
                        .relationship_id
                        .as_ref()
                        .is_some_and(|value| value != &relationship_id)
                    {
                        return Err(ApplyError::InvalidReference {
                            entity: EntityKind::Room,
                            id: room_id.to_string(),
                            field: "relationship_id",
                        });
                    }
                }
                if let Some(existing) = self.transfers.get(&transfer_id) {
                    return Err(invalid_transition(
                        EntityKind::Transfer,
                        &transfer_id,
                        existing.state.wire_name(),
                        "transfer_created",
                    ));
                }
                self.transfers.insert(
                    transfer_id.clone(),
                    Transfer {
                        id: transfer_id,
                        relationship_id,
                        room_id,
                        content_id,
                        direction,
                        state: TransferState::Queued,
                        transferred_bytes: 0,
                        total_bytes,
                        failure: None,
                    },
                );
            }
            EngineEvent::TransferStarted { transfer_id } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                require_transfer_state(transfer, TransferState::Queued, "transfer_started")?;
                transfer.state = TransferState::Connecting;
            }
            EngineEvent::TransferProgressed {
                transfer_id,
                transferred_bytes,
            } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                if !matches!(
                    transfer.state,
                    TransferState::Connecting | TransferState::Transferring
                ) {
                    return Err(invalid_transition(
                        EntityKind::Transfer,
                        &transfer_id,
                        transfer.state.wire_name(),
                        "transfer_progressed",
                    ));
                }
                if transferred_bytes < transfer.transferred_bytes
                    || transferred_bytes > transfer.total_bytes
                {
                    return Err(ApplyError::InvalidProgress {
                        transfer_id,
                        previous_bytes: transfer.transferred_bytes,
                        transferred_bytes,
                        total_bytes: transfer.total_bytes,
                    });
                }
                transfer.state = TransferState::Transferring;
                transfer.transferred_bytes = transferred_bytes;
            }
            EngineEvent::TransferPaused { transfer_id } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                if !matches!(
                    transfer.state,
                    TransferState::Connecting | TransferState::Transferring
                ) {
                    return Err(invalid_transition(
                        EntityKind::Transfer,
                        &transfer_id,
                        transfer.state.wire_name(),
                        "transfer_paused",
                    ));
                }
                transfer.state = TransferState::Paused;
            }
            EngineEvent::TransferResumed { transfer_id } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                require_transfer_state(transfer, TransferState::Paused, "transfer_resumed")?;
                transfer.state = TransferState::Connecting;
                transfer.failure = None;
            }
            EngineEvent::TransferDelivered { transfer_id } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                require_transfer_state(
                    transfer,
                    TransferState::Transferring,
                    "transfer_delivered",
                )?;
                transfer.state = TransferState::Delivered;
                transfer.transferred_bytes = transfer.total_bytes;
                transfer.failure = None;
            }
            EngineEvent::TransferFailed {
                transfer_id,
                failure,
            } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                if transfer.state.is_terminal() {
                    return Err(invalid_transition(
                        EntityKind::Transfer,
                        &transfer_id,
                        transfer.state.wire_name(),
                        "transfer_failed",
                    ));
                }
                transfer.state = TransferState::Failed;
                transfer.failure = Some(failure);
            }
            EngineEvent::TransferCanceled { transfer_id } => {
                let transfer = transfer_mut(&mut self.transfers, &transfer_id)?;
                if transfer.state.is_terminal() {
                    return Err(invalid_transition(
                        EntityKind::Transfer,
                        &transfer_id,
                        transfer.state.wire_name(),
                        "transfer_canceled",
                    ));
                }
                transfer.state = TransferState::Canceled;
                transfer.failure = None;
            }
        }
        Ok(())
    }
}

fn validate_contract_version(actual: u16) -> Result<(), ApplyError> {
    if actual != APPLICATION_CONTRACT_VERSION {
        return Err(ApplyError::UnsupportedContractVersion {
            expected: APPLICATION_CONTRACT_VERSION,
            actual,
        });
    }
    Ok(())
}

fn missing(entity: EntityKind, id: &impl fmt::Display) -> ApplyError {
    ApplyError::MissingEntity {
        entity,
        id: id.to_string(),
    }
}

fn invalid_transition(
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

fn transfer_mut<'a>(
    transfers: &'a mut BTreeMap<TransferId, Transfer>,
    transfer_id: &TransferId,
) -> Result<&'a mut Transfer, ApplyError> {
    transfers
        .get_mut(transfer_id)
        .ok_or_else(|| missing(EntityKind::Transfer, transfer_id))
}

fn require_transfer_state(
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
