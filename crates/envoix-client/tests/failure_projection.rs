use envoix_client::failure::project_session_failure;
use envoix_client::model::{
    FailureCategory, FailureCode, FailureOrigin, FailurePhase, RecoveryAction, TransferDirection,
};
use envoix_error::{CoreError, RendezvousCause, TransferCause};

struct Case {
    error: CoreError,
    direction: TransferDirection,
    fallback_phase: FailurePhase,
    code: FailureCode,
    category: FailureCategory,
    phase: FailurePhase,
    origin: FailureOrigin,
    retryable: bool,
    recovery_action: RecoveryAction,
}

fn assert_case(case: Case) {
    let projection = project_session_failure(&case.error, case.direction, case.fallback_phase);
    let failure = projection.failure;

    assert_eq!(failure.code, case.code);
    assert_eq!(failure.code.category(), case.category);
    assert_eq!(failure.phase, case.phase);
    assert_eq!(projection.origin, case.origin);
    assert_eq!(failure.retryable, case.retryable);
    assert_eq!(failure.recovery_action, case.recovery_action);
    assert_eq!(
        failure.code.user_message_key(),
        format!("transfer.{}", failure.code.wire_name())
    );
}

#[test]
fn generic_session_failures_have_one_canonical_projection() {
    use FailureCategory::{
        Authentication, Integrity, Internal, Network, Storage, Unsupported, User,
    };
    use FailureCode::{
        AuthenticationFailed, InternalError, NetworkLost, ProtocolOrIntegrityFailure,
        ReceiverSaveFailed, SenderSourceUnavailable, UnsupportedFeature, UserCanceled,
    };
    use FailureOrigin::{Local, Unknown};
    use FailurePhase::{Authenticating, Connecting, Transferring, Verifying};
    use RecoveryAction::{None, RePair, Resume, Retry};
    use TransferDirection::{Receive, Send};

    let cases = [
        Case {
            error: CoreError::Cancelled,
            direction: Send,
            fallback_phase: Transferring,
            code: UserCanceled,
            category: User,
            phase: Transferring,
            origin: Local,
            retryable: false,
            recovery_action: None,
        },
        Case {
            error: CoreError::Transport("offline".into()),
            direction: Send,
            fallback_phase: Connecting,
            code: NetworkLost,
            category: Network,
            phase: Connecting,
            origin: Unknown,
            retryable: true,
            recovery_action: Resume,
        },
        Case {
            error: CoreError::Discovery("peer disappeared".into()),
            direction: Receive,
            fallback_phase: Connecting,
            code: NetworkLost,
            category: Network,
            phase: Connecting,
            origin: Unknown,
            retryable: true,
            recovery_action: Resume,
        },
        Case {
            error: CoreError::Crypto("bad key".into()),
            direction: Receive,
            fallback_phase: Connecting,
            code: AuthenticationFailed,
            category: Authentication,
            phase: Authenticating,
            origin: Unknown,
            retryable: true,
            recovery_action: RePair,
        },
        Case {
            error: CoreError::Protocol("bad digest".into()),
            direction: Receive,
            fallback_phase: Transferring,
            code: ProtocolOrIntegrityFailure,
            category: Integrity,
            phase: Verifying,
            origin: Unknown,
            retryable: false,
            recovery_action: None,
        },
        Case {
            error: CoreError::Io("source gone".into()),
            direction: Send,
            fallback_phase: Transferring,
            code: SenderSourceUnavailable,
            category: Storage,
            phase: Transferring,
            origin: Local,
            retryable: true,
            recovery_action: Retry,
        },
        Case {
            error: CoreError::Storage("write failed".into()),
            direction: Receive,
            fallback_phase: Transferring,
            code: ReceiverSaveFailed,
            category: Storage,
            phase: Transferring,
            origin: Local,
            retryable: true,
            recovery_action: Resume,
        },
        Case {
            error: CoreError::InvalidInput("unsupported option".into()),
            direction: Send,
            fallback_phase: Connecting,
            code: UnsupportedFeature,
            category: Unsupported,
            phase: Connecting,
            origin: Local,
            retryable: false,
            recovery_action: None,
        },
        Case {
            error: CoreError::Transfer("unexpected state".into()),
            direction: Send,
            fallback_phase: Transferring,
            code: InternalError,
            category: Internal,
            phase: Transferring,
            origin: Unknown,
            retryable: true,
            recovery_action: Retry,
        },
    ];

    for case in cases {
        assert_case(case);
    }
}

