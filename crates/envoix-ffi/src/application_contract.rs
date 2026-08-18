//! Typed UniFFI projection of the v0.3 application contract.
//!
//! This boundary contains only commands, ordered facts, immutable snapshots,
//! and effects. File payloads and relationship credentials never cross it.

use std::sync::{Arc, Mutex, MutexGuard};

use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::command::{CommandEnvelope, EngineCommand, RoomInvitation, VerificationCode};
use envoix_client::decision;
use envoix_client::effect::{EffectEnvelope, EngineEffect};
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::model::{
    CommandId, ContentId, DeviceId, FailureCode, FailurePhase, RecoveryAction, RelationshipId,
    RelationshipState, RoomCloseReason, RoomId, RoomState, TransferDirection, TransferFailure,
    TransferId, TransferRejection, TransferState,
};
use envoix_client::ports::{CapabilityAvailability, PlatformCapabilities, PlatformCapability};
use envoix_client::snapshot::{ApplicationErrorCode, ApplyError, ApplyOutcome, EngineSnapshot};

use crate::{FfiFailureCode, FfiFailurePhase, FfiRecoveryAction, FfiTransferDirection};

pub const ENVOIX_APPLICATION_BINDING_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationBindingInfo {
    pub binding_version: u32,
    pub contract_version: u16,
}

