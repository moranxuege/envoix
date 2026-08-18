//! Canonical projection from session failures to application recovery policy.

use envoix_error::{CoreError, RendezvousCause, TransferCause};

use crate::model::{
    FailureCode, FailureOrigin, FailurePhase, RecoveryAction, TransferDirection, TransferFailure,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionFailureProjection {
    pub failure: TransferFailure,
    pub origin: FailureOrigin,
}

pub fn project_session_failure(
    error: &CoreError,
    direction: TransferDirection,
    fallback_phase: FailurePhase,
) -> SessionFailureProjection {
    let (error, invitation_consumed) = match error {
        CoreError::InvitationConsumed(source) => (source.as_ref(), true),
        error => (error, false),
    };
    let mut projection = match error {
        CoreError::Cause { cause, .. } => project_transfer_cause(*cause),
        CoreError::Rendezvous { cause, .. } => project_rendezvous_cause(*cause),
        CoreError::Cancelled => projection(
            FailureCode::UserCanceled,
            fallback_phase,
            FailureOrigin::Local,
            false,
            RecoveryAction::None,
        ),
        CoreError::Transport(_) | CoreError::Discovery(_) => projection(
            FailureCode::NetworkLost,
            fallback_phase,
            FailureOrigin::Unknown,
            true,
            RecoveryAction::Resume,
        ),
        CoreError::Crypto(_) => projection(
            FailureCode::AuthenticationFailed,
            FailurePhase::Authenticating,
            FailureOrigin::Unknown,
            true,
            RecoveryAction::RePair,
        ),
        CoreError::Protocol(_) => projection(
            FailureCode::ProtocolOrIntegrityFailure,
            FailurePhase::Verifying,
            FailureOrigin::Unknown,
            false,
            RecoveryAction::None,
        ),
        CoreError::Io(_) | CoreError::Storage(_) => {
            let (code, recovery_action) = match direction {
                TransferDirection::Send => {
                    (FailureCode::SenderSourceUnavailable, RecoveryAction::Retry)
                }
                TransferDirection::Receive => {
                    (FailureCode::ReceiverSaveFailed, RecoveryAction::Resume)
                }
            };
            projection(
                code,
                fallback_phase,
                FailureOrigin::Local,
                true,
                recovery_action,
            )
        }
        CoreError::InvalidInput(_) => projection(
            FailureCode::UnsupportedFeature,
            fallback_phase,
            FailureOrigin::Local,
            false,
            RecoveryAction::None,
        ),
        CoreError::Transfer(_) => projection(
            FailureCode::InternalError,
            fallback_phase,
            FailureOrigin::Unknown,
            true,
            RecoveryAction::Retry,
        ),
        CoreError::InvitationConsumed(_) => unreachable!("consumed invitation was unwrapped"),
    };
    if invitation_consumed && projection.failure.retryable {
        projection.failure.recovery_action = RecoveryAction::RePair;
    }
    projection
}

fn project_rendezvous_cause(cause: RendezvousCause) -> SessionFailureProjection {
    let (code, retryable, recovery_action) = match cause {
        RendezvousCause::RoomNotFound => (FailureCode::RoomNotFound, true, RecoveryAction::Retry),
        RendezvousCause::RoomExpired => (FailureCode::RoomExpired, true, RecoveryAction::RePair),
        RendezvousCause::RoomFull => (FailureCode::RoomFull, true, RecoveryAction::Retry),
        RendezvousCause::RoomRateLimited => {
            (FailureCode::RoomRateLimited, true, RecoveryAction::Retry)
        }
        RendezvousCause::RoomUnderAttack => {
            (FailureCode::RoomUnderAttack, true, RecoveryAction::RePair)
        }
        RendezvousCause::EndpointRateLimited => (
            FailureCode::EndpointRateLimited,
            true,
            RecoveryAction::Retry,
        ),
        RendezvousCause::IpRateLimited => (FailureCode::IpRateLimited, true, RecoveryAction::Retry),
        RendezvousCause::ServerBusy => (FailureCode::ServerBusy, true, RecoveryAction::Retry),
        RendezvousCause::MalformedJoin => (FailureCode::MalformedJoin, false, RecoveryAction::None),
        RendezvousCause::UnsupportedVersion => (
            FailureCode::UnsupportedRendezvousVersion,
            false,
            RecoveryAction::None,
        ),
    };
    projection(
        code,
        FailurePhase::Pairing,
        FailureOrigin::Unknown,
        retryable,
        recovery_action,
    )
}

fn project_transfer_cause(cause: TransferCause) -> SessionFailureProjection {
    use FailureCode as Code;
    use FailureOrigin::{Local, Unknown};
    use FailurePhase::{Committing, Connecting, Negotiating, Transferring, Verifying};
    use RecoveryAction::{ChooseFolder, None, OpenSettings, Resume, Retry};

    let (code, phase, origin, retryable, recovery_action) = match cause {
        TransferCause::NearbyHybridPreAuthTransportFailure => {
            (Code::NetworkLost, Connecting, Unknown, true, Resume)
        }
        TransferCause::SenderSourceUnavailable => (
            Code::SenderSourceUnavailable,
            Transferring,
            Local,
            true,
            Retry,
        ),
        TransferCause::SenderPermissionLost => (
            Code::SenderPermissionLost,
            Transferring,
            Local,
            true,
            OpenSettings,
        ),
        TransferCause::SenderSourceChanged => {
            (Code::SenderSourceChanged, Verifying, Local, true, Retry)
        }
        TransferCause::SenderItemRemoved => {
            (Code::SenderItemRemoved, Transferring, Local, false, None)
        }
        TransferCause::SenderCanceled => (Code::SenderCanceled, Transferring, Local, false, None),
        TransferCause::ProtocolOrIntegrityFailure => (
            Code::ProtocolOrIntegrityFailure,
            Verifying,
            Unknown,
            false,
            None,
        ),
        TransferCause::ReceiverSpaceInsufficient => (
            Code::ReceiverSpaceInsufficient,
            Negotiating,
            Local,
            true,
            ChooseFolder,
        ),
        TransferCause::ReceiverDestinationDecisionRequired => (
            Code::ReceiverDestinationDecisionRequired,
            Negotiating,
            Local,
            true,
            ChooseFolder,
        ),
        TransferCause::ReceiverDestinationUnavailable => (
            Code::ReceiverDestinationUnavailable,
            Committing,
            Local,
            true,
            ChooseFolder,
        ),
        TransferCause::ReceiverSaveFailed => {
            (Code::ReceiverSaveFailed, Committing, Local, true, Resume)
        }
        TransferCause::ReceiverReusedObjectLost => (
            Code::ReceiverReusedObjectLost,
            Committing,
            Local,
            true,
            Resume,
        ),
        TransferCause::ReceiverFinalizationOutcomeUnknown => (
            Code::ReceiverFinalizationOutcomeUnknown,
            Committing,
            Local,
            true,
            Resume,
        ),
    };
    projection(code, phase, origin, retryable, recovery_action)
}

fn projection(
    code: FailureCode,
    phase: FailurePhase,
    origin: FailureOrigin,
    retryable: bool,
    recovery_action: RecoveryAction,
) -> SessionFailureProjection {
    SessionFailureProjection {
        failure: TransferFailure {
            code,
            phase,
            retryable,
            recovery_action,
        },
        origin,
    }
}
