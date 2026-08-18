//! Immutable application snapshots and their ordered event reducer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::APPLICATION_CONTRACT_VERSION;
use crate::event::{EngineEvent, EventEnvelope};
use crate::model::{
    Device, DeviceId, EntityKind, Relationship, RelationshipId, RelationshipState, Room, RoomId,
    RoomState, Transfer, TransferId, TransferState,
};
use crate::ports::PlatformCapabilities;
use crate::reducers;

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
    GenerationMismatch,
    UnsupportedEvent,
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
    #[error(
        "relationship {relationship_id} cannot move generation backwards from {current_generation} to {attempted_generation}"
    )]
    GenerationMismatch {
        relationship_id: RelationshipId,
        current_generation: u64,
        attempted_generation: u64,
    },
    #[error("application event {event} is not supported by contract {contract_version}")]
    UnsupportedEvent {
        event: &'static str,
        contract_version: u16,
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
            Self::GenerationMismatch { .. } => ApplicationErrorCode::GenerationMismatch,
            Self::UnsupportedEvent { .. } => ApplicationErrorCode::UnsupportedEvent,
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
                let relationship = reducers::relationship::trust(
                    &self.devices,
                    self.relationships.get(&relationship_id),
                    Relationship {
                        id: relationship_id.clone(),
                        device_id,
                        generation,
                        previous_generation: None,
                        state: RelationshipState::Trusted,
                    },
                )?;
                self.relationships.insert(relationship_id, relationship);
            }
            EngineEvent::RelationshipRotated {
                relationship_id,
                generation,
            } => {
                let relationship = reducers::relationship::rotate(
                    self.relationships.get(&relationship_id),
                    &relationship_id,
                    generation,
                )?;
                self.relationships.insert(relationship_id, relationship);
            }
            EngineEvent::RelationshipRevoked { relationship_id } => {
                let relationship = reducers::relationship::revoke(
                    self.relationships.get(&relationship_id),
                    &relationship_id,
                )?;
                self.relationships.insert(relationship_id, relationship);
            }
            EngineEvent::RoomOpened {
                room_id,
                relationship_id,
                replaces_room_id,
            } => {
                let reduction = reducers::room::open(
                    &self.relationships,
                    &self.rooms,
                    Room {
                        id: room_id.clone(),
                        relationship_id,
                        state: RoomState::Connecting,
                        close_reason: None,
                        replacement_room_id: None,
                    },
                    replaces_room_id.as_ref(),
                )?;
                if let Some(replaced) = reduction.replaced {
                    self.rooms.insert(replaced.id.clone(), replaced);
                }
                self.rooms.insert(room_id, reduction.room);
            }
            EngineEvent::RoomPeerAdmitted { room_id } => {
                let room = reducers::room::admit(self.rooms.get(&room_id), &room_id)?;
                self.rooms.insert(room_id, room);
            }
            EngineEvent::RoomAuthenticated { room_id } => {
                let room = reducers::room::authenticate(self.rooms.get(&room_id), &room_id)?;
                self.rooms.insert(room_id, room);
            }
            EngineEvent::RoomConnected { .. } => {
                return Err(ApplyError::UnsupportedEvent {
                    event: "room_connected",
                    contract_version: APPLICATION_CONTRACT_VERSION,
                });
            }
            EngineEvent::RoomClosed { room_id, reason } => {
                let room = reducers::room::close(self.rooms.get(&room_id), &room_id, reason)?;
                self.rooms.insert(room_id, room);
            }
            EngineEvent::TransferCreated {
                transfer_id,
                relationship_id,
                room_id,
                content_id,
                direction,
                total_bytes,
            } => {
                let transfer = reducers::transfer::create(
                    &self.relationships,
                    &self.rooms,
                    self.transfers.get(&transfer_id),
                    Transfer {
                        id: transfer_id.clone(),
                        relationship_id,
                        room_id,
                        content_id,
                        direction,
                        state: TransferState::Queued,
                        transferred_bytes: 0,
                        total_bytes,
                        failure: None,
                        rejection: None,
                    },
                )?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferOffered {
                transfer_id,
                relationship_id,
                room_id,
                content_id,
                total_bytes,
            } => {
                let transfer = reducers::transfer::offer(
                    &self.relationships,
                    &self.rooms,
                    self.transfers.get(&transfer_id),
                    Transfer {
                        id: transfer_id.clone(),
                        relationship_id,
                        room_id: Some(room_id),
                        content_id,
                        direction: crate::model::TransferDirection::Receive,
                        state: TransferState::Offered,
                        transferred_bytes: 0,
                        total_bytes,
                        failure: None,
                        rejection: None,
                    },
                )?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferAccepted { transfer_id } => {
                let transfer =
                    reducers::transfer::accept(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferRejected {
                transfer_id,
                reason,
            } => {
                let transfer = reducers::transfer::reject(
                    self.transfers.get(&transfer_id),
                    &transfer_id,
                    reason,
                )?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferStarted { transfer_id } => {
                let transfer =
                    reducers::transfer::start(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferProgressed {
                transfer_id,
                transferred_bytes,
            } => {
                let transfer = reducers::transfer::progress(
                    self.transfers.get(&transfer_id),
                    &transfer_id,
                    transferred_bytes,
                )?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferPaused { transfer_id } => {
                let transfer =
                    reducers::transfer::pause(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferResumed { transfer_id } => {
                let transfer =
                    reducers::transfer::resume(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferDelivered { transfer_id } => {
                let transfer =
                    reducers::transfer::deliver(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferFailed {
                transfer_id,
                failure,
            } => {
                let transfer = reducers::transfer::fail(
                    self.transfers.get(&transfer_id),
                    &transfer_id,
                    failure,
                )?;
                self.transfers.insert(transfer_id, transfer);
            }
            EngineEvent::TransferCanceled { transfer_id } => {
                let transfer =
                    reducers::transfer::cancel(self.transfers.get(&transfer_id), &transfer_id)?;
                self.transfers.insert(transfer_id, transfer);
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