#[uniffi::export]
pub fn envoix_application_binding_info() -> FfiApplicationBindingInfo {
    FfiApplicationBindingInfo {
        binding_version: ENVOIX_APPLICATION_BINDING_VERSION,
        contract_version: APPLICATION_CONTRACT_VERSION,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationErrorCode {
    UnsupportedContractVersion,
    InvalidSequence,
    EventGap,
    EntityNotFound,
    InvalidReference,
    InvalidTransition,
    InvalidProgress,
    GenerationMismatch,
    UnsupportedEvent,
    InvalidInput,
    StateUnavailable,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiApplicationError {
    #[error("{reason}")]
    Failed {
        code: FfiApplicationErrorCode,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplyOutcome {
    Applied,
    IgnoredDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPlatformCapability {
    SecureVault,
    FileSource,
    FileDestination,
    NearbyDiscovery,
    ClipboardRead,
    ClipboardWrite,
    BackgroundExecution,
    Notifications,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiCapabilityAvailability {
    Available,
    Limited,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPlatformCapabilityStatus {
    pub capability: FfiPlatformCapability,
    pub availability: FfiCapabilityAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPlatformCapabilities {
    pub values: Vec<FfiPlatformCapabilityStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRelationshipState {
    Trusted,
    Revoked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationRoomState {
    Connecting,
    Authenticating,
    Connected,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationRoomCloseReason {
    UserEnded,
    Expired,
    PeerEnded,
    Backgrounded,
    NetworkLost,
    ProtocolFailure,
    Replaced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationTransferState {
    Offered,
    Queued,
    Connecting,
    Transferring,
    Paused,
    AwaitingDeliveryProof,
    Delivered,
    Rejected,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferRejection {
    UserDeclined,
    Busy,
    InsufficientSpace,
    UnsupportedContent,
    InvalidOffer,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationTransferFailure {
    pub code: FfiFailureCode,
    pub phase: FfiFailurePhase,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub user_message_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationDevice {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationRelationship {
    pub id: String,
    pub device_id: String,
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub state: FfiRelationshipState,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationRoom {
    pub id: String,
    pub relationship_id: Option<String>,
    pub state: FfiApplicationRoomState,
    pub close_reason: Option<FfiApplicationRoomCloseReason>,
    pub replacement_room_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationTransfer {
    pub id: String,
    pub relationship_id: String,
    pub room_id: Option<String>,
    pub content_id: String,
    pub direction: FfiTransferDirection,
    pub state: FfiApplicationTransferState,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub failure: Option<FfiApplicationTransferFailure>,
    pub rejection: Option<FfiTransferRejection>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationSnapshot {
    pub contract_version: u16,
    pub last_sequence: u64,
    pub capabilities: FfiPlatformCapabilities,
    pub devices: Vec<FfiApplicationDevice>,
    pub relationships: Vec<FfiApplicationRelationship>,
    pub rooms: Vec<FfiApplicationRoom>,
    pub transfers: Vec<FfiApplicationTransfer>,
}

#[derive(Clone, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationCommand {
    CreateRoom,
    JoinRoom {
        invitation: String,
    },
    VerifyPairing {
        room_id: String,
        verification_code: String,
    },
    ReconnectRelationship {
        relationship_id: String,
    },
    CreateTransfer {
        relationship_id: String,
        content_id: String,
        direction: FfiTransferDirection,
    },
    AcceptTransfer {
        transfer_id: String,
    },
    RejectTransfer {
        transfer_id: String,
        reason: FfiTransferRejection,
    },
    PauseTransfer {
        transfer_id: String,
    },
    ResumeTransfer {
        transfer_id: String,
    },
    RecoverTransfer {
        transfer_id: String,
    },
    CancelTransfer {
        transfer_id: String,
    },
    RemoveTransfer {
        transfer_id: String,
    },
    RevokeRelationship {
        relationship_id: String,
    },
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationCommandEnvelope {
    pub contract_version: u16,
    pub command_id: String,
    pub command: FfiApplicationCommand,
}

#[derive(Clone, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationEffect {
    CreateRoom,
    JoinRoom {
        invitation: String,
    },
    VerifyPairing {
        room_id: String,
        verification_code: String,
    },
    ReconnectRelationship {
        relationship_id: String,
        generation: u64,
        previous_generation: Option<u64>,
    },
    CreateTransfer {
        relationship_id: String,
        content_id: String,
        direction: FfiTransferDirection,
    },
    AcceptTransfer {
        transfer_id: String,
    },
    RejectTransfer {
        transfer_id: String,
        reason: FfiTransferRejection,
    },
    PauseTransfer {
        transfer_id: String,
    },
    ResumeTransfer {
        transfer_id: String,
    },
    RecoverTransfer {
        transfer_id: String,
        action: FfiRecoveryAction,
    },
    CancelTransfer {
        transfer_id: String,
    },
    RemoveTransfer {
        transfer_id: String,
    },
    RevokeRelationship {
        relationship_id: String,
    },
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationEffectEnvelope {
    pub contract_version: u16,
    pub command_id: String,
    pub effect: FfiApplicationEffect,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiApplicationEvent {
    CapabilitiesChanged {
        capabilities: FfiPlatformCapabilities,
    },
    DeviceObserved {
        device_id: String,
        display_name: String,
    },
    RelationshipTrusted {
        relationship_id: String,
        device_id: String,
        generation: u64,
    },
    RelationshipRotated {
        relationship_id: String,
        generation: u64,
    },
    RelationshipRevoked {
        relationship_id: String,
    },
    RoomOpened {
        room_id: String,
        relationship_id: Option<String>,
        replaces_room_id: Option<String>,
    },
    RoomPeerAdmitted {
        room_id: String,
    },
    RoomAuthenticated {
        room_id: String,
    },
    RoomClosed {
        room_id: String,
        reason: FfiApplicationRoomCloseReason,
    },
    TransferCreated {
        transfer_id: String,
        relationship_id: String,
        room_id: Option<String>,
        content_id: String,
        direction: FfiTransferDirection,
        total_bytes: u64,
    },
    TransferOffered {
        transfer_id: String,
        relationship_id: String,
        room_id: String,
        content_id: String,
        total_bytes: u64,
    },
    TransferAccepted {
        transfer_id: String,
    },
    TransferRejected {
        transfer_id: String,
        reason: FfiTransferRejection,
    },
    TransferStarted {
        transfer_id: String,
    },
    TransferProgressed {
        transfer_id: String,
        transferred_bytes: u64,
    },
    TransferPaused {
        transfer_id: String,
    },
    TransferResumed {
        transfer_id: String,
    },
    TransferRecoveryStarted {
        transfer_id: String,
    },
    TransferPayloadCompleted {
        transfer_id: String,
    },
    TransferDeliveryProofVerified {
        transfer_id: String,
    },
    TransferFailed {
        transfer_id: String,
        failure: FfiApplicationTransferFailure,
    },
    TransferCanceled {
        transfer_id: String,
    },
    TransferRemoved {
        transfer_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiApplicationEventEnvelope {
    pub contract_version: u16,
    pub sequence: u64,
    pub event: FfiApplicationEvent,
}

#[derive(uniffi::Object)]
pub struct FfiApplicationEngine {
    snapshot: Mutex<EngineSnapshot>,
}

#[uniffi::export]
impl FfiApplicationEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(EngineSnapshot::new()),
        })
    }

    pub fn snapshot(&self) -> Result<FfiApplicationSnapshot, FfiApplicationError> {
        let snapshot = self.lock_snapshot()?;
        Ok(ffi_snapshot(&snapshot))
    }

    pub fn apply(
        &self,
        envelope: FfiApplicationEventEnvelope,
    ) -> Result<FfiApplyOutcome, FfiApplicationError> {
        let envelope = core_event_envelope(envelope)?;
        let outcome = self.lock_snapshot()?.apply(envelope).map_err(apply_error)?;
        Ok(match outcome {
            ApplyOutcome::Applied => FfiApplyOutcome::Applied,
            ApplyOutcome::IgnoredDuplicate => FfiApplyOutcome::IgnoredDuplicate,
        })
    }

    pub fn decide(
        &self,
        envelope: FfiApplicationCommandEnvelope,
    ) -> Result<FfiApplicationEffectEnvelope, FfiApplicationError> {
        let command = core_command_envelope(envelope)?;
        let snapshot = self.lock_snapshot()?;
        decision::decide(&snapshot, command)
            .map(ffi_effect_envelope)
            .map_err(apply_error)
    }
}

impl FfiApplicationEngine {
    fn lock_snapshot(&self) -> Result<MutexGuard<'_, EngineSnapshot>, FfiApplicationError> {
        self.snapshot
            .lock()
            .map_err(|_| FfiApplicationError::Failed {
                code: FfiApplicationErrorCode::StateUnavailable,
                reason: "application state lock is poisoned".into(),
            })
    }
}

fn apply_error(error: ApplyError) -> FfiApplicationError {
    let code = match error.code() {
        ApplicationErrorCode::UnsupportedContractVersion => {
            FfiApplicationErrorCode::UnsupportedContractVersion
        }
        ApplicationErrorCode::InvalidSequence => FfiApplicationErrorCode::InvalidSequence,
        ApplicationErrorCode::EventGap => FfiApplicationErrorCode::EventGap,
        ApplicationErrorCode::EntityNotFound => FfiApplicationErrorCode::EntityNotFound,
        ApplicationErrorCode::InvalidReference => FfiApplicationErrorCode::InvalidReference,
        ApplicationErrorCode::InvalidTransition => FfiApplicationErrorCode::InvalidTransition,
        ApplicationErrorCode::InvalidProgress => FfiApplicationErrorCode::InvalidProgress,
        ApplicationErrorCode::GenerationMismatch => FfiApplicationErrorCode::GenerationMismatch,
        ApplicationErrorCode::UnsupportedEvent => FfiApplicationErrorCode::UnsupportedEvent,
    };
    FfiApplicationError::Failed {
        code,
        reason: error.to_string(),
    }
}

fn invalid_input(error: impl std::fmt::Display) -> FfiApplicationError {
    FfiApplicationError::Failed {
        code: FfiApplicationErrorCode::InvalidInput,
        reason: error.to_string(),
    }
}

fn core_command_envelope(
    envelope: FfiApplicationCommandEnvelope,
) -> Result<CommandEnvelope, FfiApplicationError> {
    Ok(CommandEnvelope {
        contract_version: envelope.contract_version,
        command_id: CommandId::parse(envelope.command_id).map_err(invalid_input)?,
        command: core_command(envelope.command)?,
    })
}

fn core_command(command: FfiApplicationCommand) -> Result<EngineCommand, FfiApplicationError> {
    Ok(match command {
        FfiApplicationCommand::CreateRoom => EngineCommand::CreateRoom,
        FfiApplicationCommand::JoinRoom { invitation } => EngineCommand::JoinRoom {
            invitation: RoomInvitation::parse(invitation).map_err(invalid_input)?,
        },
        FfiApplicationCommand::VerifyPairing {
            room_id,
            verification_code,
        } => EngineCommand::VerifyPairing {
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
            verification_code: VerificationCode::parse(verification_code).map_err(invalid_input)?,
        },
        FfiApplicationCommand::ReconnectRelationship { relationship_id } => {
            EngineCommand::ReconnectRelationship {
                relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            }
        }
        FfiApplicationCommand::CreateTransfer {
            relationship_id,
            content_id,
            direction,
        } => EngineCommand::CreateTransfer {
            relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            content_id: ContentId::parse(content_id).map_err(invalid_input)?,
            direction: core_direction(direction),
        },
        FfiApplicationCommand::AcceptTransfer { transfer_id } => EngineCommand::AcceptTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::RejectTransfer {
            transfer_id,
            reason,
        } => EngineCommand::RejectTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            reason: core_rejection(reason),
        },
        FfiApplicationCommand::PauseTransfer { transfer_id } => EngineCommand::PauseTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::ResumeTransfer { transfer_id } => EngineCommand::ResumeTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::RecoverTransfer { transfer_id } => EngineCommand::RecoverTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::CancelTransfer { transfer_id } => EngineCommand::CancelTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::RemoveTransfer { transfer_id } => EngineCommand::RemoveTransfer {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationCommand::RevokeRelationship { relationship_id } => {
            EngineCommand::RevokeRelationship {
                relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            }
        }
    })
}

fn core_event_envelope(
    envelope: FfiApplicationEventEnvelope,
) -> Result<EventEnvelope, FfiApplicationError> {
    Ok(EventEnvelope {
        contract_version: envelope.contract_version,
        sequence: envelope.sequence,
        event: core_event(envelope.event)?,
    })
}

fn core_event(event: FfiApplicationEvent) -> Result<EngineEvent, FfiApplicationError> {
    Ok(match event {
        FfiApplicationEvent::CapabilitiesChanged { capabilities } => {
            EngineEvent::CapabilitiesChanged {
                capabilities: core_capabilities(capabilities),
            }
        }
        FfiApplicationEvent::DeviceObserved {
            device_id,
            display_name,
        } => EngineEvent::DeviceObserved {
            device_id: DeviceId::parse(device_id).map_err(invalid_input)?,
            display_name,
        },
        FfiApplicationEvent::RelationshipTrusted {
            relationship_id,
            device_id,
            generation,
        } => EngineEvent::RelationshipTrusted {
            relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            device_id: DeviceId::parse(device_id).map_err(invalid_input)?,
            generation,
        },
        FfiApplicationEvent::RelationshipRotated {
            relationship_id,
            generation,
        } => EngineEvent::RelationshipRotated {
            relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            generation,
        },
        FfiApplicationEvent::RelationshipRevoked { relationship_id } => {
            EngineEvent::RelationshipRevoked {
                relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            }
        }
        FfiApplicationEvent::RoomOpened {
            room_id,
            relationship_id,
            replaces_room_id,
        } => EngineEvent::RoomOpened {
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
            relationship_id: relationship_id
                .map(RelationshipId::parse)
                .transpose()
                .map_err(invalid_input)?,
            replaces_room_id: replaces_room_id
                .map(RoomId::parse)
                .transpose()
                .map_err(invalid_input)?,
        },
        FfiApplicationEvent::RoomPeerAdmitted { room_id } => EngineEvent::RoomPeerAdmitted {
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::RoomAuthenticated { room_id } => EngineEvent::RoomAuthenticated {
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::RoomClosed { room_id, reason } => EngineEvent::RoomClosed {
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
            reason: core_room_close_reason(reason),
        },
        FfiApplicationEvent::TransferCreated {
            transfer_id,
            relationship_id,
            room_id,
            content_id,
            direction,
            total_bytes,
        } => EngineEvent::TransferCreated {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            room_id: room_id
                .map(RoomId::parse)
                .transpose()
                .map_err(invalid_input)?,
            content_id: ContentId::parse(content_id).map_err(invalid_input)?,
            direction: core_direction(direction),
            total_bytes,
        },
        FfiApplicationEvent::TransferOffered {
            transfer_id,
            relationship_id,
            room_id,
            content_id,
            total_bytes,
        } => EngineEvent::TransferOffered {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            relationship_id: RelationshipId::parse(relationship_id).map_err(invalid_input)?,
            room_id: RoomId::parse(room_id).map_err(invalid_input)?,
            content_id: ContentId::parse(content_id).map_err(invalid_input)?,
            total_bytes,
        },
        FfiApplicationEvent::TransferAccepted { transfer_id } => EngineEvent::TransferAccepted {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::TransferRejected {
            transfer_id,
            reason,
        } => EngineEvent::TransferRejected {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            reason: core_rejection(reason),
        },
        FfiApplicationEvent::TransferStarted { transfer_id } => EngineEvent::TransferStarted {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::TransferProgressed {
            transfer_id,
            transferred_bytes,
        } => EngineEvent::TransferProgressed {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            transferred_bytes,
        },
        FfiApplicationEvent::TransferPaused { transfer_id } => EngineEvent::TransferPaused {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::TransferResumed { transfer_id } => EngineEvent::TransferResumed {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::TransferRecoveryStarted { transfer_id } => {
            EngineEvent::TransferRecoveryStarted {
                transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            }
        }
        FfiApplicationEvent::TransferPayloadCompleted { transfer_id } => {
            EngineEvent::TransferPayloadCompleted {
                transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            }
        }
        FfiApplicationEvent::TransferDeliveryProofVerified { transfer_id } => {
            EngineEvent::TransferDeliveryProofVerified {
                transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            }
        }
        FfiApplicationEvent::TransferFailed {
            transfer_id,
            failure,
        } => EngineEvent::TransferFailed {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
            failure: core_failure(failure),
        },
        FfiApplicationEvent::TransferCanceled { transfer_id } => EngineEvent::TransferCanceled {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
        FfiApplicationEvent::TransferRemoved { transfer_id } => EngineEvent::TransferRemoved {
            transfer_id: TransferId::parse(transfer_id).map_err(invalid_input)?,
        },
    })
}

fn ffi_effect_envelope(envelope: EffectEnvelope) -> FfiApplicationEffectEnvelope {
    FfiApplicationEffectEnvelope {
        contract_version: envelope.contract_version,
        command_id: envelope.command_id.to_string(),
        effect: match envelope.effect {
            EngineEffect::CreateRoom => FfiApplicationEffect::CreateRoom,
            EngineEffect::JoinRoom { invitation } => FfiApplicationEffect::JoinRoom {
                invitation: invitation.expose().into(),
            },
            EngineEffect::VerifyPairing {
                room_id,
                verification_code,
            } => FfiApplicationEffect::VerifyPairing {
                room_id: room_id.to_string(),
                verification_code: verification_code.expose().into(),
            },
            EngineEffect::ReconnectRelationship {
                relationship_id,
                generation,
                previous_generation,
            } => FfiApplicationEffect::ReconnectRelationship {
                relationship_id: relationship_id.to_string(),
                generation,
                previous_generation,
            },
            EngineEffect::CreateTransfer {
                relationship_id,
                content_id,
                direction,
            } => FfiApplicationEffect::CreateTransfer {
                relationship_id: relationship_id.to_string(),
                content_id: content_id.to_string(),
                direction: ffi_direction(direction),
            },
            EngineEffect::AcceptTransfer { transfer_id } => FfiApplicationEffect::AcceptTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineEffect::RejectTransfer {
                transfer_id,
                reason,
            } => FfiApplicationEffect::RejectTransfer {
                transfer_id: transfer_id.to_string(),
                reason: ffi_rejection(reason),
            },
            EngineEffect::PauseTransfer { transfer_id } => FfiApplicationEffect::PauseTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineEffect::ResumeTransfer { transfer_id } => FfiApplicationEffect::ResumeTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineEffect::RecoverTransfer {
                transfer_id,
                action,
            } => FfiApplicationEffect::RecoverTransfer {
                transfer_id: transfer_id.to_string(),
                action: ffi_recovery_action(action),
            },
            EngineEffect::CancelTransfer { transfer_id } => FfiApplicationEffect::CancelTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineEffect::RemoveTransfer { transfer_id } => FfiApplicationEffect::RemoveTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineEffect::RevokeRelationship { relationship_id } => {
                FfiApplicationEffect::RevokeRelationship {
                    relationship_id: relationship_id.to_string(),
                }
            }
        },
    }
}

fn ffi_snapshot(snapshot: &EngineSnapshot) -> FfiApplicationSnapshot {
    FfiApplicationSnapshot {
        contract_version: snapshot.contract_version,
        last_sequence: snapshot.last_sequence,
        capabilities: ffi_capabilities(&snapshot.capabilities),
        devices: snapshot
            .devices
            .values()
            .map(|device| FfiApplicationDevice {
                id: device.id.to_string(),
                display_name: device.display_name.clone(),
            })
            .collect(),
        relationships: snapshot
            .relationships
            .values()
            .map(|relationship| FfiApplicationRelationship {
                id: relationship.id.to_string(),
                device_id: relationship.device_id.to_string(),
                generation: relationship.generation,
                previous_generation: relationship.previous_generation,
                state: ffi_relationship_state(relationship.state),
            })
            .collect(),
        rooms: snapshot
            .rooms
            .values()
            .map(|room| FfiApplicationRoom {
                id: room.id.to_string(),
                relationship_id: room.relationship_id.as_ref().map(ToString::to_string),
                state: ffi_room_state(room.state),
                close_reason: room.close_reason.map(ffi_room_close_reason),
                replacement_room_id: room.replacement_room_id.as_ref().map(ToString::to_string),
            })
            .collect(),
        transfers: snapshot
            .transfers
            .values()
            .map(|transfer| FfiApplicationTransfer {
                id: transfer.id.to_string(),
                relationship_id: transfer.relationship_id.to_string(),
                room_id: transfer.room_id.as_ref().map(ToString::to_string),
                content_id: transfer.content_id.to_string(),
                direction: ffi_direction(transfer.direction),
                state: ffi_transfer_state(transfer.state),
                transferred_bytes: transfer.transferred_bytes,
                total_bytes: transfer.total_bytes,
                failure: transfer.failure.as_ref().map(ffi_failure),
                rejection: transfer.rejection.map(ffi_rejection),
            })
            .collect(),
    }
}

const CAPABILITIES: [PlatformCapability; 8] = [
    PlatformCapability::SecureVault,
    PlatformCapability::FileSource,
    PlatformCapability::FileDestination,
    PlatformCapability::NearbyDiscovery,
    PlatformCapability::ClipboardRead,
    PlatformCapability::ClipboardWrite,
    PlatformCapability::BackgroundExecution,
    PlatformCapability::Notifications,
];

fn ffi_capabilities(capabilities: &PlatformCapabilities) -> FfiPlatformCapabilities {
    FfiPlatformCapabilities {
        values: CAPABILITIES
            .into_iter()
            .map(|capability| FfiPlatformCapabilityStatus {
                capability: ffi_capability(capability),
                availability: ffi_availability(capabilities.availability(capability)),
            })
            .collect(),
    }
}

fn core_capabilities(capabilities: FfiPlatformCapabilities) -> PlatformCapabilities {
    PlatformCapabilities::new(capabilities.values.into_iter().map(|status| {
        (
            core_capability(status.capability),
            core_availability(status.availability),
        )
    }))
}

fn ffi_capability(capability: PlatformCapability) -> FfiPlatformCapability {
    match capability {
        PlatformCapability::SecureVault => FfiPlatformCapability::SecureVault,
        PlatformCapability::FileSource => FfiPlatformCapability::FileSource,
        PlatformCapability::FileDestination => FfiPlatformCapability::FileDestination,
        PlatformCapability::NearbyDiscovery => FfiPlatformCapability::NearbyDiscovery,
        PlatformCapability::ClipboardRead => FfiPlatformCapability::ClipboardRead,
        PlatformCapability::ClipboardWrite => FfiPlatformCapability::ClipboardWrite,
        PlatformCapability::BackgroundExecution => FfiPlatformCapability::BackgroundExecution,
        PlatformCapability::Notifications => FfiPlatformCapability::Notifications,
    }
}

fn core_capability(capability: FfiPlatformCapability) -> PlatformCapability {
    match capability {
        FfiPlatformCapability::SecureVault => PlatformCapability::SecureVault,
        FfiPlatformCapability::FileSource => PlatformCapability::FileSource,
        FfiPlatformCapability::FileDestination => PlatformCapability::FileDestination,
        FfiPlatformCapability::NearbyDiscovery => PlatformCapability::NearbyDiscovery,
        FfiPlatformCapability::ClipboardRead => PlatformCapability::ClipboardRead,
        FfiPlatformCapability::ClipboardWrite => PlatformCapability::ClipboardWrite,
        FfiPlatformCapability::BackgroundExecution => PlatformCapability::BackgroundExecution,
        FfiPlatformCapability::Notifications => PlatformCapability::Notifications,
    }
}

fn ffi_availability(availability: CapabilityAvailability) -> FfiCapabilityAvailability {
    match availability {
        CapabilityAvailability::Available => FfiCapabilityAvailability::Available,
        CapabilityAvailability::Limited => FfiCapabilityAvailability::Limited,
        CapabilityAvailability::Unavailable => FfiCapabilityAvailability::Unavailable,
    }
}

fn core_availability(availability: FfiCapabilityAvailability) -> CapabilityAvailability {
    match availability {
        FfiCapabilityAvailability::Available => CapabilityAvailability::Available,
        FfiCapabilityAvailability::Limited => CapabilityAvailability::Limited,
        FfiCapabilityAvailability::Unavailable => CapabilityAvailability::Unavailable,
    }
}

fn ffi_relationship_state(state: RelationshipState) -> FfiRelationshipState {
    match state {
        RelationshipState::Trusted => FfiRelationshipState::Trusted,
        RelationshipState::Revoked => FfiRelationshipState::Revoked,
    }
}

fn ffi_room_state(state: RoomState) -> FfiApplicationRoomState {
    match state {
        RoomState::Connecting => FfiApplicationRoomState::Connecting,
        RoomState::Authenticating => FfiApplicationRoomState::Authenticating,
        RoomState::Connected => FfiApplicationRoomState::Connected,
        RoomState::Closed => FfiApplicationRoomState::Closed,
    }
}

fn ffi_room_close_reason(reason: RoomCloseReason) -> FfiApplicationRoomCloseReason {
    match reason {
        RoomCloseReason::UserEnded => FfiApplicationRoomCloseReason::UserEnded,
        RoomCloseReason::Expired => FfiApplicationRoomCloseReason::Expired,
        RoomCloseReason::PeerEnded => FfiApplicationRoomCloseReason::PeerEnded,
        RoomCloseReason::Backgrounded => FfiApplicationRoomCloseReason::Backgrounded,
        RoomCloseReason::NetworkLost => FfiApplicationRoomCloseReason::NetworkLost,
        RoomCloseReason::ProtocolFailure => FfiApplicationRoomCloseReason::ProtocolFailure,
        RoomCloseReason::Replaced => FfiApplicationRoomCloseReason::Replaced,
    }
}

fn core_room_close_reason(reason: FfiApplicationRoomCloseReason) -> RoomCloseReason {
    match reason {
        FfiApplicationRoomCloseReason::UserEnded => RoomCloseReason::UserEnded,
        FfiApplicationRoomCloseReason::Expired => RoomCloseReason::Expired,
        FfiApplicationRoomCloseReason::PeerEnded => RoomCloseReason::PeerEnded,
        FfiApplicationRoomCloseReason::Backgrounded => RoomCloseReason::Backgrounded,
        FfiApplicationRoomCloseReason::NetworkLost => RoomCloseReason::NetworkLost,
        FfiApplicationRoomCloseReason::ProtocolFailure => RoomCloseReason::ProtocolFailure,
        FfiApplicationRoomCloseReason::Replaced => RoomCloseReason::Replaced,
    }
}

fn ffi_direction(direction: TransferDirection) -> FfiTransferDirection {
    match direction {
        TransferDirection::Send => FfiTransferDirection::Send,
        TransferDirection::Receive => FfiTransferDirection::Receive,
    }
}

fn core_direction(direction: FfiTransferDirection) -> TransferDirection {
    match direction {
        FfiTransferDirection::Send => TransferDirection::Send,
        FfiTransferDirection::Receive => TransferDirection::Receive,
    }
}

fn ffi_transfer_state(state: TransferState) -> FfiApplicationTransferState {
    match state {
        TransferState::Offered => FfiApplicationTransferState::Offered,
        TransferState::Queued => FfiApplicationTransferState::Queued,
        TransferState::Connecting => FfiApplicationTransferState::Connecting,
        TransferState::Transferring => FfiApplicationTransferState::Transferring,
        TransferState::Paused => FfiApplicationTransferState::Paused,
        TransferState::AwaitingDeliveryProof => FfiApplicationTransferState::AwaitingDeliveryProof,
        TransferState::Delivered => FfiApplicationTransferState::Delivered,
        TransferState::Rejected => FfiApplicationTransferState::Rejected,
        TransferState::Failed => FfiApplicationTransferState::Failed,
        TransferState::Canceled => FfiApplicationTransferState::Canceled,
    }
}

fn ffi_rejection(rejection: TransferRejection) -> FfiTransferRejection {
    match rejection {
        TransferRejection::UserDeclined => FfiTransferRejection::UserDeclined,
        TransferRejection::Busy => FfiTransferRejection::Busy,
        TransferRejection::InsufficientSpace => FfiTransferRejection::InsufficientSpace,
        TransferRejection::UnsupportedContent => FfiTransferRejection::UnsupportedContent,
        TransferRejection::InvalidOffer => FfiTransferRejection::InvalidOffer,
    }
}

fn core_rejection(rejection: FfiTransferRejection) -> TransferRejection {
    match rejection {
        FfiTransferRejection::UserDeclined => TransferRejection::UserDeclined,
        FfiTransferRejection::Busy => TransferRejection::Busy,
        FfiTransferRejection::InsufficientSpace => TransferRejection::InsufficientSpace,
        FfiTransferRejection::UnsupportedContent => TransferRejection::UnsupportedContent,
        FfiTransferRejection::InvalidOffer => TransferRejection::InvalidOffer,
    }
}

fn ffi_failure(failure: &TransferFailure) -> FfiApplicationTransferFailure {
    FfiApplicationTransferFailure {
        code: ffi_failure_code(failure.code),
        phase: ffi_failure_phase(failure.phase),
        retryable: failure.retryable,
        recovery_action: ffi_recovery_action(failure.recovery_action),
        user_message_key: failure.code.user_message_key().into(),
    }
}

fn core_failure(failure: FfiApplicationTransferFailure) -> TransferFailure {
    TransferFailure {
        code: core_failure_code(failure.code),
        phase: core_failure_phase(failure.phase),
        retryable: failure.retryable,
        recovery_action: core_recovery_action(failure.recovery_action),
    }
}

fn ffi_failure_code(code: FailureCode) -> FfiFailureCode {
    match code {
        FailureCode::UserCanceled => FfiFailureCode::UserCanceled,
        FailureCode::NetworkLost => FfiFailureCode::NetworkLost,
        FailureCode::AuthenticationFailed => FfiFailureCode::AuthenticationFailed,
        FailureCode::RoomNotFound => FfiFailureCode::RoomNotFound,
        FailureCode::RoomExpired => FfiFailureCode::RoomExpired,
        FailureCode::RoomFull => FfiFailureCode::RoomFull,
        FailureCode::RoomRateLimited => FfiFailureCode::RoomRateLimited,
        FailureCode::RoomUnderAttack => FfiFailureCode::RoomUnderAttack,
        FailureCode::EndpointRateLimited => FfiFailureCode::EndpointRateLimited,
        FailureCode::IpRateLimited => FfiFailureCode::IpRateLimited,
        FailureCode::ServerBusy => FfiFailureCode::ServerBusy,
        FailureCode::MalformedJoin => FfiFailureCode::MalformedJoin,
        FailureCode::UnsupportedRendezvousVersion | FailureCode::UnsupportedVersion => {
            FfiFailureCode::UnsupportedRendezvousVersion
        }
        FailureCode::UnsupportedFeature => FfiFailureCode::UnsupportedFeature,
        FailureCode::InternalError | FailureCode::Internal => FfiFailureCode::InternalError,
        FailureCode::SenderSourceUnavailable | FailureCode::SourceUnavailable => {
            FfiFailureCode::SenderSourceUnavailable
        }
        FailureCode::SenderPermissionLost => FfiFailureCode::SenderPermissionLost,
        FailureCode::SenderSourceChanged => FfiFailureCode::SenderSourceChanged,
        FailureCode::SenderItemRemoved => FfiFailureCode::SenderItemRemoved,
        FailureCode::SenderCanceled => FfiFailureCode::SenderCanceled,
        FailureCode::ProtocolOrIntegrityFailure | FailureCode::IntegrityFailure => {
            FfiFailureCode::ProtocolOrIntegrityFailure
        }
        FailureCode::ReceiverSpaceInsufficient => FfiFailureCode::ReceiverSpaceInsufficient,
        FailureCode::ReceiverDestinationDecisionRequired => {
            FfiFailureCode::ReceiverDestinationDecisionRequired
        }
        FailureCode::ReceiverDestinationUnavailable | FailureCode::DestinationUnavailable => {
            FfiFailureCode::ReceiverDestinationUnavailable
        }
        FailureCode::ReceiverSaveFailed => FfiFailureCode::ReceiverSaveFailed,
        FailureCode::ReceiverReusedObjectLost => FfiFailureCode::ReceiverReusedObjectLost,
        FailureCode::ReceiverFinalizationOutcomeUnknown => {
            FfiFailureCode::ReceiverFinalizationOutcomeUnknown
        }
    }
}

fn core_failure_code(code: FfiFailureCode) -> FailureCode {
    match code {
        FfiFailureCode::UserCanceled => FailureCode::UserCanceled,
        FfiFailureCode::NetworkLost => FailureCode::NetworkLost,
        FfiFailureCode::AuthenticationFailed => FailureCode::AuthenticationFailed,
        FfiFailureCode::RoomNotFound => FailureCode::RoomNotFound,
        FfiFailureCode::RoomExpired => FailureCode::RoomExpired,
        FfiFailureCode::RoomFull => FailureCode::RoomFull,
        FfiFailureCode::RoomRateLimited => FailureCode::RoomRateLimited,
        FfiFailureCode::RoomUnderAttack => FailureCode::RoomUnderAttack,
        FfiFailureCode::EndpointRateLimited => FailureCode::EndpointRateLimited,
        FfiFailureCode::IpRateLimited => FailureCode::IpRateLimited,
        FfiFailureCode::ServerBusy => FailureCode::ServerBusy,
        FfiFailureCode::MalformedJoin => FailureCode::MalformedJoin,
        FfiFailureCode::UnsupportedRendezvousVersion => FailureCode::UnsupportedRendezvousVersion,
        FfiFailureCode::UnsupportedFeature => FailureCode::UnsupportedFeature,
        FfiFailureCode::InternalError => FailureCode::InternalError,
        FfiFailureCode::SenderSourceUnavailable => FailureCode::SenderSourceUnavailable,
        FfiFailureCode::SenderPermissionLost => FailureCode::SenderPermissionLost,
        FfiFailureCode::SenderSourceChanged => FailureCode::SenderSourceChanged,
        FfiFailureCode::SenderItemRemoved => FailureCode::SenderItemRemoved,
        FfiFailureCode::SenderCanceled => FailureCode::SenderCanceled,
        FfiFailureCode::ProtocolOrIntegrityFailure => FailureCode::ProtocolOrIntegrityFailure,
        FfiFailureCode::ReceiverSpaceInsufficient => FailureCode::ReceiverSpaceInsufficient,
        FfiFailureCode::ReceiverDestinationDecisionRequired => {
            FailureCode::ReceiverDestinationDecisionRequired
        }
        FfiFailureCode::ReceiverDestinationUnavailable => {
            FailureCode::ReceiverDestinationUnavailable
        }
        FfiFailureCode::ReceiverSaveFailed => FailureCode::ReceiverSaveFailed,
        FfiFailureCode::ReceiverReusedObjectLost => FailureCode::ReceiverReusedObjectLost,
        FfiFailureCode::ReceiverFinalizationOutcomeUnknown => {
            FailureCode::ReceiverFinalizationOutcomeUnknown
        }
    }
}

fn ffi_failure_phase(phase: FailurePhase) -> FfiFailurePhase {
    match phase {
        FailurePhase::Setup => FfiFailurePhase::Setup,
        FailurePhase::Pairing => FfiFailurePhase::Pairing,
        FailurePhase::Connecting => FfiFailurePhase::Connecting,
        FailurePhase::Authenticating => FfiFailurePhase::Authenticating,
        FailurePhase::Negotiating => FfiFailurePhase::Negotiating,
        FailurePhase::Transferring => FfiFailurePhase::Transferring,
        FailurePhase::Verifying => FfiFailurePhase::Verifying,
        FailurePhase::Committing => FfiFailurePhase::Committing,
    }
}

fn core_failure_phase(phase: FfiFailurePhase) -> FailurePhase {
    match phase {
        FfiFailurePhase::Setup => FailurePhase::Setup,
        FfiFailurePhase::Pairing => FailurePhase::Pairing,
        FfiFailurePhase::Connecting => FailurePhase::Connecting,
        FfiFailurePhase::Authenticating => FailurePhase::Authenticating,
        FfiFailurePhase::Negotiating => FailurePhase::Negotiating,
        FfiFailurePhase::Transferring => FailurePhase::Transferring,
        FfiFailurePhase::Verifying => FailurePhase::Verifying,
        FfiFailurePhase::Committing => FailurePhase::Committing,
    }
}

fn ffi_recovery_action(action: RecoveryAction) -> FfiRecoveryAction {
    match action {
        RecoveryAction::Retry => FfiRecoveryAction::Retry,
        RecoveryAction::Resume => FfiRecoveryAction::Resume,
        RecoveryAction::ChooseFolder => FfiRecoveryAction::ChooseFolder,
        RecoveryAction::OpenSettings => FfiRecoveryAction::OpenSettings,
        RecoveryAction::RePair => FfiRecoveryAction::RePair,
        RecoveryAction::None => FfiRecoveryAction::None,
    }
}

fn core_recovery_action(action: FfiRecoveryAction) -> RecoveryAction {
    match action {
        FfiRecoveryAction::Retry => RecoveryAction::Retry,
        FfiRecoveryAction::Resume => RecoveryAction::Resume,
        FfiRecoveryAction::ChooseFolder => RecoveryAction::ChooseFolder,
        FfiRecoveryAction::OpenSettings => RecoveryAction::OpenSettings,
        FfiRecoveryAction::RePair => RecoveryAction::RePair,
        FfiRecoveryAction::None => RecoveryAction::None,
    }
}

#[cfg(test)]
fn ffi_command_envelope(envelope: &CommandEnvelope) -> FfiApplicationCommandEnvelope {
    FfiApplicationCommandEnvelope {
        contract_version: envelope.contract_version,
        command_id: envelope.command_id.to_string(),
        command: match &envelope.command {
            EngineCommand::CreateRoom => FfiApplicationCommand::CreateRoom,
            EngineCommand::JoinRoom { invitation } => FfiApplicationCommand::JoinRoom {
                invitation: invitation.expose().into(),
            },
            EngineCommand::VerifyPairing {
                room_id,
                verification_code,
            } => FfiApplicationCommand::VerifyPairing {
                room_id: room_id.to_string(),
                verification_code: verification_code.expose().into(),
            },
            EngineCommand::ReconnectRelationship { relationship_id } => {
                FfiApplicationCommand::ReconnectRelationship {
                    relationship_id: relationship_id.to_string(),
                }
            }
            EngineCommand::CreateTransfer {
                relationship_id,
                content_id,
                direction,
            } => FfiApplicationCommand::CreateTransfer {
                relationship_id: relationship_id.to_string(),
                content_id: content_id.to_string(),
                direction: ffi_direction(*direction),
            },
            EngineCommand::AcceptTransfer { transfer_id } => {
                FfiApplicationCommand::AcceptTransfer {
                    transfer_id: transfer_id.to_string(),
                }
            }
            EngineCommand::RejectTransfer {
                transfer_id,
                reason,
            } => FfiApplicationCommand::RejectTransfer {
                transfer_id: transfer_id.to_string(),
                reason: ffi_rejection(*reason),
            },
            EngineCommand::PauseTransfer { transfer_id } => FfiApplicationCommand::PauseTransfer {
                transfer_id: transfer_id.to_string(),
            },
            EngineCommand::ResumeTransfer { transfer_id } => {
                FfiApplicationCommand::ResumeTransfer {
                    transfer_id: transfer_id.to_string(),
                }
            }
            EngineCommand::RecoverTransfer { transfer_id } => {
                FfiApplicationCommand::RecoverTransfer {
                    transfer_id: transfer_id.to_string(),
                }
            }
            EngineCommand::CancelTransfer { transfer_id } => {
                FfiApplicationCommand::CancelTransfer {
                    transfer_id: transfer_id.to_string(),
                }
            }
            EngineCommand::RemoveTransfer { transfer_id } => {
                FfiApplicationCommand::RemoveTransfer {
                    transfer_id: transfer_id.to_string(),
                }
            }
            EngineCommand::RevokeRelationship { relationship_id } => {
                FfiApplicationCommand::RevokeRelationship {
                    relationship_id: relationship_id.to_string(),
                }
            }
        },
    }
}

#[cfg(test)]
fn ffi_event_envelope(
    envelope: &EventEnvelope,
) -> Result<FfiApplicationEventEnvelope, FfiApplicationError> {
    let event = match &envelope.event {
        EngineEvent::CapabilitiesChanged { capabilities } => {
            FfiApplicationEvent::CapabilitiesChanged {
                capabilities: ffi_capabilities(capabilities),
            }
        }
        EngineEvent::DeviceObserved {
            device_id,
            display_name,
        } => FfiApplicationEvent::DeviceObserved {
            device_id: device_id.to_string(),
            display_name: display_name.clone(),
        },
        EngineEvent::RelationshipTrusted {
            relationship_id,
            device_id,
            generation,
        } => FfiApplicationEvent::RelationshipTrusted {
            relationship_id: relationship_id.to_string(),
            device_id: device_id.to_string(),
            generation: *generation,
        },
        EngineEvent::RelationshipRotated {
            relationship_id,
            generation,
        } => FfiApplicationEvent::RelationshipRotated {
            relationship_id: relationship_id.to_string(),
            generation: *generation,
        },
        EngineEvent::RelationshipRevoked { relationship_id } => {
            FfiApplicationEvent::RelationshipRevoked {
                relationship_id: relationship_id.to_string(),
            }
        }
        EngineEvent::RoomOpened {
            room_id,
            relationship_id,
            replaces_room_id,
        } => FfiApplicationEvent::RoomOpened {
            room_id: room_id.to_string(),
            relationship_id: relationship_id.as_ref().map(ToString::to_string),
            replaces_room_id: replaces_room_id.as_ref().map(ToString::to_string),
        },
        EngineEvent::RoomPeerAdmitted { room_id } => FfiApplicationEvent::RoomPeerAdmitted {
            room_id: room_id.to_string(),
        },
        EngineEvent::RoomAuthenticated { room_id } => FfiApplicationEvent::RoomAuthenticated {
            room_id: room_id.to_string(),
        },
        EngineEvent::RoomClosed { room_id, reason } => FfiApplicationEvent::RoomClosed {
            room_id: room_id.to_string(),
            reason: ffi_room_close_reason(*reason),
        },
        EngineEvent::TransferCreated {
            transfer_id,
            relationship_id,
            room_id,
            content_id,
            direction,
            total_bytes,
        } => FfiApplicationEvent::TransferCreated {
            transfer_id: transfer_id.to_string(),
            relationship_id: relationship_id.to_string(),
            room_id: room_id.as_ref().map(ToString::to_string),
            content_id: content_id.to_string(),
            direction: ffi_direction(*direction),
            total_bytes: *total_bytes,
        },
        EngineEvent::TransferOffered {
            transfer_id,
            relationship_id,
            room_id,
            content_id,
            total_bytes,
        } => FfiApplicationEvent::TransferOffered {
            transfer_id: transfer_id.to_string(),
            relationship_id: relationship_id.to_string(),
            room_id: room_id.to_string(),
            content_id: content_id.to_string(),
            total_bytes: *total_bytes,
        },
        EngineEvent::TransferAccepted { transfer_id } => FfiApplicationEvent::TransferAccepted {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::TransferRejected {
            transfer_id,
            reason,
        } => FfiApplicationEvent::TransferRejected {
            transfer_id: transfer_id.to_string(),
            reason: ffi_rejection(*reason),
        },
        EngineEvent::TransferStarted { transfer_id } => FfiApplicationEvent::TransferStarted {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::TransferProgressed {
            transfer_id,
            transferred_bytes,
        } => FfiApplicationEvent::TransferProgressed {
            transfer_id: transfer_id.to_string(),
            transferred_bytes: *transferred_bytes,
        },
        EngineEvent::TransferPaused { transfer_id } => FfiApplicationEvent::TransferPaused {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::TransferResumed { transfer_id } => FfiApplicationEvent::TransferResumed {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::TransferRecoveryStarted { transfer_id } => {
            FfiApplicationEvent::TransferRecoveryStarted {
                transfer_id: transfer_id.to_string(),
            }
        }
        EngineEvent::TransferPayloadCompleted { transfer_id } => {
            FfiApplicationEvent::TransferPayloadCompleted {
                transfer_id: transfer_id.to_string(),
            }
        }
        EngineEvent::TransferDeliveryProofVerified { transfer_id } => {
            FfiApplicationEvent::TransferDeliveryProofVerified {
                transfer_id: transfer_id.to_string(),
            }
        }
        EngineEvent::TransferFailed {
            transfer_id,
            failure,
        } => FfiApplicationEvent::TransferFailed {
            transfer_id: transfer_id.to_string(),
            failure: ffi_failure(failure),
        },
        EngineEvent::TransferCanceled { transfer_id } => FfiApplicationEvent::TransferCanceled {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::TransferRemoved { transfer_id } => FfiApplicationEvent::TransferRemoved {
            transfer_id: transfer_id.to_string(),
        },
        EngineEvent::RoomConnected { .. } | EngineEvent::TransferDelivered { .. } => {
            return Err(FfiApplicationError::Failed {
                code: FfiApplicationErrorCode::UnsupportedEvent,
                reason: "historical event is not part of application binding v1".into(),
            });
        }
    };
    Ok(FfiApplicationEventEnvelope {
        contract_version: envelope.contract_version,
        sequence: envelope.sequence,
        event,
    })
}

#[cfg(test)]
mod tests {
    use envoix_client::command::CommandEnvelope;
    use envoix_client::event::EventEnvelope;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        contract_version: u16,
        commands: Vec<CommandEnvelope>,
        events: Vec<EventEnvelope>,
        snapshot: EngineSnapshot,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/application-contract-v6.json"
        ))
        .unwrap()
    }

    #[test]
    fn current_fixture_rebuilds_the_same_typed_snapshot() {
        let fixture = fixture();
        let engine = FfiApplicationEngine::new();
        for event in &fixture.events {
            assert_eq!(
                engine.apply(ffi_event_envelope(event).unwrap()).unwrap(),
                FfiApplyOutcome::Applied
            );
        }

        assert_eq!(fixture.contract_version, APPLICATION_CONTRACT_VERSION);
        assert_eq!(engine.snapshot().unwrap(), ffi_snapshot(&fixture.snapshot));
    }

    #[test]
    fn every_current_command_round_trips_without_json() {
        for command in fixture().commands {
            let projected = ffi_command_envelope(&command);
            let round_trip = core_command_envelope(projected).unwrap();
            assert!(round_trip == command);
        }
    }

    #[test]
    fn duplicate_events_are_idempotent_and_gaps_are_typed() {
        let event = ffi_event_envelope(&fixture().events[0]).unwrap();
        let engine = FfiApplicationEngine::new();
        assert_eq!(
            engine.apply(event.clone()).unwrap(),
            FfiApplyOutcome::Applied
        );
        assert_eq!(
            engine.apply(event).unwrap(),
            FfiApplyOutcome::IgnoredDuplicate
        );

        let gap = ffi_event_envelope(&fixture().events[2]).unwrap();
        assert!(matches!(
            engine.apply(gap),
            Err(FfiApplicationError::Failed {
                code: FfiApplicationErrorCode::EventGap,
                ..
            })
        ));
    }

    #[test]
    fn binding_negotiation_is_explicit_and_secret_free() {
        let binding = envoix_application_binding_info();
        assert_eq!(binding.binding_version, ENVOIX_APPLICATION_BINDING_VERSION);
        assert_eq!(binding.contract_version, APPLICATION_CONTRACT_VERSION);
        assert!(
            crate::envoix_core_info()
                .capabilities
                .contains(&"typed_application_contract_v6".to_string())
        );

        let snapshot = FfiApplicationEngine::new().snapshot().unwrap();
        let diagnostic = format!("{snapshot:?}").to_ascii_lowercase();
        assert!(!diagnostic.contains("credential"));
        assert!(!diagnostic.contains("invitation"));
        assert!(!diagnostic.contains("verification"));
    }
}
