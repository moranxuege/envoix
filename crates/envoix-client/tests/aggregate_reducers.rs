use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::model::{
    ContentId, DeviceId, EntityKind, RelationshipId, RelationshipState, RoomCloseReason, RoomId,
    RoomState, TransferDirection, TransferId, TransferRejection, TransferState,
};
use envoix_client::snapshot::{ApplicationErrorCode, ApplyError, EngineSnapshot};

struct FixtureIds {
    device: DeviceId,
    relationship: RelationshipId,
    room: RoomId,
    transfer: TransferId,
    content: ContentId,
}

fn fixture_ids(suffix: &str) -> FixtureIds {
    FixtureIds {
        device: DeviceId::parse(format!("dev_{suffix}")).unwrap(),
        relationship: RelationshipId::parse(format!("rel_{suffix}")).unwrap(),
        room: RoomId::parse(format!("room_{suffix}")).unwrap(),
        transfer: TransferId::parse(format!("transfer_{suffix}")).unwrap(),
        content: ContentId::parse(format!("content_{suffix}")).unwrap(),
    }
}

fn apply_next(snapshot: &mut EngineSnapshot, event: EngineEvent) -> Result<(), ApplyError> {
    let sequence = snapshot.last_sequence.checked_add(1).unwrap();
    snapshot
        .apply(EventEnvelope {
            contract_version: APPLICATION_CONTRACT_VERSION,
            sequence,
            event,
        })
        .map(|_| ())
}

fn trust_relationship(snapshot: &mut EngineSnapshot, ids: &FixtureIds) {
    apply_next(
        snapshot,
        EngineEvent::DeviceObserved {
            device_id: ids.device.clone(),
            display_name: "Fixture device".into(),
        },
    )
    .unwrap();
    apply_next(
        snapshot,
        EngineEvent::RelationshipTrusted {
            relationship_id: ids.relationship.clone(),
            device_id: ids.device.clone(),
            generation: 4,
        },
    )
    .unwrap();
}

fn connect_room(snapshot: &mut EngineSnapshot, ids: &FixtureIds) {
    apply_next(
        snapshot,
        EngineEvent::RoomOpened {
            room_id: ids.room.clone(),
            relationship_id: Some(ids.relationship.clone()),
            replaces_room_id: None,
        },
    )
    .unwrap();
    apply_next(
        snapshot,
        EngineEvent::RoomPeerAdmitted {
            room_id: ids.room.clone(),
        },
    )
    .unwrap();
    apply_next(
        snapshot,
        EngineEvent::RoomAuthenticated {
            room_id: ids.room.clone(),
        },
    )
    .unwrap();
}

#[test]
fn relationship_reducer_requires_a_device_and_revocation_is_terminal() {
    let ids = fixture_ids("relationship");
    let mut snapshot = EngineSnapshot::new();

    let error = apply_next(
        &mut snapshot,
        EngineEvent::RelationshipTrusted {
            relationship_id: ids.relationship.clone(),
            device_id: ids.device.clone(),
            generation: 4,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ApplicationErrorCode::EntityNotFound);
    assert!(matches!(
        error,
        ApplyError::MissingEntity {
            entity: EntityKind::Device,
            ..
        }
    ));
    assert_eq!(snapshot.last_sequence, 0);
    assert!(snapshot.relationships.is_empty());

    trust_relationship(&mut snapshot, &ids);
    assert_eq!(
        snapshot.relationships[&ids.relationship].previous_generation,
        None
    );
    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRotated {
            relationship_id: ids.relationship.clone(),
            generation: 5,
        },
    )
    .unwrap();
    assert_eq!(snapshot.relationships[&ids.relationship].generation, 5);
    assert_eq!(
        snapshot.relationships[&ids.relationship].previous_generation,
        Some(4)
    );

    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRotated {
            relationship_id: ids.relationship.clone(),
            generation: 5,
        },
    )
    .unwrap();
    let before_stale_generation = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRotated {
            relationship_id: ids.relationship.clone(),
            generation: 4,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ApplicationErrorCode::GenerationMismatch);
    assert!(matches!(
        error,
        ApplyError::GenerationMismatch {
            current_generation: 5,
            attempted_generation: 4,
            ..
        }
    ));
    assert_eq!(snapshot, before_stale_generation);

    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRevoked {
            relationship_id: ids.relationship.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.relationships[&ids.relationship].state,
        RelationshipState::Revoked
    );

    let before_revoked_rotation = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRotated {
            relationship_id: ids.relationship.clone(),
            generation: 6,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ApplyError::InvalidTransition {
            entity: EntityKind::Relationship,
            ..
        }
    ));
    assert_eq!(snapshot, before_revoked_rotation);

    let before = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRevoked {
            relationship_id: ids.relationship,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ApplyError::InvalidTransition {
            entity: EntityKind::Relationship,
            ..
        }
    ));
    assert_eq!(snapshot, before);
}

