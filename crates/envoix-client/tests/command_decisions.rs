use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::command::{CommandEnvelope, EngineCommand, VerificationCode};
use envoix_client::decision::decide;
use envoix_client::effect::{EffectEnvelope, EngineEffect};
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::model::{
    CommandId, ContentId, DeviceId, EntityKind, RelationshipId, RoomId, TransferDirection,
    TransferId,
};
use envoix_client::snapshot::{ApplicationErrorCode, ApplyError, EngineSnapshot};

struct FixtureIds {
    device: DeviceId,
    relationship: RelationshipId,
    room: RoomId,
    transfer: TransferId,
    content: ContentId,
}

fn fixture_ids() -> FixtureIds {
    FixtureIds {
        device: DeviceId::parse("dev_decision").unwrap(),
        relationship: RelationshipId::parse("rel_decision").unwrap(),
        room: RoomId::parse("room_decision").unwrap(),
        transfer: TransferId::parse("transfer_decision").unwrap(),
        content: ContentId::parse("content_decision").unwrap(),
    }
}

fn apply_next(snapshot: &mut EngineSnapshot, event: EngineEvent) {
    snapshot
        .apply(EventEnvelope {
            contract_version: APPLICATION_CONTRACT_VERSION,
            sequence: snapshot.last_sequence + 1,
            event,
        })
        .unwrap();
}

fn command(command: EngineCommand) -> CommandEnvelope {
    CommandEnvelope {
        contract_version: APPLICATION_CONTRACT_VERSION,
        command_id: CommandId::parse("command_decision").unwrap(),
        command,
    }
}

fn expect_error(result: Result<EffectEnvelope, ApplyError>) -> ApplyError {
    match result {
        Ok(_) => panic!("command unexpectedly produced an effect"),
        Err(error) => error,
    }
}

fn trust_relationship(snapshot: &mut EngineSnapshot, ids: &FixtureIds) {
    apply_next(
        snapshot,
        EngineEvent::DeviceObserved {
            device_id: ids.device.clone(),
            display_name: "Decision fixture".into(),
        },
    );
    apply_next(
        snapshot,
        EngineEvent::RelationshipTrusted {
            relationship_id: ids.relationship.clone(),
            device_id: ids.device.clone(),
            generation: 4,
        },
    );
}

#[test]
fn reconnect_decision_resolves_the_shared_generation_state() {
    let ids = fixture_ids();
    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);
    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRotated {
            relationship_id: ids.relationship.clone(),
            generation: 5,
        },
    );

    let decision = decide(
        &snapshot,
        command(EngineCommand::ReconnectRelationship {
            relationship_id: ids.relationship.clone(),
        }),
    )
    .unwrap();

    assert_eq!(decision.contract_version, APPLICATION_CONTRACT_VERSION);
    assert_eq!(decision.command_id.as_str(), "command_decision");
    assert!(matches!(
        decision.effect,
        EngineEffect::ReconnectRelationship {
            relationship_id,
            generation: 5,
            previous_generation: Some(4),
        } if relationship_id == ids.relationship
    ));
}

#[test]
fn pairing_verification_requires_a_connected_room() {
    let ids = fixture_ids();
    let verification_code = VerificationCode::parse("123456").unwrap();
    let mut snapshot = EngineSnapshot::new();

    let error = expect_error(decide(
        &snapshot,
        command(EngineCommand::VerifyPairing {
            room_id: ids.room.clone(),
            verification_code: verification_code.clone(),
        }),
    ));
    assert_eq!(error.code(), ApplicationErrorCode::EntityNotFound);

    apply_next(
        &mut snapshot,
        EngineEvent::RoomOpened {
            room_id: ids.room.clone(),
            relationship_id: None,
            replaces_room_id: None,
        },
    );
    let error = expect_error(decide(
        &snapshot,
        command(EngineCommand::VerifyPairing {
            room_id: ids.room.clone(),
            verification_code: verification_code.clone(),
        }),
    ));
    assert_eq!(error.code(), ApplicationErrorCode::InvalidTransition);

    apply_next(
        &mut snapshot,
        EngineEvent::RoomPeerAdmitted {
            room_id: ids.room.clone(),
        },
    );
    apply_next(
        &mut snapshot,
        EngineEvent::RoomAuthenticated {
            room_id: ids.room.clone(),
        },
    );
    let decision = decide(
        &snapshot,
        command(EngineCommand::VerifyPairing {
            room_id: ids.room.clone(),
            verification_code,
        }),
    )
    .unwrap();
    let wire = serde_json::to_vec(&decision).unwrap();
    let decoded: EffectEnvelope = serde_json::from_slice(&wire).unwrap();
    assert!(decoded == decision);
    assert!(matches!(
        decision.effect,
        EngineEffect::VerifyPairing {
            room_id,
            verification_code,
        } if room_id == ids.room && verification_code.expose() == "123456"
    ));
}

