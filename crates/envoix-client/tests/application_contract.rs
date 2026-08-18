use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::command::{
    CommandEnvelope, EngineCommand, MAX_ROOM_INVITATION_BYTES, RoomInvitation, VerificationCode,
};
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::model::{
    CommandId, ContentId, DeviceId, EntityKind, FailureCode, FailurePhase, RecoveryAction,
    RelationshipId, RelationshipState, RoomCloseReason, RoomId, RoomState, TransferDirection,
    TransferFailure, TransferId, TransferState,
};
use envoix_client::ports::{CapabilityAvailability, PlatformCapabilities, PlatformCapability};
use envoix_client::runtime::replay;
use envoix_client::snapshot::{ApplicationErrorCode, ApplyError, ApplyOutcome, EngineSnapshot};

#[derive(Clone)]
struct FixtureIds {
    device: DeviceId,
    relationship: RelationshipId,
    room: RoomId,
    transfer: TransferId,
    content: ContentId,
}

fn fixture_ids() -> FixtureIds {
    FixtureIds {
        device: DeviceId::parse("dev_fixture_0001").unwrap(),
        relationship: RelationshipId::parse("rel_fixture_0001").unwrap(),
        room: RoomId::parse("room_fixture_0001").unwrap(),
        transfer: TransferId::parse("transfer_fixture_0001").unwrap(),
        content: ContentId::parse("content_fixture_0001").unwrap(),
    }
}

fn envelope(sequence: u64, event: EngineEvent) -> EventEnvelope {
    EventEnvelope {
        contract_version: APPLICATION_CONTRACT_VERSION,
        sequence,
        event,
    }
}

fn remembered_transfer_events(ids: &FixtureIds) -> Vec<EventEnvelope> {
    vec![
        envelope(
            1,
            EngineEvent::CapabilitiesChanged {
                capabilities: PlatformCapabilities::new([
                    (
                        PlatformCapability::SecureVault,
                        CapabilityAvailability::Available,
                    ),
                    (
                        PlatformCapability::FileDestination,
                        CapabilityAvailability::Available,
                    ),
                    (
                        PlatformCapability::NearbyDiscovery,
                        CapabilityAvailability::Limited,
                    ),
                ]),
            },
        ),
        envelope(
            2,
            EngineEvent::DeviceObserved {
                device_id: ids.device.clone(),
                display_name: "Fixture WSL".into(),
            },
        ),
        envelope(
            3,
            EngineEvent::RelationshipTrusted {
                relationship_id: ids.relationship.clone(),
                device_id: ids.device.clone(),
                generation: 4,
            },
        ),
        envelope(
            4,
            EngineEvent::RoomOpened {
                room_id: ids.room.clone(),
                relationship_id: Some(ids.relationship.clone()),
                replaces_room_id: None,
            },
        ),
        envelope(
            5,
            EngineEvent::RoomPeerAdmitted {
                room_id: ids.room.clone(),
            },
        ),
        envelope(
            6,
            EngineEvent::RoomAuthenticated {
                room_id: ids.room.clone(),
            },
        ),
        envelope(
            7,
            EngineEvent::TransferCreated {
                transfer_id: ids.transfer.clone(),
                relationship_id: ids.relationship.clone(),
                room_id: Some(ids.room.clone()),
                content_id: ids.content.clone(),
                direction: TransferDirection::Send,
                total_bytes: 42,
            },
        ),
        envelope(
            8,
            EngineEvent::TransferStarted {
                transfer_id: ids.transfer.clone(),
            },
        ),
        envelope(
            9,
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 21,
            },
        ),
        envelope(
            10,
            EngineEvent::RoomClosed {
                room_id: ids.room.clone(),
                reason: RoomCloseReason::Expired,
            },
        ),
        envelope(
            11,
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 42,
            },
        ),
        envelope(
            12,
            EngineEvent::TransferDelivered {
                transfer_id: ids.transfer.clone(),
            },
        ),
        envelope(
            13,
            EngineEvent::RelationshipRevoked {
                relationship_id: ids.relationship.clone(),
            },
        ),
    ]
}

