//! The host-side bridge between the generated command contract and L4's live
//! command vocabulary. Frontends encode `submit` bodies; the host decodes them
//! here into a typed [`SubmitSpec`] and answers with encoded acceptance and
//! completion frames. Actually invoking
//! [`submit_command`](envoix_runtime::Runtime::submit_command) is host
//! composition (BN4/F2/F3), not this crate's job.
//!
//! Every match in this module is deliberately exhaustive with no wildcard arm:
//! if the live Rust vocabulary or the generated schema gains a variant the
//! other side lacks, this module stops compiling — the loud drift signal that
//! `generated_command_schema_exhaustiveness` builds on.

use envoix_runtime::{
    CommandCompletion, CommandRejected, CommandVerdict, PauseOrigin, ProductCommand, ProductState,
    SourceOfferAnswer,
};
use envoix_types::{CommandId, Direction, RecordId};

use crate::command::{
    AcceptanceView, CommandAcceptanceView, CommandBody, CommandCompletionView, CommandError,
    CommandFrame, CommandView, CompletionView, CreateIntentView, CreateOutcomeView,
    CreateResultView, CreateView, DispositionView, FrontendIntentView, LocalDirectionView,
    PausedStateView, RejectionView, SourceAcquisitionKeyView, SourceOfferAnswerView,
    SourceOfferOutcomeView, SourceOfferRefusalView, SourceOfferResultView, SourceOfferView,
    SubmitView, decode_command_frame,
};

/// A decoded, validated submit request. The host resolves `card` to the live
/// attachment it holds, verifies `epoch` matches that attachment's own epoch,
/// and feeds `command_id` + `command` to `submit_command` — the runtime's gate
/// re-checks commander status regardless, so a raced epoch is rejected typed,
/// never trusted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubmitSpec {
    pub card: RecordId,
    pub epoch: u64,
    pub command_id: CommandId,
    pub command: ProductCommand,
}

/// A decoded, validated request that a card be created.
///
/// The invite text stays a `String` all the way to the host, which hands it to
/// the invite grammar: this layer neither parses nor inspects it, so there is
/// no second reader of an invite anywhere in the system.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateSpec {
    pub request_id: CommandId,
    pub intent: CreateIntent,
}

/// What kind of card a frontend asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateIntent {
    /// Mint a room and be on `local_direction` of it. Carries no document:
    /// a source is acquired after the card exists, under an identity the
    /// authority mints.
    MintRoom { local_direction: Direction },
    /// Join whatever this opaque text turns out to be. The invite decides the
    /// local direction, so none is stated here.
    JoinRoom { invite: String },
}

/// One decoded frontend-originated intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontendIntent {
    Command(SubmitSpec),
    Create(CreateSpec),
    /// A document offered to the acquisition that asked for it.
    SourceOffer(SourceOfferSpec),
}

/// One decoded source offer. The key is carried whole: a card match alone is
/// how a picked document could satisfy a request it was never chosen for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceOfferSpec {
    pub card: RecordId,
    pub generation: u32,
    pub request: CommandId,
    /// Untrusted provider metadata; the authority sanitizes it.
    pub display_name: String,
    /// What the provider claimed, never the transfer's total.
    pub reported_size: Option<u64>,
}

/// Why frontend bytes did not yield a [`FrontendIntent`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitDecodeError {
    /// The generated decoder rejected the frame (malformed, hostile,
    /// oversized, wrong schema, unknown variant, …).
    Frame(CommandError),
    /// A well-formed frame whose body is not `intent`. Acceptance, completion
    /// and create-result bodies flow host→frontend only; one arriving FROM a
    /// frontend is a contract violation, not a request.
    NotAnIntent,
}

/// Decodes hostile frontend bytes into a typed intent.
pub fn decode_intent(bytes: &[u8]) -> Result<FrontendIntent, SubmitDecodeError> {
    let frame = decode_command_frame(bytes).map_err(SubmitDecodeError::Frame)?;
    let CommandBody::Intent(intent) = frame.body else {
        return Err(SubmitDecodeError::NotAnIntent);
    };
    match intent {
        FrontendIntentView::Command(submit) => submit_spec(submit).map(FrontendIntent::Command),
        FrontendIntentView::Create(create) => create_spec(create).map(FrontendIntent::Create),
        FrontendIntentView::SourceOffer(offer) => {
            source_offer_spec(offer).map(FrontendIntent::SourceOffer)
        }
    }
}

