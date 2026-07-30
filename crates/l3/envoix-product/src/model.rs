use envoix_attempt_api::{
    AdmittedAttemptEvent, AttemptPlan, AttemptStamp, RetirementAck, RetirementIntent,
};
use envoix_capabilities::{AdmittedDutyResult, AdmittedSourceResult, Duty};
use envoix_outcomes::{Outcome, Phase};
use envoix_types::{AttemptGen, ByteCount, CommandId, Direction, RequestId};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedSourceOffer, PairingChannel, ProductIdentity, RoomParticipation, SourceLifecycle,
    StagedContent,
};

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
pub enum WorkerKind {
    Attempt,
    Staging,
}

/// Durable proof state for the worker that owns the current generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Quiescence {
    Running {
        worker: WorkerKind,
    },
    Retiring {
        worker: WorkerKind,
        intent: RetirementIntent,
    },
    Quiescent,
}

impl Quiescence {
    pub const fn is_quiescent(self) -> bool {
        matches!(self, Self::Quiescent)
    }

    pub const fn is_retiring(self) -> bool {
        matches!(self, Self::Retiring { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub complete_sent: bool,
    pub proof_delivered: bool,
    pub receipt_mismatch: bool,
    pub remove_requested: bool,
}

/// What a frontend states to create a card.
///
/// Carries NO source metadata. A document is chosen after the card is durable,
/// under an acquisition key the authority mints — so a name, a total, and a
/// caller's guess at whether the source was ready have nothing to say here.
/// Their absence is what makes `SourceLifecycle` the only source authority
/// rather than the newest of three.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTransfer {
    pub direction: Direction,
    /// Whether this endpoint minted the room or joined one. Carried from the
    /// create intent because only the intent knows: a mint states its own
    /// direction, a join derives the opposite from an invite only Rust reads.
    pub participation: RoomParticipation,
    /// The rendezvous channel this card is frozen to, minted for a mint or
    /// adopted from the invite a join was created with. `None` is a card with
    /// no channel yet — the shape every pre-F2b creation path had.
    pub pairing: Option<Box<PairingChannel>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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
    /// Reconciles a durable record after process-owned workers were torn down.
    Restore,
    /// A document offered to the acquisition the authority asked for.
    ///
    /// Carries the WHOLE key, never the card alone: a card match is how a
    /// picked document could satisfy a request it was never chosen for.
    SourceOffered {
        offer: AcceptedSourceOffer,
    },
    /// The platform's admitted answer about the acquisition it was asked for.
    ///
    /// The edge that was missing: the source duty was issued and dispatched,
    /// and nothing carried its result back, so no card could leave `Acquiring`.
    /// Taking an [`AdmittedSourceResult`] rather than a key and a report is
    /// what stops an unadmitted platform claim reaching the reducer.
    SourceSettled(AdmittedSourceResult),
    StageProgress {
        stamp: AttemptStamp,
        transferred: ByteCount,
    },
    /// Staging read the source through and can say what it contains.
    ///
    /// Carries the digest, not just the length: `Ready` is unconstructible
    /// without one, so the reducer cannot invent a hash that verifies nothing.
    StageComplete {
        stamp: AttemptStamp,
        content: StagedContent,
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
    AttemptRetired(RetirementAck),
    StagingRetired {
        stamp: AttemptStamp,
    },
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
    /// Take hold of the document that was chosen for this card's acquisition.
    ///
    /// Issued when an offer is ACCEPTED, not at creation. It used to be minted
    /// the moment a sender existed, which asked the platform to bind a document
    /// nobody had chosen yet — the adapter claimed from an empty registry and
    /// answered `source_unreadable` every time. Opening the picker is not this:
    /// that is a frontend affordance, published as the card's `pick_source`
    /// action, and it happens before there is anything to bind.
    AcquireSource,
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
    RetireStaging {
        stamp: AttemptStamp,
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

/// One applied frontend command and the product state its application produced.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedCommand {
    pub id: CommandId,
    /// The command that owns this identity. A re-presented identity answers
    /// its disposition only for the SAME command; a different command with a
    /// reused identity is a conflict, never a plausible-looking duplicate.
    pub command: ProductCommand,
    pub state: ProductState,
}

/// How the ledger resolves a re-presented command identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerHit {
    /// Same identity, same command: the committed disposition.
    Duplicate { state: ProductState },
    /// Same identity, DIFFERENT command: the identity is owned by `applied`.
    /// The new command must be rejected typed — answering the recorded
    /// disposition would silently swallow it.
    Conflict { applied: ProductCommand },
}

/// The durable dedup ledger for frontend mutating commands.
///
/// The ledger rides inside [`TransferRecord`], so an entry becomes durable in
/// the SAME record write that commits the command's effect — they can only
/// commit or roll back together, which is what makes command application
/// exactly-once across a process death. It is bounded ([`Self::RETENTION`],
/// newest kept): the horizon only has to span command identities a frontend
/// may still re-issue after a restart, and a live frontend re-issues promptly
/// on reattach.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CommandLedger(Vec<AppliedCommand>);

impl CommandLedger {
    /// The host's retry horizon: a re-issue older than this many newer
    /// completions has been pruned and re-applies as fresh. BN3's generated
    /// command contract states this bound to hosts (entries are ~20 bytes, so
    /// the worst case is ~5 KiB per card).
    pub const RETENTION: usize = 256;

    /// How the ledger resolves `id` re-presented with `command`, if `id` was
    /// already applied.
    pub fn disposition(&self, id: CommandId, command: ProductCommand) -> Option<LedgerHit> {
        self.0
            .iter()
            .find(|applied| applied.id == id)
            .map(|applied| {
                if applied.command == command {
                    LedgerHit::Duplicate {
                        state: applied.state,
                    }
                } else {
                    LedgerHit::Conflict {
                        applied: applied.command,
                    }
                }
            })
    }

    pub(crate) fn record(&mut self, id: CommandId, command: ProductCommand, state: ProductState) {
        self.0.push(AppliedCommand { id, command, state });
        if self.0.len() > Self::RETENTION {
            let excess = self.0.len() - Self::RETENTION;
            self.0.drain(..excess);
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRecord {
    pub identity: ProductIdentity,
    pub direction: Direction,
    pub state: ProductState,
    pub quiescence: Quiescence,
    pub generation: AttemptGen,
    pub phase: Phase,
    pub bytes: ByteCount,
    pub bytes_resumed: ByteCount,
    pub outcome: Option<Outcome>,
    pub facts: Facts,
    /// Where this card's SEND source is in its acquisition, or that it needs
    /// none. Record v5's addition, and the reason v4 is not readable: a v4
    /// record has no honest value for this. A receiver decoded as
    /// `AwaitingSelection`, or a sender defaulted to `NotRequired`, would each
    /// be a card lying about what it is — which is exactly the class of default
    /// this record type refuses elsewhere.
    pub source: SourceLifecycle,
    /// Whether this endpoint minted its room or joined one. Durable because a
    /// JOINED card must not republish the invite it adopted.
    pub participation: RoomParticipation,
    /// The channel this card was frozen to at creation. Defaulted so pre-F2b
    /// records still decode.
    ///
    /// Boxed because the record is CLONED five times per published read frame
    /// (`allowed_commands` probes each command on a throwaway copy), and a
    /// channel is four strings: one pointer in the hot structure costs less
    /// than four in every probe.
    #[serde(default)]
    pub pairing: Option<Box<PairingChannel>>,
    /// The frontend create identity that authorized this card's initial
    /// record. It is written in the SAME commit that creates the card, so a
    /// retry can recover the original result after process death without
    /// allocating a second identity, room, or card.
    ///
    /// `None` is the pre-F2b shape and the non-frontend construction paths.
    #[serde(default)]
    pub create_request_id: Option<Box<CommandId>>,
    pub(crate) receipt_request: RequestId,
    /// Durable frontend-command dedup, committed atomically with each effect.
    /// Defaulted so pre-BN2 dev records still decode.
    #[serde(default)]
    pub command_ledger: CommandLedger,
}
