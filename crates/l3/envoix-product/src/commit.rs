use std::fmt;
use std::num::NonZeroUsize;

use crate::{
    IdentityError, IdentitySource, NewTransfer, ProductEffect, ProductInput, ProductState,
    Quiescence, RecordCodecError, TransferRecord, WorkerKind, encode_record,
};

/// Persists one card-scoped product record body.
///
/// Retrying the same bytes after an ambiguous failure must be idempotent. P4
/// binds one implementation to the card identity and supplies the durable
/// revision discipline behind this narrow port.
pub trait RecordStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitError;

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("record commit failed")
    }
}

impl std::error::Error for CommitError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoRecordStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitFailure {
    Encode(RecordCodecError),
    Store(CommitError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStatus {
    /// The reduction changed no durable state and authorized no world effect.
    NotRequired,
    /// Durability was explicitly optional, so the barrier succeeded vacuously.
    Vacuous,
    /// The authorizing record write succeeded.
    Committed { attempts: usize },
    /// The authorizing write exhausted its bound. The original post-commit
    /// effects were dropped before visible storage-failure escalation.
    Escalated {
        attempts: usize,
        failure: CommitFailure,
        failed_state_persisted: bool,
    },
}

impl CommitStatus {
    pub const fn authorizing_commit_succeeded(self) -> bool {
        matches!(self, Self::Vacuous | Self::Committed { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyOutcome {
    pub state: ProductState,
    /// Bookkeeping and retirement effects authorized directly by the reducer.
    pub released_immediately: Vec<ProductEffect>,
    /// World-facing effects authorized only by a successful record commit.
    pub released_after_commit: Vec<ProductEffect>,
    pub commit: CommitStatus,
}

/// Synchronous product coordinator that makes record-write success the release
/// barrier for world-facing effects.
pub struct CommittedSession<S = NoRecordStore> {
    record: TransferRecord,
    store: Option<S>,
    max_commit_attempts: NonZeroUsize,
}

impl<S: RecordStore> CommittedSession<S> {
    pub fn create(
        transfer: NewTransfer,
        identities: &mut impl IdentitySource,
        store: S,
        max_commit_attempts: NonZeroUsize,
    ) -> Result<(Self, ApplyOutcome), IdentityError> {
        let (record, effects) = TransferRecord::create(transfer, identities)?;
        let mut session = Self::from_record(record, store, max_commit_attempts);
        let outcome = session.finish_reduction(effects, true, None, false)?;
        Ok((session, outcome))
    }

    /// Wraps a record that is already the last successfully committed state.
    ///
    /// On a later exhausted write, this baseline is restored before
    /// `StorageFailed` is reduced so an uncommitted terminal/removal decision
    /// cannot suppress visible escalation.
    pub fn from_record(
        record: TransferRecord,
        store: S,
        max_commit_attempts: NonZeroUsize,
    ) -> Self {
        Self {
            record,
            store: Some(store),
            max_commit_attempts,
        }
    }

    pub fn record(&self) -> &TransferRecord {
        &self.record
    }

    pub fn store(&self) -> &S {
        self.store
            .as_ref()
            .expect("a store-backed session always retains its store")
    }

    pub fn store_mut(&mut self) -> &mut S {
        self.store
            .as_mut()
            .expect("a store-backed session always retains its store")
    }

    pub fn into_parts(self) -> (TransferRecord, S) {
        (
            self.record,
            self.store
                .expect("a store-backed session always retains its store"),
        )
    }

    pub fn apply(&mut self, input: ProductInput) -> Result<ApplyOutcome, IdentityError> {
        let before = self.record.clone();
        let worker_gone_proven = !before.quiescence.is_quiescent()
            && matches!(
                &input,
                ProductInput::Restore
                    | ProductInput::AttemptRetired(_)
                    | ProductInput::StagingRetired { .. }
            );
        let effects = self.record.reduce(input)?;
        let durable_change = self.record != before;
        self.finish_reduction(effects, durable_change, Some(before), worker_gone_proven)
    }

    fn finish_reduction(
        &mut self,
        effects: Vec<ProductEffect>,
        durable_change: bool,
        committed_before: Option<TransferRecord>,
        worker_gone_proven: bool,
    ) -> Result<ApplyOutcome, IdentityError> {
        let (released_immediately, staged) = partition_effects(effects);
        if !durable_change && staged.is_empty() {
            return Ok(self.outcome(released_immediately, Vec::new(), CommitStatus::NotRequired));
        }

        let encoded = match encode_record(&self.record) {
            Ok(encoded) => encoded,
            Err(error) => {
                return self.escalate(
                    released_immediately,
                    committed_before,
                    0,
                    CommitFailure::Encode(error),
                    worker_gone_proven,
                    staged,
                );
            }
        };

        let mut last_failure = None;
        for attempt in 1..=self.max_commit_attempts.get() {
            match self
                .store
                .as_mut()
                .expect("store-backed coordinator")
                .commit(&encoded)
            {
                Ok(()) => {
                    return Ok(self.outcome(
                        released_immediately,
                        staged,
                        CommitStatus::Committed { attempts: attempt },
                    ));
                }
                Err(error) => last_failure = Some(CommitFailure::Store(error)),
            }
        }

        self.escalate(
            released_immediately,
            committed_before,
            self.max_commit_attempts.get(),
            last_failure.expect("a nonzero failed attempt bound records an error"),
            worker_gone_proven,
            staged,
        )
    }

    fn escalate(
        &mut self,
        mut released_immediately: Vec<ProductEffect>,
        committed_before: Option<TransferRecord>,
        attempts: usize,
        failure: CommitFailure,
        worker_gone_proven: bool,
        staged: Vec<ProductEffect>,
    ) -> Result<ApplyOutcome, IdentityError> {
        // The failed barrier never released this start, so C7 cannot have
        // admitted the tentative worker and there is nothing to retire.
        let attempt_start_unreleased = staged
            .iter()
            .any(|effect| matches!(effect, ProductEffect::StartAttempt { .. }));
        if attempt_start_unreleased
            && self.record.quiescence
                == (Quiescence::Running {
                    worker: WorkerKind::Attempt,
                })
        {
            self.record.quiescence = Quiescence::Quiescent;
        }
        let monotone_completion = worker_gone_proven
            && matches!(
                self.record.state,
                ProductState::Completed | ProductState::Unconfirmed
            );
        let tentative_cleanup = if monotone_completion {
            Vec::new()
        } else {
            self.record.reduce(ProductInput::StorageFailed)?
        };
        let (tentative_immediate, _dropped_post_commit) = partition_effects(tentative_cleanup);
        extend_unique(&mut released_immediately, tentative_immediate);

        if !monotone_completion && let Some(committed_before) = committed_before {
            self.record = committed_before;
            let mut escalation = self.record.reduce(ProductInput::StorageFailed)?;
            if worker_gone_proven {
                // Product decisions roll back; external lease release does not.
                self.record.quiescence = Quiescence::Quiescent;
                escalation.retain(|effect| {
                    !matches!(
                        effect,
                        ProductEffect::RetireAttempt { .. } | ProductEffect::RetireStaging { .. }
                    )
                });
            }
            let (escalation_immediate, _dropped_post_commit) = partition_effects(escalation);
            extend_unique(&mut released_immediately, escalation_immediate);
        }

        let failed_state_persisted = encode_record(&self.record).ok().is_some_and(|encoded| {
            self.store
                .as_mut()
                .expect("store-backed coordinator")
                .commit(&encoded)
                .is_ok()
        });

        // Ordinary escalation drops every staged post-commit effect (their
        // authorizing state was rolled back / replaced with a storage fault). But
        // a monotone-completion record is RETAINED unchanged, so if its
        // best-effort write succeeds that same write authorizes its post-commit
        // effect (e.g. the receive receipt) — dropping it would strand a durably
        // completed transfer with no receipt until an unrelated later restart.
        let released_after_commit = if monotone_completion && failed_state_persisted {
            staged
        } else {
            Vec::new()
        };

        Ok(self.outcome(
            released_immediately,
            released_after_commit,
            CommitStatus::Escalated {
                attempts,
                failure,
                failed_state_persisted,
            },
        ))
    }

    fn outcome(
        &self,
        released_immediately: Vec<ProductEffect>,
        released_after_commit: Vec<ProductEffect>,
        commit: CommitStatus,
    ) -> ApplyOutcome {
        ApplyOutcome {
            state: self.record.state,
            released_immediately,
            released_after_commit,
            commit,
        }
    }
}

impl CommittedSession<NoRecordStore> {
    pub fn create_without_store(
        transfer: NewTransfer,
        identities: &mut impl IdentitySource,
    ) -> Result<(Self, ApplyOutcome), IdentityError> {
        let (record, effects) = TransferRecord::create(transfer, identities)?;
        let mut session = Self::without_store(record);
        let outcome = session.finish_without_store(effects, true);
        Ok((session, outcome))
    }

    pub fn without_store(record: TransferRecord) -> Self {
        Self {
            record,
            store: None,
            max_commit_attempts: NonZeroUsize::MIN,
        }
    }

    pub fn record(&self) -> &TransferRecord {
        &self.record
    }

    pub fn apply(&mut self, input: ProductInput) -> Result<ApplyOutcome, IdentityError> {
        let before = self.record.clone();
        let effects = self.record.reduce(input)?;
        let durable_change = self.record != before;
        Ok(self.finish_without_store(effects, durable_change))
    }

    fn finish_without_store(
        &mut self,
        effects: Vec<ProductEffect>,
        durable_change: bool,
    ) -> ApplyOutcome {
        let (released_immediately, released_after_commit) = partition_effects(effects);
        let commit = if durable_change || !released_after_commit.is_empty() {
            CommitStatus::Vacuous
        } else {
            CommitStatus::NotRequired
        };
        ApplyOutcome {
            state: self.record.state,
            released_immediately,
            released_after_commit,
            commit,
        }
    }
}

fn partition_effects(effects: Vec<ProductEffect>) -> (Vec<ProductEffect>, Vec<ProductEffect>) {
    let mut immediate = Vec::new();
    let mut post_commit = Vec::new();
    for effect in effects {
        if is_post_commit(&effect) {
            post_commit.push(effect);
        } else {
            immediate.push(effect);
        }
    }
    (immediate, post_commit)
}

fn is_post_commit(effect: &ProductEffect) -> bool {
    matches!(
        effect,
        ProductEffect::StartAttempt { .. }
            | ProductEffect::CapabilityDuty { .. }
            | ProductEffect::StorageIntent { .. }
    )
}

fn extend_unique(target: &mut Vec<ProductEffect>, effects: Vec<ProductEffect>) {
    for effect in effects {
        if !target.contains(&effect) {
            target.push(effect);
        }
    }
}