#[test]
fn room_reducer_requires_trust_and_close_is_terminal() {
    let ids = fixture_ids("room");
    let missing = RelationshipId::parse("rel_missing").unwrap();
    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);

    let before_missing = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RoomOpened {
            room_id: ids.room.clone(),
            relationship_id: Some(missing),
            replaces_room_id: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ApplyError::MissingEntity {
            entity: EntityKind::Relationship,
            ..
        }
    ));
    assert_eq!(snapshot, before_missing);

    connect_room(&mut snapshot, &ids);
    apply_next(
        &mut snapshot,
        EngineEvent::RoomClosed {
            room_id: ids.room.clone(),
            reason: RoomCloseReason::Expired,
        },
    )
    .unwrap();
    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Closed);
    assert_eq!(
        snapshot.relationships[&ids.relationship].state,
        RelationshipState::Trusted
    );

    let before_terminal = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RoomAuthenticated { room_id: ids.room },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ApplyError::InvalidTransition {
            entity: EntityKind::Room,
            ..
        }
    ));
    assert_eq!(snapshot, before_terminal);
}

#[test]
fn room_admission_authentication_and_replacement_are_explicit_and_atomic() {
    let ids = fixture_ids("room_lifecycle");
    let replacement = RoomId::parse("room_lifecycle_replacement").unwrap();
    let missing = RoomId::parse("room_lifecycle_missing").unwrap();
    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);

    apply_next(
        &mut snapshot,
        EngineEvent::RoomOpened {
            room_id: ids.room.clone(),
            relationship_id: Some(ids.relationship.clone()),
            replaces_room_id: None,
        },
    )
    .unwrap();

    let before_authentication = snapshot.clone();
    let error = apply_next(
        &mut snapshot,
        EngineEvent::RoomConnected {
            room_id: ids.room.clone(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ApplicationErrorCode::UnsupportedEvent);
    assert_eq!(snapshot, before_authentication);

    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::RoomAuthenticated {
                room_id: ids.room.clone(),
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Room,
            ..
        })
    ));
    assert_eq!(snapshot, before_authentication);

    apply_next(
        &mut snapshot,
        EngineEvent::RoomPeerAdmitted {
            room_id: ids.room.clone(),
        },
    )
    .unwrap();
    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Authenticating);
    apply_next(
        &mut snapshot,
        EngineEvent::RoomAuthenticated {
            room_id: ids.room.clone(),
        },
    )
    .unwrap();
    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Connected);

    let before_implicit_replacement = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::RoomOpened {
                room_id: replacement.clone(),
                relationship_id: Some(ids.relationship.clone()),
                replaces_room_id: None,
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Room,
            ..
        })
    ));
    assert_eq!(snapshot, before_implicit_replacement);

    apply_next(
        &mut snapshot,
        EngineEvent::RoomOpened {
            room_id: replacement.clone(),
            relationship_id: Some(ids.relationship.clone()),
            replaces_room_id: Some(ids.room.clone()),
        },
    )
    .unwrap();
    assert_eq!(snapshot.rooms[&replacement].state, RoomState::Connecting);
    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Closed);
    assert_eq!(
        snapshot.rooms[&ids.room].close_reason,
        Some(RoomCloseReason::Replaced)
    );
    assert_eq!(
        snapshot.rooms[&ids.room].replacement_room_id,
        Some(replacement.clone())
    );
    assert_eq!(
        snapshot.relationships[&ids.relationship].state,
        RelationshipState::Trusted
    );

    let before_missing_replacement = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::RoomOpened {
                room_id: RoomId::parse("room_lifecycle_invalid").unwrap(),
                relationship_id: Some(ids.relationship),
                replaces_room_id: Some(missing),
            },
        ),
        Err(ApplyError::MissingEntity {
            entity: EntityKind::Room,
            ..
        })
    ));
    assert_eq!(snapshot, before_missing_replacement);
}

