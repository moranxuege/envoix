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
};
use envoix_types::{CommandId, RecordId};

use crate::command::{
    AcceptanceView, CommandAcceptanceView, CommandBody, CommandCompletionView, CommandError,
    CommandFrame, CommandView, CompletionView, DispositionView, PausedStateView, RejectionView,
    decode_command_frame,
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

/// Why frontend bytes did not yield a [`SubmitSpec`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitDecodeError {
    /// The generated decoder rejected the frame (malformed, hostile,
    /// oversized, wrong schema, unknown variant, …).
    Frame(CommandError),
    /// A well-formed frame whose body is not `submit`. Acceptance/completion
    /// bodies flow host→frontend only; one arriving FROM a frontend is a
    /// contract violation, not a command.
    NotASubmit,
}

/// Decodes hostile frontend bytes into a typed submit request.
pub fn decode_submit(bytes: &[u8]) -> Result<SubmitSpec, SubmitDecodeError> {
    let frame = decode_command_frame(bytes).map_err(SubmitDecodeError::Frame)?;
    let CommandBody::Submit(submit) = frame.body else {
        return Err(SubmitDecodeError::NotASubmit);
    };
    let card = u64::from_str_radix(&submit.card, 16).map_err(|_| {
        SubmitDecodeError::Frame(CommandError::Shape {
            context: "SubmitView.card",
        })
    })?;
    let command_id = u128::from_str_radix(&submit.command_id, 16).map_err(|_| {
        SubmitDecodeError::Frame(CommandError::Shape {
            context: "SubmitView.command_id",
        })
    })?;
    Ok(SubmitSpec {
        card: RecordId::new(card),
        epoch: submit.epoch,
        command_id: CommandId::from_bytes(command_id.to_be_bytes()),
        command: live_command(submit.command),
    })
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
        CommandRejected::Internal => RejectionView::Internal,
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