#[test]
fn decisions_reject_missing_revoked_and_illegal_transfer_state() {
    let ids = fixture_ids();
    let snapshot = EngineSnapshot::new();
    let error = expect_error(decide(
        &snapshot,
        command(EngineCommand::ReconnectRelationship {
            relationship_id: ids.relationship.clone(),
        }),
    ));
    assert_eq!(error.code(), ApplicationErrorCode::EntityNotFound);

    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);
    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipRevoked {
            relationship_id: ids.relationship.clone(),
        },
    );
    let error = expect_error(decide(
        &snapshot,
        command(EngineCommand::ReconnectRelationship {
            relationship_id: ids.relationship.clone(),
        }),
    ));
    assert!(matches!(
        error,
        ApplyError::InvalidTransition {
            entity: EntityKind::Relationship,
            ..
        }
    ));

    let mut snapshot = EngineSnapshot::new();
    trust_relationship(&mut snapshot, &ids);
    apply_next(
        &mut snapshot,
        EngineEvent::TransferCreated {
            transfer_id: ids.transfer.clone(),
            relationship_id: ids.relationship,
            room_id: None,
            content_id: ids.content,
            direction: TransferDirection::Send,
            total_bytes: 1,
        },
    );
    let error = expect_error(decide(
        &snapshot,
        command(EngineCommand::PauseTransfer {
            transfer_id: ids.transfer,
        }),
    ));
    assert!(matches!(
        error,
        ApplyError::InvalidTransition {
            entity: EntityKind::Transfer,
            ..
        }
    ));
}

#[test]
fn transfer_decisions_follow_the_shared_transfer_states() {
    let ids = fixture_ids();
    let mut queued = EngineSnapshot::new();
    trust_relationship(&mut queued, &ids);

    let create = decide(
        &queued,
        command(EngineCommand::CreateTransfer {
            relationship_id: ids.relationship.clone(),
            content_id: ids.content.clone(),
            direction: TransferDirection::Send,
        }),
    )
    .unwrap();
    assert!(matches!(
        create.effect,
        EngineEffect::CreateTransfer {
            relationship_id,
            content_id,
            direction: TransferDirection::Send,
        } if relationship_id == ids.relationship && content_id == ids.content
    ));
    apply_next(
        &mut queued,
        EngineEvent::TransferCreated {
            transfer_id: ids.transfer.clone(),
            relationship_id: ids.relationship.clone(),
            room_id: None,
            content_id: ids.content,
            direction: TransferDirection::Send,
            total_bytes: 1,
        },
    );

    assert!(
        decide(
            &queued,
            command(EngineCommand::CancelTransfer {
                transfer_id: ids.transfer.clone(),
            }),
        )
        .is_ok()
    );
    assert!(
        decide(
            &queued,
            command(EngineCommand::PauseTransfer {
                transfer_id: ids.transfer.clone(),
            }),
        )
        .is_err()
    );

    let mut active = queued.clone();
    apply_next(
        &mut active,
        EngineEvent::TransferStarted {
            transfer_id: ids.transfer.clone(),
        },
    );
    assert!(
        decide(
            &active,
            command(EngineCommand::PauseTransfer {
                transfer_id: ids.transfer.clone(),
            }),
        )
        .is_ok()
    );
    assert!(
        decide(
            &active,
            command(EngineCommand::ResumeTransfer {
                transfer_id: ids.transfer.clone(),
            }),
        )
        .is_err()
    );

    let mut paused = active;
    apply_next(
        &mut paused,
        EngineEvent::TransferPaused {
            transfer_id: ids.transfer.clone(),
        },
    );
    assert!(
        decide(
            &paused,
            command(EngineCommand::ResumeTransfer {
                transfer_id: ids.transfer.clone(),
            }),
        )
        .is_ok()
    );

    let mut terminal = queued;
    apply_next(
        &mut terminal,
        EngineEvent::TransferCanceled {
            transfer_id: ids.transfer.clone(),
        },
    );
    for command in [
        EngineCommand::PauseTransfer {
            transfer_id: ids.transfer.clone(),
        },
        EngineCommand::ResumeTransfer {
            transfer_id: ids.transfer.clone(),
        },
        EngineCommand::CancelTransfer {
            transfer_id: ids.transfer,
        },
    ] {
        assert!(decide(&terminal, self::command(command)).is_err());
    }
}

#[test]
fn a_command_contract_mismatch_never_produces_an_effect() {
    let mut incompatible = command(EngineCommand::CreateRoom);
    incompatible.contract_version += 1;

    assert!(matches!(
        decide(&EngineSnapshot::new(), incompatible),
        Err(ApplyError::UnsupportedContractVersion {
            expected: APPLICATION_CONTRACT_VERSION,
            ..
        })
    ));

    let mut incompatible_snapshot = EngineSnapshot::new();
    incompatible_snapshot.contract_version += 1;
    assert!(matches!(
        decide(&incompatible_snapshot, command(EngineCommand::CreateRoom)),
        Err(ApplyError::UnsupportedContractVersion {
            expected: APPLICATION_CONTRACT_VERSION,
            ..
        })
    ));
}
