use std::collections::BTreeSet;

use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::command::{CommandEnvelope, EngineCommand};
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::runtime::replay;
use envoix_client::snapshot::EngineSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApplicationContractFixture {
    contract_version: u16,
    commands: Vec<CommandEnvelope>,
    events: Vec<EventEnvelope>,
    snapshot: EngineSnapshot,
}

fn command_tag(command: &EngineCommand) -> &'static str {
    match command {
        EngineCommand::CreateRoom => "create_room",
        EngineCommand::JoinRoom { .. } => "join_room",
        EngineCommand::VerifyPairing { .. } => "verify_pairing",
        EngineCommand::ReconnectRelationship { .. } => "reconnect_relationship",
        EngineCommand::CreateTransfer { .. } => "create_transfer",
        EngineCommand::PauseTransfer { .. } => "pause_transfer",
        EngineCommand::ResumeTransfer { .. } => "resume_transfer",
        EngineCommand::CancelTransfer { .. } => "cancel_transfer",
        EngineCommand::RevokeRelationship { .. } => "revoke_relationship",
    }
}

fn event_tag(event: &EngineEvent) -> &'static str {
    match event {
        EngineEvent::CapabilitiesChanged { .. } => "capabilities_changed",
        EngineEvent::DeviceObserved { .. } => "device_observed",
        EngineEvent::RelationshipTrusted { .. } => "relationship_trusted",
        EngineEvent::RelationshipRevoked { .. } => "relationship_revoked",
        EngineEvent::RoomOpened { .. } => "room_opened",
        EngineEvent::RoomConnected { .. } => "room_connected",
        EngineEvent::RoomClosed { .. } => "room_closed",
        EngineEvent::TransferCreated { .. } => "transfer_created",
        EngineEvent::TransferStarted { .. } => "transfer_started",
        EngineEvent::TransferProgressed { .. } => "transfer_progressed",
        EngineEvent::TransferPaused { .. } => "transfer_paused",
        EngineEvent::TransferResumed { .. } => "transfer_resumed",
        EngineEvent::TransferDelivered { .. } => "transfer_delivered",
        EngineEvent::TransferFailed { .. } => "transfer_failed",
        EngineEvent::TransferCanceled { .. } => "transfer_canceled",
    }
}

#[test]
fn application_contract_v1_fixture_is_complete_and_replayable() {
    let raw = include_str!("../../../tests/fixtures/v0.3/application-contract-v1.json");
    let json: serde_json::Value = serde_json::from_str(raw).unwrap();
    let fixture: ApplicationContractFixture = serde_json::from_value(json.clone()).unwrap();

    assert_eq!(fixture.contract_version, APPLICATION_CONTRACT_VERSION);
    assert!(
        fixture
            .commands
            .iter()
            .all(|command| command.contract_version == APPLICATION_CONTRACT_VERSION)
    );
    assert!(
        fixture
            .events
            .iter()
            .all(|event| event.contract_version == APPLICATION_CONTRACT_VERSION)
    );

    let command_tags = fixture
        .commands
        .iter()
        .map(|envelope| command_tag(&envelope.command))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        command_tags,
        BTreeSet::from([
            "cancel_transfer",
            "create_room",
            "create_transfer",
            "join_room",
            "pause_transfer",
            "reconnect_relationship",
            "resume_transfer",
            "revoke_relationship",
            "verify_pairing",
        ])
    );
    assert_eq!(fixture.commands.len(), command_tags.len());

    let event_tags = fixture
        .events
        .iter()
        .map(|envelope| event_tag(&envelope.event))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        event_tags,
        BTreeSet::from([
            "capabilities_changed",
            "device_observed",
            "relationship_revoked",
            "relationship_trusted",
            "room_closed",
            "room_connected",
            "room_opened",
            "transfer_canceled",
            "transfer_created",
            "transfer_delivered",
            "transfer_failed",
            "transfer_paused",
            "transfer_progressed",
            "transfer_resumed",
            "transfer_started",
        ])
    );

    for (index, event) in fixture.events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
    }
    let replayed = replay(EngineSnapshot::new(), fixture.events.clone()).unwrap();
    assert_eq!(replayed, fixture.snapshot);
    assert_eq!(serde_json::to_value(&fixture).unwrap(), json);
}