#[test]
fn ordered_events_rebuild_an_identical_snapshot() {
    let ids = fixture_ids();
    let events = remembered_transfer_events(&ids);
    let snapshot = replay(EngineSnapshot::new(), events.clone()).unwrap();

    assert_eq!(snapshot.last_sequence, 13);
    assert_eq!(
        snapshot.relationships[&ids.relationship].state,
        RelationshipState::Revoked
    );
    assert_eq!(snapshot.rooms[&ids.room].state, RoomState::Closed);
    assert_eq!(
        snapshot.rooms[&ids.room].close_reason,
        Some(RoomCloseReason::Expired)
    );
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Delivered
    );
    assert_eq!(snapshot.transfers[&ids.transfer].transferred_bytes, 42);

    let wire = serde_json::to_vec(&events).unwrap();
    let decoded: Vec<EventEnvelope> = serde_json::from_slice(&wire).unwrap();
    let replayed = replay(EngineSnapshot::new(), decoded).unwrap();
    assert_eq!(replayed, snapshot);
    let snapshot_wire = serde_json::to_vec(&snapshot).unwrap();
    assert_eq!(
        serde_json::from_slice::<EngineSnapshot>(&snapshot_wire).unwrap(),
        snapshot
    );
}

#[test]
fn replay_detects_duplicates_gaps_and_illegal_transitions() {
    let ids = fixture_ids();
    let events = remembered_transfer_events(&ids);
    let mut snapshot = replay(EngineSnapshot::new(), events[..9].iter().cloned()).unwrap();
    let before_duplicate = snapshot.clone();

    assert_eq!(
        snapshot.apply(events[8].clone()).unwrap(),
        ApplyOutcome::IgnoredDuplicate
    );
    assert_eq!(snapshot, before_duplicate);
    assert!(matches!(
        snapshot.apply(envelope(
            11,
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 30,
            },
        )),
        Err(ApplyError::SequenceGap {
            expected: 10,
            actual: 11,
        })
    ));
    assert!(matches!(
        snapshot.apply(envelope(
            10,
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 43,
            },
        )),
        Err(ApplyError::InvalidProgress { .. })
    ));
    assert_eq!(snapshot.last_sequence, 9);
    assert_eq!(snapshot.transfers[&ids.transfer].transferred_bytes, 21);

    snapshot
        .apply(envelope(
            10,
            EngineEvent::TransferPaused {
                transfer_id: ids.transfer.clone(),
            },
        ))
        .unwrap();
    assert!(matches!(
        snapshot.apply(envelope(
            11,
            EngineEvent::TransferDelivered {
                transfer_id: ids.transfer,
            },
        )),
        Err(ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        })
    ));
    assert_eq!(snapshot.last_sequence, 10);

    let mut exhausted = EngineSnapshot::new();
    exhausted.last_sequence = u64::MAX;
    let before_exhausted_duplicate = exhausted.clone();
    assert_eq!(
        exhausted
            .apply(envelope(
                u64::MAX,
                EngineEvent::CapabilitiesChanged {
                    capabilities: PlatformCapabilities::new([(
                        PlatformCapability::Notifications,
                        CapabilityAvailability::Available,
                    )]),
                },
            ))
            .unwrap(),
        ApplyOutcome::IgnoredDuplicate
    );
    assert_eq!(exhausted, before_exhausted_duplicate);
}

