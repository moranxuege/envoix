use envoix_attempt_api::{AdmittedAttemptEvent, AttemptPlan, AttemptStamp, RetirementIntent};
use envoix_capabilities::{AdmittedDutyResult, Duty};
use envoix_outcomes::{Outcome, Phase};
use envoix_types::{AttemptGen, ByteCount, Direction, OfferedName, RequestId};
use serde::{Deserialize, Serialize};

use crate::ProductIdentity;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseOrigin {
    Local,
    Peer,
    Lost,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "origin")]
pub enum ProductState {
    Preparing,
    Waiting,
    Connecting,
    Verifying,
    Transferring,
    Confirming,
    Paused(PauseOrigin),
    Unconfirmed,
    Completed,
    Failed,
    Cancelled,
}

impl ProductState {
    pub const fn is_active(self) -> bool {
        match self {
            Self::Waiting
            | Self::Connecting
            | Self::Verifying
            | Self::Transferring
            | Self::Confirming => true,
            Self::Preparing
            | Self::Paused(_)
            | Self::Unconfirmed
            | Self::Completed
            | Self::Failed
            | Self::Cancelled => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDecision {
    Ready,
    Stage { recoverable: bool },
    NeedsRepick,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub source_ready: bool,
    pub complete_sent: bool,
    pub proof_delivered: bool,
    pub receipt_mismatch: bool,
    pub remove_requested: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTransfer {
    pub direction: Direction,
    pub offered_name: OfferedName,
    pub total: ByteCount,
    pub source: SourceDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductCommand {
    Pause,
    Cancel,
    Resume,
    Remove,
    RePickSource,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProductInput {
    Command(ProductCommand),
    Restore,
    StageProgress {
        stamp: AttemptStamp,
        transferred: ByteCount,
    },
    StageComplete {
        stamp: AttemptStamp,
        total: ByteCount,
    },
    StageFailed {
        stamp: AttemptStamp,
    },
    Advertised {
        stamp: AttemptStamp,
    },
    VerificationStarted {
        stamp: AttemptStamp,
    },
    VerificationFinished {
        stamp: AttemptStamp,
    },
    AttemptObserved(AdmittedAttemptEvent),
    AttemptEnded {
        stamp: AttemptStamp,
    },
    ConfirmTimeout {
        stamp: AttemptStamp,
    },
    ReceiptVerified {
        stamp: AttemptStamp,
    },
    ReceiptMismatch {
        stamp: AttemptStamp,
    },
    ReceiptPosted(AdmittedDutyResult),
    StorageFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityAction {
    PostReceipt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageAction {
    DiscardPartial,
    TombstoneCard,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "effect")]
pub enum ProductEffect {
    StartAttempt {
        plan: AttemptPlan,
    },
    RetireAttempt {
        stamp: AttemptStamp,
        intent: RetirementIntent,
    },
    StartConfirmTimer {
        stamp: AttemptStamp,
    },
    StopConfirmTimer {
        stamp: AttemptStamp,
    },
    StartMailboxPoll {
        stamp: AttemptStamp,
    },
    StopMailboxPoll {
        stamp: AttemptStamp,
    },
    CapabilityDuty {
        duty: Duty,
        action: CapabilityAction,
    },
    StorageIntent {
        identity: ProductIdentity,
        action: StorageAction,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRecord {
    pub identity: ProductIdentity,
    pub direction: Direction,
    pub offered_name: OfferedName,
    pub total: ByteCount,
    pub state: ProductState,
    pub generation: AttemptGen,
    pub phase: Phase,
    pub bytes: ByteCount,
    pub bytes_resumed: ByteCount,
    pub outcome: Option<Outcome>,
    pub facts: Facts,
    pub source_recoverable: bool,
    pub(crate) receipt_request: RequestId,
}
