use envoix_product::{ProductCommand, ProductState};
use tokio::sync::oneshot;

/// The intake's prompt answer for an admitted command. Acceptance is NOT proof
/// of effect: only the [`CommandTicket`]'s completion is.
#[derive(Debug)]
pub enum CommandVerdict {
    /// Admitted for application; await the ticket for the committed completion.
    Accepted(CommandTicket),
    /// This command identity was already applied and committed; `state` is its
    /// recorded disposition. Nothing was reduced or written.
    Duplicate { state: ProductState },
    /// This command identity is owned by a DIFFERENT committed command, named
    /// by `applied`. The submission was not reduced or written; answering the
    /// recorded disposition would silently swallow it behind a plausible
    /// duplicate. Mint a fresh identity.
    Conflict { applied: ProductCommand },
}

/// How an accepted command's application actually ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandCompletion {
    /// The effect and its ledger entry crossed the durable commit barrier.
    /// `state` is the card state the application produced.
    Committed { state: ProductState },
    /// The commit barrier escalated: the effect was rolled back and is NOT
    /// durable. `state` is the visible post-escalation card state. Re-issuing
    /// the same identity later applies the command cleanly.
    CommitFailed { state: ProductState },
    /// The actor died between acceptance and completion; whether the write
    /// landed is unknown here. Re-issue with the same identity after restore:
    /// a duplicate answer proves it committed, an acceptance proves it did not.
    Interrupted,
    /// Reducing the command failed internally; nothing durable changed.
    Internal,
}

/// The awaitable completion of one accepted command.
///
/// Dropping the ticket abandons observation only — it cannot affect the
/// command, the card, or the transfer (Pillar 7).
#[derive(Debug)]
pub struct CommandTicket {
    pub(crate) completion: oneshot::Receiver<CommandCompletion>,
}

impl CommandTicket {
    /// Waits for the command's completion. Never hangs on a dead actor: a
    /// dropped sender resolves as [`CommandCompletion::Interrupted`].
    pub async fn completed(self) -> CommandCompletion {
        self.completion
            .await
            .unwrap_or(CommandCompletion::Interrupted)
    }
}

/// The actor's internal acceptance answer (pre-completion).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrontendVerdict {
    Accepted,
    Duplicate { state: ProductState },
    Conflict { applied: ProductCommand },
}