#[test]
fn typed_transfer_causes_keep_their_product_policy() {
    use FailureCategory::{Integrity, Network, Permission, Storage, User};
    use FailureCode::{
        NetworkLost, ProtocolOrIntegrityFailure, ReceiverDestinationDecisionRequired,
        ReceiverDestinationUnavailable, ReceiverFinalizationOutcomeUnknown,
        ReceiverReusedObjectLost, ReceiverSaveFailed, ReceiverSpaceInsufficient, SenderCanceled,
        SenderItemRemoved, SenderPermissionLost, SenderSourceChanged, SenderSourceUnavailable,
    };
    use FailureOrigin::{Local, Unknown};
    use FailurePhase::{Committing, Connecting, Negotiating, Transferring, Verifying};
    use RecoveryAction::{ChooseFolder, None, OpenSettings, Resume, Retry};
    use TransferDirection::Send;

    let cases = [
        (
            TransferCause::NearbyHybridPreAuthTransportFailure,
            NetworkLost,
            Network,
            Connecting,
            Unknown,
            true,
            Resume,
        ),
        (
            TransferCause::SenderSourceUnavailable,
            SenderSourceUnavailable,
            Storage,
            Transferring,
            Local,
            true,
            Retry,
        ),
        (
            TransferCause::SenderPermissionLost,
            SenderPermissionLost,
            Permission,
            Transferring,
            Local,
            true,
            OpenSettings,
        ),
        (
            TransferCause::SenderSourceChanged,
            SenderSourceChanged,
            Integrity,
            Verifying,
            Local,
            true,
            Retry,
        ),
        (
            TransferCause::SenderItemRemoved,
            SenderItemRemoved,
            User,
            Transferring,
            Local,
            false,
            None,
        ),
        (
            TransferCause::SenderCanceled,
            SenderCanceled,
            User,
            Transferring,
            Local,
            false,
            None,
        ),
        (
            TransferCause::ProtocolOrIntegrityFailure,
            ProtocolOrIntegrityFailure,
            Integrity,
            Verifying,
            Unknown,
            false,
            None,
        ),
        (
            TransferCause::ReceiverSpaceInsufficient,
            ReceiverSpaceInsufficient,
            Storage,
            Negotiating,
            Local,
            true,
            ChooseFolder,
        ),
        (
            TransferCause::ReceiverDestinationDecisionRequired,
            ReceiverDestinationDecisionRequired,
            Storage,
            Negotiating,
            Local,
            true,
            ChooseFolder,
        ),
        (
            TransferCause::ReceiverDestinationUnavailable,
            ReceiverDestinationUnavailable,
            Storage,
            Committing,
            Local,
            true,
            ChooseFolder,
        ),
        (
            TransferCause::ReceiverSaveFailed,
            ReceiverSaveFailed,
            Storage,
            Committing,
            Local,
            true,
            Resume,
        ),
        (
            TransferCause::ReceiverReusedObjectLost,
            ReceiverReusedObjectLost,
            Storage,
            Committing,
            Local,
            true,
            Resume,
        ),
        (
            TransferCause::ReceiverFinalizationOutcomeUnknown,
            ReceiverFinalizationOutcomeUnknown,
            Storage,
            Committing,
            Local,
            true,
            Resume,
        ),
    ];

    for (cause, code, category, phase, origin, retryable, recovery_action) in cases {
        assert_case(Case {
            error: CoreError::Cause {
                cause,
                detail: "fixture detail".into(),
            },
            direction: Send,
            fallback_phase: Transferring,
            code,
            category,
            phase,
            origin,
            retryable,
            recovery_action,
        });
    }
}

#[test]
fn rendezvous_causes_keep_their_product_policy() {
    use FailureCategory::{Network, Unsupported};
    use FailureCode::{
        EndpointRateLimited, IpRateLimited, MalformedJoin, RoomExpired, RoomFull, RoomNotFound,
        RoomRateLimited, RoomUnderAttack, ServerBusy, UnsupportedRendezvousVersion,
    };
    use FailureOrigin::Unknown;
    use FailurePhase::{Pairing, Setup};
    use RecoveryAction::{None, RePair, Retry};
    use TransferDirection::Receive;

    let cases = [
        (
            RendezvousCause::RoomNotFound,
            RoomNotFound,
            Network,
            true,
            Retry,
        ),
        (
            RendezvousCause::RoomExpired,
            RoomExpired,
            Network,
            true,
            RePair,
        ),
        (RendezvousCause::RoomFull, RoomFull, Network, true, Retry),
        (
            RendezvousCause::RoomRateLimited,
            RoomRateLimited,
            Network,
            true,
            Retry,
        ),
        (
            RendezvousCause::RoomUnderAttack,
            RoomUnderAttack,
            Network,
            true,
            RePair,
        ),
        (
            RendezvousCause::EndpointRateLimited,
            EndpointRateLimited,
            Network,
            true,
            Retry,
        ),
        (
            RendezvousCause::IpRateLimited,
            IpRateLimited,
            Network,
            true,
            Retry,
        ),
        (
            RendezvousCause::ServerBusy,
            ServerBusy,
            Network,
            true,
            Retry,
        ),
        (
            RendezvousCause::MalformedJoin,
            MalformedJoin,
            Unsupported,
            false,
            None,
        ),
        (
            RendezvousCause::UnsupportedVersion,
            UnsupportedRendezvousVersion,
            Unsupported,
            false,
            None,
        ),
    ];

    for (cause, code, category, retryable, recovery_action) in cases {
        assert_case(Case {
            error: CoreError::Rendezvous {
                cause,
                retry_after: Some(5),
            },
            direction: Receive,
            fallback_phase: Setup,
            code,
            category,
            phase: Pairing,
            origin: Unknown,
            retryable,
            recovery_action,
        });
    }
}

#[test]
fn consumed_invitation_requires_repair_only_for_retryable_failures() {
    let retryable = project_session_failure(
        &CoreError::InvitationConsumed(Box::new(CoreError::Transport("offline".into()))),
        TransferDirection::Send,
        FailurePhase::Connecting,
    );
    assert!(retryable.failure.retryable);
    assert_eq!(retryable.failure.recovery_action, RecoveryAction::RePair);

    let terminal = project_session_failure(
        &CoreError::InvitationConsumed(Box::new(CoreError::Protocol("bad digest".into()))),
        TransferDirection::Receive,
        FailurePhase::Verifying,
    );
    assert!(!terminal.failure.retryable);
    assert_eq!(terminal.failure.recovery_action, RecoveryAction::None);
}