#[test]
fn commands_and_capabilities_form_a_versioned_typed_boundary() {
    let ids = fixture_ids();
    for invalid in ["", "has space", "path/segment", "line\nbreak"] {
        assert!(DeviceId::parse(invalid).is_err(), "{invalid:?}");
    }
    assert!(DeviceId::parse("x".repeat(129)).is_err());
    assert!(RoomInvitation::parse("").is_err());
    assert!(RoomInvitation::parse(" \n\t").is_err());
    assert!(RoomInvitation::parse("x".repeat(MAX_ROOM_INVITATION_BYTES)).is_ok());
    assert!(RoomInvitation::parse("x".repeat(MAX_ROOM_INVITATION_BYTES + 1)).is_err());
    for invalid in ["", "12345", "1234567", "12345a", "１２３４５６"] {
        assert!(VerificationCode::parse(invalid).is_err(), "{invalid:?}");
    }

    let commands = vec![
        EngineCommand::CreateRoom,
        EngineCommand::JoinRoom {
            invitation: RoomInvitation::parse("envoix://room/000000-0000-0000?expires=1").unwrap(),
        },
        EngineCommand::VerifyPairing {
            room_id: ids.room.clone(),
            verification_code: VerificationCode::parse("000000").unwrap(),
        },
        EngineCommand::ReconnectRelationship {
            relationship_id: ids.relationship.clone(),
        },
        EngineCommand::CreateTransfer {
            relationship_id: ids.relationship.clone(),
            content_id: ids.content,
            direction: TransferDirection::Send,
        },
        EngineCommand::PauseTransfer {
            transfer_id: ids.transfer.clone(),
        },
        EngineCommand::ResumeTransfer {
            transfer_id: ids.transfer.clone(),
        },
        EngineCommand::CancelTransfer {
            transfer_id: ids.transfer,
        },
        EngineCommand::RevokeRelationship {
            relationship_id: ids.relationship,
        },
    ];
    for (index, command) in commands.into_iter().enumerate() {
        let value = CommandEnvelope {
            contract_version: APPLICATION_CONTRACT_VERSION,
            command_id: CommandId::parse(format!("command_fixture_{index}")).unwrap(),
            command,
        };
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["contract_version"], APPLICATION_CONTRACT_VERSION);
        assert!(json["command"]["command"].is_string());
        let decoded: CommandEnvelope = serde_json::from_value(json).unwrap();
        assert!(decoded == value);
    }

    let invalid_invitation = serde_json::json!({
        "contract_version": APPLICATION_CONTRACT_VERSION,
        "command_id": "command_invalid_invitation",
        "command": { "command": "join_room", "invitation": "" }
    });
    assert!(serde_json::from_value::<CommandEnvelope>(invalid_invitation).is_err());

    let invalid_verification_code = serde_json::json!({
        "contract_version": APPLICATION_CONTRACT_VERSION,
        "command_id": "command_invalid_verification_code",
        "command": {
            "command": "verify_pairing",
            "room_id": "room_fixture_0001",
            "verification_code": "12345a"
        }
    });
    assert!(serde_json::from_value::<CommandEnvelope>(invalid_verification_code).is_err());

    let capabilities = PlatformCapabilities::new([(
        PlatformCapability::ClipboardRead,
        CapabilityAvailability::Unavailable,
    )]);
    assert_eq!(
        capabilities.availability(PlatformCapability::ClipboardRead),
        CapabilityAvailability::Unavailable
    );
    assert_eq!(
        capabilities.availability(PlatformCapability::Notifications),
        CapabilityAvailability::Unavailable
    );

    let mut incompatible = remembered_transfer_events(&fixture_ids()).remove(0);
    incompatible.contract_version += 1;
    assert!(matches!(
        EngineSnapshot::new().apply(incompatible),
        Err(ApplyError::UnsupportedContractVersion { .. })
    ));
}

#[test]
fn failure_and_cancellation_are_typed_terminal_facts() {
    let ids = fixture_ids();
    let base_events = remembered_transfer_events(&ids);
    let mut snapshot = replay(EngineSnapshot::new(), base_events[..6].iter().cloned()).unwrap();
    let failure = TransferFailure {
        code: FailureCode::NetworkLost,
        phase: FailurePhase::Transferring,
        retryable: true,
        recovery_action: RecoveryAction::Retry,
    };

    for event in [
        envelope(
            7,
            EngineEvent::TransferCreated {
                transfer_id: ids.transfer.clone(),
                relationship_id: ids.relationship.clone(),
                room_id: Some(ids.room.clone()),
                content_id: ids.content,
                direction: TransferDirection::Send,
                total_bytes: 42,
            },
        ),
        envelope(
            8,
            EngineEvent::TransferStarted {
                transfer_id: ids.transfer.clone(),
            },
        ),
        envelope(
            9,
            EngineEvent::TransferFailed {
                transfer_id: ids.transfer.clone(),
                failure: failure.clone(),
            },
        ),
    ] {
        snapshot.apply(event).unwrap();
    }
    assert_eq!(
        snapshot.transfers[&ids.transfer].state,
        TransferState::Failed
    );
    assert_eq!(
        snapshot.transfers[&ids.transfer].failure.as_ref(),
        Some(&failure)
    );

    let canceled_transfer = TransferId::parse("transfer_fixture_0002").unwrap();
    for event in [
        envelope(
            10,
            EngineEvent::TransferCreated {
                transfer_id: canceled_transfer.clone(),
                relationship_id: ids.relationship,
                room_id: Some(ids.room),
                content_id: ContentId::parse("content_fixture_0002").unwrap(),
                direction: TransferDirection::Receive,
                total_bytes: 7,
            },
        ),
        envelope(
            11,
            EngineEvent::TransferCanceled {
                transfer_id: canceled_transfer.clone(),
            },
        ),
    ] {
        snapshot.apply(event).unwrap();
    }
    assert_eq!(
        snapshot.transfers[&canceled_transfer].state,
        TransferState::Canceled
    );

    let before_invalid_terminal_event = snapshot.clone();
    let error = snapshot
        .apply(envelope(
            12,
            EngineEvent::TransferProgressed {
                transfer_id: canceled_transfer,
                transferred_bytes: 1,
            },
        ))
        .unwrap_err();
    assert_eq!(error.code(), ApplicationErrorCode::InvalidTransition);
    assert_eq!(snapshot, before_invalid_terminal_event);
}