#[test]
fn transfer_reducer_is_atomic_and_outlives_its_room() {
    let ids = fixture_ids("transfer");
    let other = fixture_ids("other");
    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);
    connect_room(&mut snapshot, &ids);
    trust_relationship(&mut snapshot, &other);

    let error = apply_next(
        &mut snapshot,
        EngineEvent::TransferCreated {
            transfer_id: ids.transfer.clone(),
            relationship_id: other.relationship,
            room_id: Some(ids.room.clone()),
            content_id: ids.content.clone(),
            direction: TransferDirection::Send,
            total_bytes: 5,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ApplyError::InvalidReference {
            entity: EntityKind::Room,
            field: "relationship_id",
            ..
        }
    ));
    assert!(!snapshot.transfers.contains_key(&ids.transfer));

    apply_next(
        &mut snapshot,
        EngineEvent::TransferCreated {
            transfer_id: ids.transfer.clone(),
            relationship_id: ids.relationship.clone(),
            room_id: Some(ids.room.clone()),
            content_id: ids.content,
            direction: TransferDirection::Send,
            total_bytes: 5,
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Queued
    );

    let before_invalid_progress = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 1,
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        })
    ));
    assert_eq!(snapshot, before_invalid_progress);

    for event in [
        EngineEvent::TransferStarted {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::TransferProgressed {
            transfer_id: ids.transfer.clone(),
            transferred_bytes: 2,
        },
        EngineEvent::TransferPaused {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::TransferResumed {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::RoomClosed {
            room_id: ids.room.clone(),
            reason: RoomCloseReason::Expired,
        },
        EngineEvent::TransferProgressed {
            transfer_id: ids.transfer.clone(),
            transferred_bytes: 5,
        },
        EngineEvent::TransferDelivered {
            transfer_id: ids.transfer.clone(),
        },
    ] {
        apply_next(&mut snapshot, event).unwrap();
    }

    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Closed);
    assert_eq!(
        snapshot.relationships[&ids.relationship].state,
        RelationshipState::Trusted
    );
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Delivered
    );

    let before_terminal = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::TransferCanceled {
                transfer_id: ids.transfer,
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        })
    ));
    assert_eq!(snapshot, before_terminal);
}

#[test]
fn incoming_transfer_requires_an_explicit_accept_or_typed_rejection() {
    let ids = fixture_ids("incoming_offer");
    let rejected_id = TransferId::parse("transfer_incoming_rejected").unwrap();
    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);
    connect_room(&mut snapshot, &ids);

    apply_next(
        &mut snapshot,
        EngineEvent::TransferOffered {
            transfer_id: ids.transfer.clone(),
            relationship_id: ids.relationship.clone(),
            room_id: ids.room.clone(),
            content_id: ids.content.clone(),
            total_bytes: 5,
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Offered
    );

    let before_start = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::TransferStarted {
                transfer_id: ids.transfer.clone(),
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        })
    ));
    assert_eq!(snapshot, before_start);

    apply_next(
        &mut snapshot,
        EngineEvent::TransferAccepted {
            transfer_id: ids.transfer.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Queued
    );

    apply_next(
        &mut snapshot,
        EngineEvent::TransferOffered {
            transfer_id: rejected_id.clone(),
            relationship_id: ids.relationship,
            room_id: ids.room,
            content_id: ContentId::parse("content_incoming_rejected").unwrap(),
            total_bytes: 9,
        },
    )
    .unwrap();
    apply_next(
        &mut snapshot,
        EngineEvent::TransferRejected {
            transfer_id: rejected_id.clone(),
            reason: TransferRejection::UserDeclined,
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.transfers[&rejected_id].state,
        TransferState::Rejected
    );
    assert_eq!(
        snapshot.transfers[&rejected_id].rejection,
        Some(TransferRejection::UserDeclined)
    );

    let before_terminal = snapshot.clone();
    assert!(matches!(
        apply_next(
            &mut snapshot,
            EngineEvent::TransferAccepted {
                transfer_id: rejected_id,
            },
        ),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        })
    ));
    assert_eq!(snapshot, before_terminal);
}