fn source_offer_spec(offer: SourceOfferView) -> Result<SourceOfferSpec, SubmitDecodeError> {
    let card = u64::from_str_radix(&offer.key.card, 16).map_err(|_| {
        SubmitDecodeError::Frame(CommandError::Shape {
            context: "SourceAcquisitionKeyView.card",
        })
    })?;
    Ok(SourceOfferSpec {
        card: RecordId::new(card),
        generation: offer.key.generation,
        request: command_id(&offer.key.request, "SourceAcquisitionKeyView.request")?,
        display_name: offer.display_name,
        reported_size: offer.reported_size,
    })
}

fn submit_spec(submit: SubmitView) -> Result<SubmitSpec, SubmitDecodeError> {
    let card = u64::from_str_radix(&submit.card, 16).map_err(|_| {
        SubmitDecodeError::Frame(CommandError::Shape {
            context: "SubmitView.card",
        })
    })?;
    Ok(SubmitSpec {
        card: RecordId::new(card),
        epoch: submit.epoch,
        command_id: command_id(&submit.command_id, "SubmitView.command_id")?,
        command: live_command(submit.command),
    })
}

fn create_spec(create: CreateView) -> Result<CreateSpec, SubmitDecodeError> {
    Ok(CreateSpec {
        request_id: command_id(&create.request_id, "CreateView.request_id")?,
        intent: match create.intent {
            CreateIntentView::MintRoom(mint) => CreateIntent::MintRoom {
                local_direction: match mint.local_direction {
                    LocalDirectionView::Send => Direction::Send,
                    LocalDirectionView::Receive => Direction::Receive,
                },
            },
            // Exposed at the one boundary that must read it: the invite
            // grammar in Rust is what judges this text. It is sealed on the
            // wire and sealed again in anything the frontend can render.
            CreateIntentView::JoinRoom(join) => CreateIntent::JoinRoom {
                invite: join.invite.expose().clone(),
            },
        },
    })
}

fn command_id(text: &str, context: &'static str) -> Result<CommandId, SubmitDecodeError> {
    u128::from_str_radix(text, 16)
        .map(|value| CommandId::from_bytes(value.to_be_bytes()))
        .map_err(|_| SubmitDecodeError::Frame(CommandError::Shape { context }))
}

/// The encoded answer to one create request.
///
/// It is a RESULT, not an acceptance: `created` means the card's record is on
/// disk, because that write is what creation is. A refusal means no card was
/// made at all.
pub fn create_result_frame(request_id: CommandId, outcome: CreateOutcomeView) -> CommandFrame {
    CommandFrame {
        body: CommandBody::CreateResult(CreateResultView {
            outcome,
            request_id: hex32(request_id),
        }),
    }
}

/// The encoded acceptance answer for one submitted command, correlated by its
/// caller-minted identity. Acceptance is NOT proof of effect (Pillar 3): a
/// completion frame follows separately for accepted commands.
pub fn acceptance_frame(
    command_id: CommandId,
    acceptance: &Result<CommandVerdict, CommandRejected>,
) -> CommandFrame {
    let acceptance = match acceptance {
        Ok(CommandVerdict::Accepted(_)) => AcceptanceView::Accepted,
        Ok(CommandVerdict::Duplicate { state }) => AcceptanceView::Duplicate(state_view(*state)),
        Ok(CommandVerdict::Conflict { applied }) => {
            AcceptanceView::Conflict(command_view(*applied))
        }
        Err(rejected) => AcceptanceView::Rejected(rejection_view(*rejected)),
    };
    CommandFrame {
        body: CommandBody::Acceptance(CommandAcceptanceView {
            command_id: hex32(command_id),
            acceptance,
        }),
    }
}

/// The encoded committed-completion answer for one accepted command.
pub fn completion_frame(command_id: CommandId, completion: CommandCompletion) -> CommandFrame {
    let completion = match completion {
        CommandCompletion::Committed { state } => CompletionView::Committed(state_view(state)),
        CommandCompletion::CommitFailed { state } => {
            CompletionView::CommitFailed(state_view(state))
        }
        CommandCompletion::Interrupted => CompletionView::Interrupted,
        CommandCompletion::Internal => CompletionView::Internal,
    };
    CommandFrame {
        body: CommandBody::Completion(CommandCompletionView {
            command_id: hex32(command_id),
            completion,
        }),
    }
}

/// The live command for a decoded view (frontend→host direction).
pub fn live_command(command: CommandView) -> ProductCommand {
    match command {
        CommandView::Pause => ProductCommand::Pause,
        CommandView::Cancel => ProductCommand::Cancel,
        CommandView::Resume => ProductCommand::Resume,
        CommandView::Remove => ProductCommand::Remove,
        CommandView::RePickSource => ProductCommand::RePickSource,
    }
}

