use envoix_client::APPLICATION_CONTRACT_VERSION;
use envoix_client::command::{CommandEnvelope, EngineCommand};
use envoix_client::decision::decide;
use envoix_client::event::{EngineEvent, EventEnvelope};
use envoix_client::model::{
    CommandId, ContentId, DeviceId, FailureCode, FailurePhase, RecoveryAction, RelationshipId,
    Transfer, TransferDirection, TransferFailure, TransferId, TransferRejection, TransferState,
};
use envoix_client::snapshot::EngineSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Accept,
    Reject,
    Pause,
    Resume,
    Recover,
    Cancel,
    Remove,
}

const ALL_ACTIONS: [Action; 7] = [
    Action::Accept,
    Action::Reject,
    Action::Pause,
    Action::Resume,
    Action::Recover,
    Action::Cancel,
    Action::Remove,
];

struct FixtureIds {
    device: DeviceId,
    relationship: RelationshipId,
    transfer: TransferId,
    content: ContentId,
}

fn fixture_ids() -> FixtureIds {
    FixtureIds {
        device: DeviceId::parse("dev_transition_table").unwrap(),
        relationship: RelationshipId::parse("rel_transition_table").unwrap(),
        transfer: TransferId::parse("transfer_transition_table").unwrap(),
        content: ContentId::parse("content_transition_table").unwrap(),
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

fn trusted_snapshot(ids: &FixtureIds) -> EngineSnapshot {
    let mut snapshot = EngineSnapshot::new();
    apply_next(
        &mut snapshot,
        EngineEvent::DeviceObserved {
            device_id: ids.device.clone(),
            display_name: "Transition fixture".into(),
        },
    );
    apply_next(
        &mut snapshot,
        EngineEvent::RelationshipTrusted {
            relationship_id: ids.relationship.clone(),
            device_id: ids.device.clone(),
            generation: 1,
        },
    );
    snapshot
}

fn transfer(ids: &FixtureIds, state: TransferState, failure: Option<TransferFailure>) -> Transfer {
    let total_bytes = 1;
    Transfer {
        id: ids.transfer.clone(),
        relationship_id: ids.relationship.clone(),
        room_id: None,
        content_id: ids.content.clone(),
        direction: TransferDirection::Send,
        state,
        transferred_bytes: if matches!(
            state,
            TransferState::AwaitingDeliveryProof | TransferState::Delivered
        ) {
            total_bytes
        } else {
            0
        },
        total_bytes,
        failure,
        rejection: (state == TransferState::Rejected).then_some(TransferRejection::UserDeclined),
    }
}

fn command(ids: &FixtureIds, action: Action) -> CommandEnvelope {
    let command = match action {
        Action::Accept => EngineCommand::AcceptTransfer {
            transfer_id: ids.transfer.clone(),
        },
        Action::Reject => EngineCommand::RejectTransfer {
            transfer_id: ids.transfer.clone(),
            reason: TransferRejection::UserDeclined,
        },
        Action::Pause => EngineCommand::PauseTransfer {
            transfer_id: ids.transfer.clone(),
        },
        Action::Resume => EngineCommand::ResumeTransfer {
            transfer_id: ids.transfer.clone(),
        },
        Action::Recover => EngineCommand::RecoverTransfer {
            transfer_id: ids.transfer.clone(),
        },
        Action::Cancel => EngineCommand::CancelTransfer {
            transfer_id: ids.transfer.clone(),
        },
        Action::Remove => EngineCommand::RemoveTransfer {
            transfer_id: ids.transfer.clone(),
        },
    };
    CommandEnvelope {
        contract_version: APPLICATION_CONTRACT_VERSION,
        command_id: CommandId::parse("command_transition_table").unwrap(),
        command,
    }
}

#[test]
fn transfer_action_policy_is_exhaustive_for_every_state() {
    let ids = fixture_ids();
    let recoverable = TransferFailure {
        code: FailureCode::NetworkLost,
        phase: FailurePhase::Transferring,
        retryable: true,
        recovery_action: RecoveryAction::Resume,
    };
    let fatal = TransferFailure {
        code: FailureCode::IntegrityFailure,
        phase: FailurePhase::Verifying,
        retryable: false,
        recovery_action: RecoveryAction::None,
    };
    let rows: &[(TransferState, Option<TransferFailure>, &[Action])] = &[
        (
            TransferState::Offered,
            None,
            &[Action::Accept, Action::Reject],
        ),
        (TransferState::Queued, None, &[Action::Cancel]),
        (
            TransferState::Connecting,
            None,
            &[Action::Pause, Action::Cancel],
        ),
        (
            TransferState::Transferring,
            None,
            &[Action::Pause, Action::Cancel],
        ),
        (
            TransferState::Paused,
            None,
            &[Action::Resume, Action::Cancel],
        ),
        (TransferState::AwaitingDeliveryProof, None, &[]),
        (TransferState::Delivered, None, &[Action::Remove]),
        (TransferState::Rejected, None, &[Action::Remove]),
        (
            TransferState::Failed,
            Some(recoverable),
            &[Action::Recover, Action::Remove],
        ),
        (TransferState::Failed, Some(fatal), &[Action::Remove]),
        (TransferState::Canceled, None, &[Action::Remove]),
    ];

    for (state, failure, allowed) in rows {
        let mut snapshot = trusted_snapshot(&ids);
        snapshot.transfers.insert(
            ids.transfer.clone(),
            transfer(&ids, *state, failure.clone()),
        );
        for action in ALL_ACTIONS {
            assert_eq!(
                decide(&snapshot, command(&ids, action)).is_ok(),
                allowed.contains(&action),
                "state={state:?}, action={action:?}"
            );
        }
    }
}

#[test]
fn revocation_blocks_new_attempts_but_allows_safe_settlement() {
    let ids = fixture_ids();
    let recoverable = TransferFailure {
        code: FailureCode::NetworkLost,
        phase: FailurePhase::Transferring,
        retryable: true,
        recovery_action: RecoveryAction::Resume,
    };
    let mut revoked = trusted_snapshot(&ids);
    apply_next(
        &mut revoked,
        EngineEvent::RelationshipRevoked {
            relationship_id: ids.relationship.clone(),
        },
    );

    for (state, event) in [
        (
            TransferState::Offered,
            EngineEvent::TransferAccepted {
                transfer_id: ids.transfer.clone(),
            },
        ),
        (
            TransferState::Queued,
            EngineEvent::TransferStarted {
                transfer_id: ids.transfer.clone(),
            },
        ),
        (
            TransferState::Paused,
            EngineEvent::TransferResumed {
                transfer_id: ids.transfer.clone(),
            },
        ),
        (
            TransferState::Failed,
            EngineEvent::TransferRecoveryStarted {
                transfer_id: ids.transfer.clone(),
            },
        ),
    ] {
        let mut snapshot = revoked.clone();
        snapshot.transfers.insert(
            ids.transfer.clone(),
            transfer(
                &ids,
                state,
                (state == TransferState::Failed).then(|| recoverable.clone()),
            ),
        );
        if let Some(action) = match state {
            TransferState::Offered => Some(Action::Accept),
            TransferState::Paused => Some(Action::Resume),
            TransferState::Failed => Some(Action::Recover),
            _ => None,
        } {
            assert!(
                decide(&snapshot, command(&ids, action)).is_err(),
                "state={state:?}, action={action:?}"
            );
        }
        let before = snapshot.clone();
        assert!(
            snapshot
                .apply(EventEnvelope {
                    contract_version: APPLICATION_CONTRACT_VERSION,
                    sequence: snapshot.last_sequence + 1,
                    event,
                })
                .is_err(),
            "state={state:?}"
        );
        assert_eq!(snapshot, before, "state={state:?}");
    }

    let mut finalizing = revoked.clone();
    let mut active = transfer(&ids, TransferState::Transferring, None);
    active.transferred_bytes = active.total_bytes;
    finalizing.transfers.insert(ids.transfer.clone(), active);
    apply_next(
        &mut finalizing,
        EngineEvent::TransferPayloadCompleted {
            transfer_id: ids.transfer.clone(),
        },
    );
    apply_next(
        &mut finalizing,
        EngineEvent::TransferDeliveryProofVerified {
            transfer_id: ids.transfer.clone(),
        },
    );
    assert_eq!(
        finalizing.transfers[&ids.transfer].state,
        TransferState::Delivered
    );

    let mut rejected = revoked.clone();
    rejected.transfers.insert(
        ids.transfer.clone(),
        transfer(&ids, TransferState::Offered, None),
    );
    apply_next(
        &mut rejected,
        EngineEvent::TransferRejected {
            transfer_id: ids.transfer.clone(),
            reason: TransferRejection::UserDeclined,
        },
    );

    let mut canceled = revoked;
    canceled.transfers.insert(
        ids.transfer.clone(),
        transfer(&ids, TransferState::Paused, None),
    );
    apply_next(
        &mut canceled,
        EngineEvent::TransferCanceled {
            transfer_id: ids.transfer,
        },
    );
}

#[test]
fn rejected_transfer_events_are_atomic_for_every_state() {
    let ids = fixture_ids();
    let states = [
        TransferState::Offered,
        TransferState::Queued,
        TransferState::Connecting,
        TransferState::Transferring,
        TransferState::Paused,
        TransferState::AwaitingDeliveryProof,
        TransferState::Delivered,
        TransferState::Rejected,
        TransferState::Failed,
        TransferState::Canceled,
    ];
    let failure = TransferFailure {
        code: FailureCode::NetworkLost,
        phase: FailurePhase::Transferring,
        retryable: true,
        recovery_action: RecoveryAction::Resume,
    };

    for state in states {
        let events = [
            EngineEvent::TransferAccepted {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferRejected {
                transfer_id: ids.transfer.clone(),
                reason: TransferRejection::UserDeclined,
            },
            EngineEvent::TransferStarted {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferProgressed {
                transfer_id: ids.transfer.clone(),
                transferred_bytes: 1,
            },
            EngineEvent::TransferPaused {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferResumed {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferRecoveryStarted {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferPayloadCompleted {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferDeliveryProofVerified {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferFailed {
                transfer_id: ids.transfer.clone(),
                failure: failure.clone(),
            },
            EngineEvent::TransferCanceled {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferRemoved {
                transfer_id: ids.transfer.clone(),
            },
            EngineEvent::TransferDelivered {
                transfer_id: ids.transfer.clone(),
            },
        ];

        for event in events {
            let mut snapshot = trusted_snapshot(&ids);
            snapshot.transfers.insert(
                ids.transfer.clone(),
                transfer(
                    &ids,
                    state,
                    (state == TransferState::Failed).then(|| failure.clone()),
                ),
            );
            let before = snapshot.clone();
            let sequence = snapshot.last_sequence + 1;
            let result = snapshot.apply(EventEnvelope {
                contract_version: APPLICATION_CONTRACT_VERSION,
                sequence,
                event,
            });
            if result.is_err() {
                assert_eq!(snapshot, before, "state={state:?}");
            } else {
                assert_eq!(snapshot.last_sequence, sequence, "state={state:?}");
                assert!(
                    snapshot
                        .transfers
                        .get(&ids.transfer)
                        .is_none_or(|transfer| transfer.transferred_bytes <= transfer.total_bytes)
                );
            }
        }
    }
}

#[test]
fn restart_continuation_matches_uninterrupted_reduction() {
    let ids = fixture_ids();
    let failure = TransferFailure {
        code: FailureCode::NetworkLost,
        phase: FailurePhase::Transferring,
        retryable: true,
        recovery_action: RecoveryAction::Resume,
    };
    let events = [
        EngineEvent::TransferCreated {
            transfer_id: ids.transfer.clone(),
            relationship_id: ids.relationship.clone(),
            room_id: None,
            content_id: ids.content.clone(),
            direction: TransferDirection::Send,
            total_bytes: 2,
        },
        EngineEvent::TransferStarted {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::TransferProgressed {
            transfer_id: ids.transfer.clone(),
            transferred_bytes: 1,
        },
        EngineEvent::TransferFailed {
            transfer_id: ids.transfer.clone(),
            failure,
        },
        EngineEvent::TransferRecoveryStarted {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::TransferProgressed {
            transfer_id: ids.transfer.clone(),
            transferred_bytes: 2,
        },
        EngineEvent::TransferPayloadCompleted {
            transfer_id: ids.transfer.clone(),
        },
        EngineEvent::TransferDeliveryProofVerified {
            transfer_id: ids.transfer.clone(),
        },
    ];
    let mut uninterrupted = trusted_snapshot(&ids);
    let mut interrupted = trusted_snapshot(&ids);

    for event in &events[..4] {
        apply_next(&mut uninterrupted, event.clone());
        apply_next(&mut interrupted, event.clone());
    }
    let wire = serde_json::to_vec(&interrupted).unwrap();
    let mut restored: EngineSnapshot = serde_json::from_slice(&wire).unwrap();

    for event in &events[4..] {
        apply_next(&mut uninterrupted, event.clone());
        apply_next(&mut restored, event.clone());
    }
    assert_eq!(restored, uninterrupted);
    assert_eq!(
        restored.transfers[&ids.transfer].state,
        TransferState::Delivered
    );
}
