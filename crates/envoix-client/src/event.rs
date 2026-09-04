//! Ordered, secret-free application events.

use serde::{Deserialize, Serialize};

use crate::model::{
    ContentId, DeviceId, RelationshipId, RoomCloseReason, RoomId, TransferDirection,
    TransferFailure, TransferId,
};
use crate::ports::PlatformCapabilities;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub contract_version: u16,
    pub sequence: u64,
    pub event: EngineEvent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineEvent {
    CapabilitiesChanged {
        capabilities: PlatformCapabilities,
    },
    DeviceObserved {
        device_id: DeviceId,
        display_name: String,
    },
    RelationshipTrusted {
        relationship_id: RelationshipId,
        device_id: DeviceId,
        generation: u64,
    },
    /// Persists the target generation after remembered authentication.
    /// Repeating the current generation is an idempotent crash recovery.
    RelationshipRotated {
        relationship_id: RelationshipId,
        generation: u64,
    },
    RelationshipRevoked {
        relationship_id: RelationshipId,
    },
    RoomOpened {
        room_id: RoomId,
        relationship_id: Option<RelationshipId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaces_room_id: Option<RoomId>,
    },
    RoomPeerAdmitted {
        room_id: RoomId,
    },
    RoomAuthenticated {
        room_id: RoomId,
    },
    /// Historical v1/v2 event retained only for fixture decoding.
    RoomConnected {
        room_id: RoomId,
    },
    RoomClosed {
        room_id: RoomId,
        reason: RoomCloseReason,
    },
    TransferCreated {
        transfer_id: TransferId,
        relationship_id: RelationshipId,
        room_id: Option<RoomId>,
        content_id: ContentId,
        direction: TransferDirection,
        total_bytes: u64,
    },
    TransferOffered {
        transfer_id: TransferId,
        relationship_id: RelationshipId,
        room_id: RoomId,
        content_id: ContentId,
        total_bytes: u64,
    },
    TransferAccepted {
        transfer_id: TransferId,
    },
    TransferRejected {
        transfer_id: TransferId,
        reason: crate::model::TransferRejection,
    },
    TransferStarted {
        transfer_id: TransferId,
    },
    TransferProgressed {
        transfer_id: TransferId,
        transferred_bytes: u64,
    },
    TransferPaused {
        transfer_id: TransferId,
    },
    TransferResumed {
        transfer_id: TransferId,
    },
    TransferRecoveryStarted {
        transfer_id: TransferId,
    },
    TransferPayloadCompleted {
        transfer_id: TransferId,
    },
    TransferDeliveryProofVerified {
        transfer_id: TransferId,
    },
    /// Historical v1-v4 event retained only for fixture decoding.
    TransferDelivered {
        transfer_id: TransferId,
    },
    TransferFailed {
        transfer_id: TransferId,
        failure: TransferFailure,
    },
    TransferCanceled {
        transfer_id: TransferId,
    },
    TransferRemoved {
        transfer_id: TransferId,
    },
}