/// The view for a live command (test/round-trip direction).
pub fn command_view(command: ProductCommand) -> CommandView {
    match command {
        ProductCommand::Pause => CommandView::Pause,
        ProductCommand::Cancel => CommandView::Cancel,
        ProductCommand::Resume => CommandView::Resume,
        ProductCommand::Remove => CommandView::Remove,
        ProductCommand::RePickSource => CommandView::RePickSource,
    }
}

fn rejection_view(rejected: CommandRejected) -> RejectionView {
    match rejected {
        CommandRejected::UnknownCard => RejectionView::UnknownCard,
        CommandRejected::StaleEpoch => RejectionView::StaleEpoch,
        CommandRejected::Superseded => RejectionView::Superseded,
        CommandRejected::AtCapacity => RejectionView::AtCapacity,
        CommandRejected::RuntimeStopped => RejectionView::RuntimeStopped,
        CommandRejected::Interrupted => RejectionView::Interrupted,
        // A command's own commit failure already reports as a COMPLETION
        // (`commit_failed`), so this refusal reaches the command lane only if a
        // source-offer rejection is routed here by mistake. `internal` is the
        // honest projection: this vocabulary cannot say it, and inventing a
        // closer-sounding value would tell a frontend something untrue about
        // its command.
        CommandRejected::StorageFault | CommandRejected::Internal => RejectionView::Internal,
    }
}

/// The answer to one offered document, addressed to the acquisition it names.
///
/// The key is echoed because a frontend may hold picks for more than one card,
/// and an unaddressed answer is unusable — or worse, applied to the wrong pick.
pub fn source_offer_result_frame(
    key: SourceAcquisitionKeyView,
    outcome: SourceOfferOutcomeView,
) -> CommandFrame {
    CommandFrame {
        body: CommandBody::SourceOfferResult(SourceOfferResultView { key, outcome }),
    }
}

/// The authority's answer to an offered document, in the contract's words.
pub fn source_offer_answer_view(answer: SourceOfferAnswer) -> SourceOfferAnswerView {
    match answer {
        SourceOfferAnswer::Accepted => SourceOfferAnswerView::Accepted,
        SourceOfferAnswer::AlreadyAccepted => SourceOfferAnswerView::AlreadyAccepted,
        SourceOfferAnswer::Conflict => SourceOfferAnswerView::Conflict,
        SourceOfferAnswer::Stale => SourceOfferAnswerView::Stale,
        SourceOfferAnswer::UnknownCard => SourceOfferAnswerView::UnknownCard,
        SourceOfferAnswer::NotExpected => SourceOfferAnswerView::NotExpected,
    }
}

/// Why the authority never got as far as classifying the offer.
pub fn source_offer_refusal_view(rejected: CommandRejected) -> SourceOfferRefusalView {
    match rejected {
        // The card was not found by the RUNTIME rather than by the classifier.
        // It is still the same fact a frontend needs, so it is answered rather
        // than refused — see `unknown_card` in the answer vocabulary.
        CommandRejected::UnknownCard | CommandRejected::Internal => {
            SourceOfferRefusalView::Internal
        }
        CommandRejected::StaleEpoch | CommandRejected::Superseded => {
            SourceOfferRefusalView::StaleEpoch
        }
        CommandRejected::AtCapacity | CommandRejected::Interrupted => {
            SourceOfferRefusalView::Interrupted
        }
        CommandRejected::RuntimeStopped => SourceOfferRefusalView::RuntimeStopped,
        CommandRejected::StorageFault => SourceOfferRefusalView::StorageFault,
    }
}

fn state_view(state: ProductState) -> DispositionView {
    match state {
        ProductState::Preparing => DispositionView::Preparing,
        ProductState::Waiting => DispositionView::Waiting,
        ProductState::Connecting => DispositionView::Connecting,
        ProductState::Verifying => DispositionView::Verifying,
        ProductState::Transferring => DispositionView::Transferring,
        ProductState::Confirming => DispositionView::Confirming,
        ProductState::Paused(origin) => DispositionView::Paused(PausedStateView {
            origin: match origin {
                PauseOrigin::Local => crate::command::PauseCauseView::Local,
                PauseOrigin::Peer => crate::command::PauseCauseView::Peer,
                PauseOrigin::Lost => crate::command::PauseCauseView::Lost,
            },
        }),
        ProductState::Unconfirmed => DispositionView::Unconfirmed,
        ProductState::Completed => DispositionView::Completed,
        ProductState::Failed => DispositionView::Failed,
        ProductState::Cancelled => DispositionView::Cancelled,
    }
}

fn hex32(id: CommandId) -> String {
    let mut output = String::with_capacity(32);
    for byte in id.to_bytes() {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
