use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, ResumeIntent, RetirementAck,
    RetirementIntent,
};
use envoix_capabilities::{AdmittedDutyResult, Duty, DutyKind, DutyProvenance};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{ArtifactId, ByteCount, Direction, RequestId, TransferId};

use crate::identity::next_generation;
use crate::{
    CapabilityAction, Facts, IdentityError, IdentitySource, NewTransfer, PauseOrigin,
    ProductCommand, ProductEffect, ProductIdentity, ProductInput, ProductState, Quiescence,
    SourceDecision, SourceLifecycle, StorageAction, TransferRecord, WorkerKind,
};

/// The domain tag that separates a card's source-duty request identity from its
/// receipt-duty one. Any non-zero tag works; this one names itself.
const SOURCE_REQUEST_DOMAIN: [u8; 16] = *b"envoix/source/v1";

/// Every command, in the order the authority publishes them: the constructive
/// affordances first, the destructive ones last. `allowed_commands` filters
/// this list, so a new [`ProductCommand`] variant must be added here or it
/// would never be offered however its handler behaves.
pub(crate) const ALL_COMMANDS: [ProductCommand; 5] = [
    ProductCommand::Pause,
    ProductCommand::Resume,
    ProductCommand::RePickSource,
    ProductCommand::Cancel,
    ProductCommand::Remove,
];

impl TransferRecord {
    pub fn create(
        transfer: NewTransfer,
        identities: &mut impl IdentitySource,
    ) -> Result<(Self, Vec<ProductEffect>), IdentityError> {
        Self::create_with_minted_identity(transfer, ProductIdentity::mint(identities)?)
    }

    pub fn create_with_identity(
        transfer: NewTransfer,
        transfer_id: TransferId,
        artifact_id: ArtifactId,
        identities: &mut impl IdentitySource,
    ) -> Result<(Self, Vec<ProductEffect>), IdentityError> {
        Self::create_with_minted_identity(
            transfer,
            ProductIdentity::adopt(identities, transfer_id, artifact_id)?,
        )
    }

    fn create_with_minted_identity(
        transfer: NewTransfer,
        (identity, generation, receipt_request): (
            ProductIdentity,
            envoix_types::AttemptGen,
            envoix_types::RequestId,
        ),
    ) -> Result<(Self, Vec<ProductEffect>), IdentityError> {
        let (state, quiescence, phase, facts, source_recoverable, outcome) = match transfer.source {
            SourceDecision::Ready => (
                ProductState::Connecting,
                Quiescence::Running {
                    worker: WorkerKind::Attempt,
                },
                Phase::Pairing,
                Facts {
                    source_ready: true,
                    ..Facts::default()
                },
                true,
                None,
            ),
            SourceDecision::Stage { recoverable } => (
                ProductState::Preparing,
                Quiescence::Running {
                    worker: WorkerKind::Staging,
                },
                Phase::Preparing,
                Facts::default(),
                recoverable,
                None,
            ),
            SourceDecision::NeedsRepick => (
                ProductState::Failed,
                Quiescence::Quiescent,
                Phase::Preparing,
                Facts::default(),
                false,
                Some(source_failure(false, Phase::Preparing)),
            ),
        };
        let record = Self {
            identity,
            direction: transfer.direction,
            // A pure function of direction: a receiver needs no source, a
            // sender is born asking for one. The two states that would
            // contradict the card's own direction are unreachable from here.
            source: SourceLifecycle::initial(transfer.direction),
            offered_name: transfer.offered_name,
            total: transfer.total,
            state,
            quiescence,
            generation,
            phase,
            bytes: ByteCount::new(0),
            bytes_resumed: ByteCount::new(0),
            outcome,
            facts,
            source_recoverable,
            pairing: transfer.pairing,
            create_request_id: None,
            receipt_request,
            command_ledger: crate::CommandLedger::default(),
        };
        // Both are post-commit effects, so the card is durable before either
        // the first attempt starts or the platform is asked for a source
        // (`SF02`): identity comes before work, including the picker.
        let effects = match (record.facts.source_ready, record.state) {
            (true, _) => vec![record.start_attempt(false)],
            (false, ProductState::Preparing) => vec![record.select_source()],
            (false, _) => Vec::new(),
        };
        Ok((record, effects))
    }

    pub const fn stamp(&self) -> AttemptStamp {
        AttemptStamp {
            card: self.identity.card,
            generation: self.generation,
        }
    }

    /// Which commands the authority will currently admit, DERIVED from the
    /// handlers instead of declared beside them.
    ///
    /// F2a publishes this list and the frontend renders it verbatim (R0), so it
    /// is a promise: an offered command must move the card, and a withheld one
    /// must be inert. A second, declarative statement of the same policy cannot
    /// be held to that promise, because it reads different facts — `on_pause`
    /// also requires a live attempt worker, `on_resume` and `on_repick_source`
    /// require quiescence, and `on_cancel` branches on the worker kind, none of
    /// which a state-only declaration can see. So the offer is computed by
    /// asking each handler, on a throwaway clone, whether it would do anything.
    /// Drift between the offer and the answer is then unrepresentable rather
    /// than merely tested for.
    ///
    /// Cost: one record clone and one reduction per command per projection.
    pub fn allowed_commands(&self) -> Vec<ProductCommand> {
        ALL_COMMANDS
            .into_iter()
            .filter(|command| self.command_would_move(*command))
            .collect()
    }

    /// Whether applying `command` would change the record or authorize an
    /// effect. A command that cannot even mint its generation moves nothing.
    fn command_would_move(&self, command: ProductCommand) -> bool {
        let mut probe = self.clone();
        match probe.reduce(ProductInput::Command(command)) {
            Ok(effects) => probe != *self || !effects.is_empty(),
            Err(_) => false,
        }
    }

    pub fn reduce(&mut self, input: ProductInput) -> Result<Vec<ProductEffect>, IdentityError> {
        if self.facts.remove_requested
            && !matches!(
                &input,
                ProductInput::Restore
                    | ProductInput::AttemptRetired(_)
                    | ProductInput::StagingRetired { .. }
                    | ProductInput::StorageFailed
            )
        {
            return Ok(Vec::new());
        }
        let effects = match input {
            ProductInput::Command(command) => self.on_command(command)?,
            ProductInput::Restore => self.on_restore(),
            ProductInput::StageProgress { stamp, transferred } => {
                self.on_stage_progress(stamp, transferred)
            }
            ProductInput::StageComplete { stamp, total } => self.on_stage_complete(stamp, total),
            ProductInput::StageFailed { stamp } => self.on_stage_failed(stamp),
            ProductInput::Advertised { stamp } => self.on_advertised(stamp),
            ProductInput::VerificationStarted { stamp } => self.on_verification_started(stamp),
            ProductInput::VerificationFinished { stamp } => self.on_verification_finished(stamp),
            ProductInput::AttemptObserved(event) => self.on_attempt_event(event.event()),
            ProductInput::AttemptRetired(ack) => self.on_attempt_retired(ack),
            ProductInput::StagingRetired { stamp } => self.on_staging_retired(stamp),
            ProductInput::AttemptEnded { stamp } => self.on_attempt_ended(stamp),
            ProductInput::ConfirmTimeout { stamp } => self.on_confirm_timeout(stamp),
            ProductInput::ReceiptVerified { stamp } => self.on_receipt_verified(stamp),
            ProductInput::ReceiptMismatch { stamp } => self.on_receipt_mismatch(stamp),
            ProductInput::ReceiptPosted(result) => self.on_receipt_posted(result),
            ProductInput::StorageFailed => self.on_storage_failed(),
        };
        Ok(effects)
    }

    fn on_command(&mut self, command: ProductCommand) -> Result<Vec<ProductEffect>, IdentityError> {
        match command {
            ProductCommand::Pause => Ok(self.on_pause()),
            ProductCommand::Cancel => Ok(self.on_cancel()),
            ProductCommand::Resume => self.on_resume(),
            ProductCommand::Remove => Ok(self.on_remove()),
            ProductCommand::RePickSource => self.on_repick_source(),
        }
    }

    fn on_pause(&mut self) -> Vec<ProductEffect> {
        if !self.state.is_active()
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Attempt,
                })
        {
            return Vec::new();
        }
        let stamp = self.stamp();
        let mut effects = self.exit_effects();
        self.state = ProductState::Paused(PauseOrigin::Local);
        self.outcome = Some(outcome_for(OutcomeCode::Paused, self.phase));
        self.quiescence = Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent: RetirementIntent::Pause,
        };
        effects.push(ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Pause,
        });
        effects
    }

    fn on_cancel(&mut self) -> Vec<ProductEffect> {
        let stamp = self.stamp();
        let mut effects = match self.quiescence {
            Quiescence::Running {
                worker: WorkerKind::Attempt,
            } if self.state.is_active() => {
                let mut effects = self.exit_effects();
                effects.push(ProductEffect::RetireAttempt {
                    stamp,
                    intent: RetirementIntent::Cancel,
                });
                effects
            }
            Quiescence::Running {
                worker: WorkerKind::Staging,
            } if self.state == ProductState::Preparing => {
                vec![ProductEffect::RetireStaging { stamp }]
            }
            Quiescence::Quiescent
                if matches!(
                    self.state,
                    ProductState::Paused(_) | ProductState::Unconfirmed
                ) =>
            {
                self.exit_effects()
            }
            Quiescence::Running { .. } | Quiescence::Retiring { .. } | Quiescence::Quiescent => {
                return Vec::new();
            }
        };
        self.state = ProductState::Cancelled;
        self.outcome = Some(outcome_for(OutcomeCode::Cancelled, self.phase));
        match self.quiescence {
            Quiescence::Running { worker } => {
                self.quiescence = Quiescence::Retiring {
                    worker,
                    intent: RetirementIntent::Cancel,
                };
            }
            Quiescence::Quiescent => {
                self.clear_progress();
                effects.push(self.discard_partial());
            }
            Quiescence::Retiring { .. } => unreachable!("retiring cancellation returned above"),
        }
        effects
    }

    fn on_resume(&mut self) -> Result<Vec<ProductEffect>, IdentityError> {
        if !self.quiescence.is_quiescent() {
            return Ok(Vec::new());
        }
        let resume = match self.state {
            ProductState::Paused(_) | ProductState::Unconfirmed => true,
            ProductState::Failed
                if self
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.retry == Retryability::Retryable) =>
            {
                true
            }
            ProductState::Cancelled => false,
            _ => return Ok(Vec::new()),
        };
        let mut effects = self.exit_effects();
        self.generation = next_generation(self.generation)?;
        self.outcome = None;
        if self.facts.source_ready {
            self.state = ProductState::Connecting;
            self.quiescence = Quiescence::Running {
                worker: WorkerKind::Attempt,
            };
            self.phase = Phase::Pairing;
            effects.push(self.start_attempt(resume));
        } else if self.source_recoverable {
            self.state = ProductState::Preparing;
            self.quiescence = Quiescence::Running {
                worker: WorkerKind::Staging,
            };
            self.phase = Phase::Preparing;
            self.clear_progress();
        } else {
            self.state = ProductState::Failed;
            self.quiescence = Quiescence::Quiescent;
            self.phase = Phase::Preparing;
            self.outcome = Some(source_failure(false, Phase::Preparing));
        }
        Ok(effects)
    }

    fn on_remove(&mut self) -> Vec<ProductEffect> {
        if matches!(self.quiescence, Quiescence::Retiring { .. }) {
            return Vec::new();
        }
        let mut effects = self.exit_effects();
        match self.quiescence {
            Quiescence::Running {
                worker: WorkerKind::Attempt,
            } => {
                self.quiescence = Quiescence::Retiring {
                    worker: WorkerKind::Attempt,
                    intent: RetirementIntent::Cancel,
                };
                effects.push(ProductEffect::RetireAttempt {
                    stamp: self.stamp(),
                    intent: RetirementIntent::Cancel,
                });
            }
            Quiescence::Running {
                worker: WorkerKind::Staging,
            } => {
                self.quiescence = Quiescence::Retiring {
                    worker: WorkerKind::Staging,
                    intent: RetirementIntent::Cancel,
                };
                effects.push(ProductEffect::RetireStaging {
                    stamp: self.stamp(),
                });
            }
            Quiescence::Quiescent => {}
            Quiescence::Retiring { .. } => unreachable!("retiring removal returned above"),
        }
        self.facts.remove_requested = true;
        if self.quiescence.is_quiescent() {
            effects.push(self.tombstone_card());
        }
        effects
    }

    fn on_repick_source(&mut self) -> Result<Vec<ProductEffect>, IdentityError> {
        if self.state != ProductState::Failed
            || !self.quiescence.is_quiescent()
            || self.outcome.as_ref().and_then(|outcome| outcome.recovery)
                != Some(Recovery::RePickSource)
        {
            return Ok(Vec::new());
        }
        self.generation = next_generation(self.generation)?;
        self.state = ProductState::Preparing;
        self.quiescence = Quiescence::Running {
            worker: WorkerKind::Staging,
        };
        self.phase = Phase::Preparing;
        self.clear_progress();
        self.outcome = None;
        self.source_recoverable = true;
        // RS04's missing half: the command that says "the source needs
        // re-picking" now actually asks for one. The generation moved, so this
        // is a fresh duty provenance rather than a re-presentation of the one
        // the failed attempt already discharged.
        Ok(vec![self.select_source()])
    }

    fn on_restore(&mut self) -> Vec<ProductEffect> {
        let previous_quiescence = self.quiescence;
        if !previous_quiescence.is_quiescent() {
            // Restore is delivered only after the process-owned worker domain has
            // been torn down. That boundary proves leases and handles are gone,
            // but says nothing about a cancel-vs-commit race.
            self.quiescence = Quiescence::Quiescent;
        }
        if self.facts.remove_requested {
            // An extant `remove_requested` record is itself evidence the tombstone
            // has not been observed to complete — a crash could have dropped the
            // post-commit `TombstoneCard`. Restore re-issues it idempotently
            // regardless of prior quiescence, so a removed card is never a zombie
            // that no command can clear. (Durable at-least-once replay across the
            // whole run is the P4 outbox's job; this closes the restore hole.)
            return vec![self.tombstone_card()];
        }

        let ambiguous_attempt_cancel = matches!(
            previous_quiescence,
            Quiescence::Retiring {
                worker: WorkerKind::Attempt,
                intent: RetirementIntent::Cancel,
            }
        ) && self.state == ProductState::Cancelled;
        // A pause requested while an attempt was live is equally ambiguous: it may
        // have LOST to a crossed commit and actually completed. Restore must not
        // affirm it as a clean local pause that then resumes as a fresh
        // generation — treat it like the cancel case.
        let ambiguous_attempt_pause = matches!(
            previous_quiescence,
            Quiescence::Retiring {
                worker: WorkerKind::Attempt,
                intent: RetirementIntent::Pause,
            }
        ) && matches!(self.state, ProductState::Paused(_));
        match self.state {
            ProductState::Completed => {
                if self.direction == Direction::Receive && !self.facts.proof_delivered {
                    vec![self.post_receipt()]
                } else {
                    Vec::new()
                }
            }
            ProductState::Cancelled
                if ambiguous_attempt_cancel
                    && self.direction == Direction::Send
                    && self.facts.complete_sent =>
            {
                self.state = ProductState::Unconfirmed;
                self.phase = Phase::Confirming;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Confirming));
                vec![ProductEffect::StartMailboxPoll {
                    stamp: self.stamp(),
                }]
            }
            ProductState::Cancelled if ambiguous_attempt_cancel => {
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.phase = Phase::Restoring;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Restoring));
                Vec::new()
            }
            ProductState::Paused(_)
                if ambiguous_attempt_pause
                    && self.direction == Direction::Send
                    && self.facts.complete_sent =>
            {
                self.state = ProductState::Unconfirmed;
                self.phase = Phase::Confirming;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Confirming));
                vec![ProductEffect::StartMailboxPoll {
                    stamp: self.stamp(),
                }]
            }
            ProductState::Paused(_) if ambiguous_attempt_pause => {
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.phase = Phase::Restoring;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Restoring));
                Vec::new()
            }
            ProductState::Confirming if self.facts.complete_sent => {
                self.state = ProductState::Unconfirmed;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Confirming));
                vec![ProductEffect::StartMailboxPoll {
                    stamp: self.stamp(),
                }]
            }
            state if state.is_active() => {
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.phase = Phase::Restoring;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Restoring));
                Vec::new()
            }
            ProductState::Preparing if self.facts.source_ready => {
                self.clear_progress();
                self.state = ProductState::Connecting;
                self.phase = Phase::Pairing;
                self.quiescence = Quiescence::Running {
                    worker: WorkerKind::Attempt,
                };
                vec![self.start_attempt(false)]
            }
            ProductState::Preparing => {
                self.state = ProductState::Failed;
                self.phase = Phase::Restoring;
                self.outcome = Some(source_failure(self.source_recoverable, Phase::Restoring));
                Vec::new()
            }
            ProductState::Unconfirmed => vec![ProductEffect::StartMailboxPoll {
                stamp: self.stamp(),
            }],
            _ => Vec::new(),
        }
    }

    fn on_stage_progress(
        &mut self,
        stamp: AttemptStamp,
        transferred: ByteCount,
    ) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || self.state != ProductState::Preparing
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Staging,
                })
            || transferred.get() < self.bytes.get()
            || (self.total.get() != 0 && transferred.get() > self.total.get())
        {
            return Vec::new();
        }
        self.bytes = transferred;
        Vec::new()
    }

    fn on_stage_complete(&mut self, stamp: AttemptStamp, total: ByteCount) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || self.state != ProductState::Preparing
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Staging,
                })
        {
            return Vec::new();
        }
        self.facts.source_ready = true;
        self.total = total;
        // The completion total is authoritative for the staged artifact, so the
        // staged byte count IS that total — clamp progress to it. Otherwise an
        // over-reported earlier `StageProgress` could leave `bytes > total`, a
        // state the record codec rejects (the reducer must never author a record
        // its own codec refuses).
        self.bytes = total;
        self.outcome = None;
        self.request_staging_retirement(RetirementIntent::Finalize);
        vec![ProductEffect::RetireStaging { stamp }]
    }

    fn on_stage_failed(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || self.state != ProductState::Preparing
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Staging,
                })
        {
            return Vec::new();
        }
        self.state = ProductState::Failed;
        self.outcome = Some(source_failure(self.source_recoverable, Phase::Preparing));
        self.request_staging_retirement(RetirementIntent::Finalize);
        vec![ProductEffect::RetireStaging { stamp }]
    }

    fn on_advertised(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if self.is_current(stamp) && self.state == ProductState::Connecting {
            self.state = ProductState::Waiting;
        }
        Vec::new()
    }

    fn on_verification_started(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if self.is_current(stamp)
            && matches!(self.state, ProductState::Waiting | ProductState::Connecting)
        {
            self.state = ProductState::Verifying;
            self.phase = Phase::Transferring;
        }
        Vec::new()
    }

    fn on_verification_finished(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if self.is_current(stamp) && self.state == ProductState::Verifying {
            self.state = ProductState::Connecting;
            self.phase = Phase::Authenticating;
        }
        Vec::new()
    }

    fn on_attempt_event(&mut self, event: AttemptEvent) -> Vec<ProductEffect> {
        if !self.is_current(event.stamp)
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Attempt,
                })
        {
            return Vec::new();
        }
        match event.kind {
            AttemptEventKind::Phase(phase) => self.on_phase(phase),
            AttemptEventKind::Progress { transferred } => self.on_progress(transferred),
            AttemptEventKind::Terminal(code) if self.state.is_active() => {
                self.classify_terminal(code)
            }
            AttemptEventKind::Terminal(_) => Vec::new(),
        }
    }

    fn on_phase(&mut self, phase: Phase) -> Vec<ProductEffect> {
        match phase {
            Phase::Pairing | Phase::Authenticating
                if matches!(self.state, ProductState::Waiting | ProductState::Connecting) =>
            {
                self.state = ProductState::Connecting;
                self.phase = phase;
                Vec::new()
            }
            Phase::Transferring
                if matches!(
                    self.state,
                    ProductState::Waiting | ProductState::Connecting | ProductState::Verifying
                ) =>
            {
                self.state = ProductState::Transferring;
                self.phase = phase;
                self.bytes_resumed = self.bytes;
                Vec::new()
            }
            Phase::Confirming
                if self.direction == Direction::Send
                    && self.state == ProductState::Transferring =>
            {
                self.state = ProductState::Confirming;
                self.phase = phase;
                self.facts.complete_sent = true;
                vec![
                    ProductEffect::StartConfirmTimer {
                        stamp: self.stamp(),
                    },
                    ProductEffect::StartMailboxPoll {
                        stamp: self.stamp(),
                    },
                ]
            }
            Phase::Preparing | Phase::Publishing | Phase::Restoring | Phase::Confirming => {
                Vec::new()
            }
            Phase::Pairing | Phase::Authenticating | Phase::Transferring => Vec::new(),
        }
    }

    fn on_progress(&mut self, transferred: ByteCount) -> Vec<ProductEffect> {
        // Progress is monotone within a generation: an untrusted executor event
        // must not move the bar (and therefore the next `ResumeFrom` offset)
        // backward, which would make a valid larger durable peer prefix look like
        // a protocol violation on resume.
        if self.state != ProductState::Transferring
            || transferred.get() < self.bytes.get()
            || (self.total.get() != 0 && transferred.get() > self.total.get())
        {
            return Vec::new();
        }
        self.bytes = transferred;
        Vec::new()
    }

    fn on_attempt_ended(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || !self.state.is_active()
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Attempt,
                })
        {
            return Vec::new();
        }
        self.classify_terminal(OutcomeCode::Internal)
    }

    fn on_confirm_timeout(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || self.state != ProductState::Confirming
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Attempt,
                })
        {
            return Vec::new();
        }
        self.state = ProductState::Unconfirmed;
        self.outcome = Some(outcome_for(OutcomeCode::Timeout, Phase::Confirming));
        // Abandon the in-band confirmation for the mailbox poll and ask the
        // attempt to retire. The card stays RETIRING until the ack: its
        // quiescence must MIRROR the executor's `quiesced` (C7 rejects opening a
        // fresh generation while this one is still live — a Quiescent-without-ack
        // here would let a resume stall on `PreviousAttemptLive`). The
        // delivered-send-not-discarded guarantee lives in `on_attempt_retired`,
        // which refuses to let a non-Completed ack overturn an Unconfirmed card.
        self.request_attempt_retirement(RetirementIntent::Cancel);
        vec![
            ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            },
            ProductEffect::StartMailboxPoll { stamp },
        ]
    }

    fn on_receipt_verified(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || !matches!(
                self.state,
                ProductState::Confirming | ProductState::Unconfirmed
            )
        {
            return Vec::new();
        }
        let was_confirming = self.state == ProductState::Confirming;
        let mut effects = self.exit_effects();
        self.complete();
        if was_confirming {
            self.request_attempt_retirement(RetirementIntent::Cancel);
            effects.push(ProductEffect::RetireAttempt {
                stamp,
                intent: RetirementIntent::Cancel,
            });
        }
        effects
    }

    fn on_receipt_mismatch(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if self.is_current(stamp)
            && matches!(
                self.state,
                ProductState::Confirming | ProductState::Unconfirmed
            )
        {
            self.facts.receipt_mismatch = true;
        }
        Vec::new()
    }

    fn on_receipt_posted(&mut self, result: AdmittedDutyResult) -> Vec<ProductEffect> {
        if result.duty() == self.receipt_duty() && result.outcome() == OutcomeCode::Completed {
            self.facts.proof_delivered = true;
        }
        Vec::new()
    }

    fn on_storage_failed(&mut self) -> Vec<ProductEffect> {
        // A card that is DURABLY at rest (Quiescent + terminal) has a settled
        // outcome and is spared. A terminal-OPTIMISTIC card that is still
        // Retiring was never durably confirmed: if the store then fails it must
        // escalate to a visible failure — this is also the path by which P2
        // surfaces a failed destructive write (it reduces StorageFailed on the
        // rolled-back, still-Retiring baseline), so it must NOT be spared here.
        if self.quiescence.is_quiescent()
            && matches!(
                self.state,
                ProductState::Completed | ProductState::Failed | ProductState::Cancelled
            )
        {
            return Vec::new();
        }
        let mut effects = self.exit_effects();
        match self.quiescence {
            Quiescence::Running {
                worker: WorkerKind::Attempt,
            } => {
                self.request_attempt_retirement(RetirementIntent::Cancel);
                effects.push(ProductEffect::RetireAttempt {
                    stamp: self.stamp(),
                    intent: RetirementIntent::Cancel,
                });
            }
            Quiescence::Running {
                worker: WorkerKind::Staging,
            } => {
                self.request_staging_retirement(RetirementIntent::Cancel);
                effects.push(ProductEffect::RetireStaging {
                    stamp: self.stamp(),
                });
            }
            Quiescence::Retiring { .. } | Quiescence::Quiescent => {}
        }
        self.state = ProductState::Failed;
        self.outcome = Some(outcome_for(OutcomeCode::StorageFault, self.phase));
        effects
    }

    fn classify_terminal(&mut self, code: OutcomeCode) -> Vec<ProductEffect> {
        let mut effects = self.exit_effects();
        let stamp = self.stamp();
        let was_confirming = self.state == ProductState::Confirming;
        match code {
            OutcomeCode::Completed => {
                self.complete();
            }
            // Local pause/cancel commands move the record before their executor
            // echo arrives. While still active, these terminal codes therefore
            // represent the peer's explicit intent.
            OutcomeCode::Paused => {
                self.state = ProductState::Paused(PauseOrigin::Peer);
                self.outcome = Some(outcome_for(code, self.phase));
            }
            OutcomeCode::Cancelled => {
                self.state = ProductState::Cancelled;
                self.outcome = Some(outcome_for(code, self.phase));
            }
            OutcomeCode::PeerLost if was_confirming => {
                self.state = ProductState::Unconfirmed;
                self.outcome = Some(outcome_for(code, self.phase));
                effects.push(ProductEffect::StartMailboxPoll {
                    stamp: self.stamp(),
                });
            }
            OutcomeCode::PeerLost if self.bytes.get() > 0 => {
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.outcome = Some(outcome_for(code, self.phase));
            }
            _ => {
                self.state = ProductState::Failed;
                self.outcome = Some(outcome_for(code, self.phase));
            }
        }
        self.request_attempt_retirement(RetirementIntent::Finalize);
        // C7 requires an explicit retirement request even after a terminal
        // observation; only its later acknowledgement proves quiescence.
        effects.push(ProductEffect::RetireAttempt {
            stamp,
            intent: RetirementIntent::Finalize,
        });
        effects
    }

    fn on_attempt_retired(&mut self, ack: RetirementAck) -> Vec<ProductEffect> {
        let stamp = ack.stamp();
        let outcome = ack.outcome();
        let Quiescence::Retiring {
            worker: WorkerKind::Attempt,
            intent,
        } = self.quiescence
        else {
            return Vec::new();
        };
        if !self.is_current(stamp) {
            return Vec::new();
        }

        self.quiescence = Quiescence::Quiescent;
        if self.facts.remove_requested {
            return vec![self.tombstone_card()];
        }
        // A storage fault that escalated AFTER this retirement was requested is
        // the authoritative terminal: the record could not be persisted. The ack
        // proves the attempt released its lease (so the card is now safely
        // Quiescent and resumable), but it must NOT re-adopt the network outcome
        // and mask the storage failure — otherwise a Cancelled/Completed ack
        // would silently overwrite the visible StorageFault.
        if self.state == ProductState::Failed
            && self.outcome.as_ref().map(|outcome| outcome.code) == Some(OutcomeCode::StorageFault)
        {
            return Vec::new();
        }
        // Completion evidence and the mailbox proof channel are monotone product
        // authorities. A non-completed transport ack only proves lease release;
        // it cannot overturn either a proven completion or a send still awaiting
        // its receipt.
        if outcome != OutcomeCode::Completed
            && matches!(
                self.state,
                ProductState::Completed | ProductState::Unconfirmed
            )
        {
            return Vec::new();
        }
        self.adopt_retired_outcome(outcome, intent)
    }

    fn on_staging_retired(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        let Quiescence::Retiring {
            worker: WorkerKind::Staging,
            intent,
        } = self.quiescence
        else {
            return Vec::new();
        };
        if !self.is_current(stamp) {
            return Vec::new();
        }

        self.quiescence = Quiescence::Quiescent;
        if self.facts.remove_requested {
            return vec![self.tombstone_card()];
        }

        match intent {
            RetirementIntent::Cancel if self.state == ProductState::Cancelled => {
                self.clear_progress();
                vec![self.discard_partial()]
            }
            RetirementIntent::Finalize
                if self.state == ProductState::Preparing && self.facts.source_ready =>
            {
                self.clear_progress();
                self.state = ProductState::Connecting;
                self.phase = Phase::Pairing;
                self.quiescence = Quiescence::Running {
                    worker: WorkerKind::Attempt,
                };
                vec![self.start_attempt(false)]
            }
            RetirementIntent::Pause | RetirementIntent::Cancel | RetirementIntent::Finalize => {
                Vec::new()
            }
        }
    }

    fn adopt_retired_outcome(
        &mut self,
        code: OutcomeCode,
        intent: RetirementIntent,
    ) -> Vec<ProductEffect> {
        match code {
            OutcomeCode::Completed => {
                self.complete();
                if self.direction == Direction::Receive {
                    vec![self.post_receipt()]
                } else {
                    Vec::new()
                }
            }
            OutcomeCode::Cancelled => {
                self.state = ProductState::Cancelled;
                self.clear_progress();
                self.outcome = Some(outcome_for(code, self.phase));
                vec![self.discard_partial()]
            }
            OutcomeCode::Paused => {
                let origin = if intent == RetirementIntent::Pause {
                    PauseOrigin::Local
                } else {
                    PauseOrigin::Peer
                };
                self.state = ProductState::Paused(origin);
                self.outcome = Some(outcome_for(code, self.phase));
                Vec::new()
            }
            OutcomeCode::PeerLost
                if self.facts.complete_sent && self.phase == Phase::Confirming =>
            {
                let already_polling = self.state == ProductState::Unconfirmed;
                self.state = ProductState::Unconfirmed;
                self.outcome = Some(outcome_for(code, self.phase));
                if already_polling {
                    Vec::new()
                } else {
                    vec![ProductEffect::StartMailboxPoll {
                        stamp: self.stamp(),
                    }]
                }
            }
            OutcomeCode::PeerLost if self.bytes.get() > 0 => {
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.outcome = Some(outcome_for(code, self.phase));
                Vec::new()
            }
            _ => {
                self.state = ProductState::Failed;
                self.outcome = Some(outcome_for(code, self.phase));
                Vec::new()
            }
        }
    }

    fn complete(&mut self) {
        self.state = ProductState::Completed;
        self.bytes = self.total;
        self.outcome = Some(outcome_for(OutcomeCode::Completed, self.phase));
    }

    fn clear_progress(&mut self) {
        self.bytes = ByteCount::new(0);
        self.bytes_resumed = ByteCount::new(0);
    }

    fn request_attempt_retirement(&mut self, intent: RetirementIntent) {
        if self.quiescence
            == (Quiescence::Running {
                worker: WorkerKind::Attempt,
            })
        {
            self.quiescence = Quiescence::Retiring {
                worker: WorkerKind::Attempt,
                intent,
            };
        }
    }

    fn request_staging_retirement(&mut self, intent: RetirementIntent) {
        if self.quiescence
            == (Quiescence::Running {
                worker: WorkerKind::Staging,
            })
        {
            self.quiescence = Quiescence::Retiring {
                worker: WorkerKind::Staging,
                intent,
            };
        }
    }

    fn start_attempt(&self, resume: bool) -> ProductEffect {
        let resume = if resume {
            ResumeIntent::ResumeFrom { offset: self.bytes }
        } else {
            ResumeIntent::Fresh
        };
        ProductEffect::StartAttempt {
            plan: AttemptPlan {
                stamp: self.stamp(),
                direction: self.direction,
                transfer: self.identity.transfer,
                artifact: self.identity.artifact,
                resume,
            },
        }
    }

    fn post_receipt(&self) -> ProductEffect {
        ProductEffect::CapabilityDuty {
            duty: self.receipt_duty(),
            action: CapabilityAction::PostReceipt,
        }
    }

    fn discard_partial(&self) -> ProductEffect {
        ProductEffect::StorageIntent {
            identity: self.identity,
            action: StorageAction::DiscardPartial,
        }
    }

    fn tombstone_card(&self) -> ProductEffect {
        ProductEffect::StorageIntent {
            identity: self.identity,
            action: StorageAction::TombstoneCard,
        }
    }

    fn select_source(&self) -> ProductEffect {
        ProductEffect::CapabilityDuty {
            duty: self.source_duty(),
            action: CapabilityAction::SelectSource,
        }
    }

    fn receipt_duty(&self) -> Duty {
        Duty {
            provenance: DutyProvenance {
                card: self.identity.card,
                generation: self.generation,
                request: self.receipt_request,
            },
            kind: DutyKind::Courier,
        }
    }

    fn source_duty(&self) -> Duty {
        Duty {
            provenance: DutyProvenance {
                card: self.identity.card,
                generation: self.generation,
                request: self.source_request(),
            },
            kind: DutyKind::SourceHandle,
        }
    }

    /// The source duty's request identity, domain-separated from the receipt's.
    ///
    /// The C6 ledger keys discharge by provenance, so two duties of one card
    /// and generation sharing a request would make the first admitted result
    /// answer for both. One minted secret with a domain tag is injective and
    /// can never collide with the untagged value, so a second minted field —
    /// and a second thing a record could be missing — is not needed.
    pub(crate) fn source_request(&self) -> RequestId {
        let mut bytes = self.receipt_request.to_bytes();
        for (byte, tag) in bytes.iter_mut().zip(SOURCE_REQUEST_DOMAIN) {
            *byte ^= tag;
        }
        RequestId::from_bytes(bytes)
    }

    fn exit_effects(&self) -> Vec<ProductEffect> {
        match self.state {
            ProductState::Confirming => vec![
                ProductEffect::StopConfirmTimer {
                    stamp: self.stamp(),
                },
                ProductEffect::StopMailboxPoll {
                    stamp: self.stamp(),
                },
            ],
            ProductState::Unconfirmed => vec![ProductEffect::StopMailboxPoll {
                stamp: self.stamp(),
            }],
            _ => Vec::new(),
        }
    }

    fn is_current(&self, stamp: AttemptStamp) -> bool {
        stamp == self.stamp()
    }
}

fn source_failure(recoverable: bool, phase: Phase) -> Outcome {
    if recoverable {
        Outcome::new(
            OutcomeCode::SourceUnreadable,
            phase,
            Retryability::Retryable,
            SafeDisplay::new("Source is temporarily unavailable"),
        )
        .with_recovery(Recovery::RetryLater)
    } else {
        Outcome::new(
            OutcomeCode::SourceUnreadable,
            phase,
            Retryability::NeedsUser,
            SafeDisplay::new("Source must be selected again"),
        )
        .with_recovery(Recovery::RePickSource)
    }
}

fn outcome_for(code: OutcomeCode, phase: Phase) -> Outcome {
    let (retry, recovery, display) = match code {
        OutcomeCode::Completed => (Retryability::Terminal, None, "Transfer completed"),
        OutcomeCode::Cancelled => (Retryability::Retryable, None, "Transfer cancelled"),
        OutcomeCode::Paused => (Retryability::Retryable, None, "Transfer paused"),
        OutcomeCode::PeerLost => (
            Retryability::Retryable,
            Some(Recovery::ReconnectPeer),
            "Connection to peer was lost",
        ),
        OutcomeCode::Timeout => (
            Retryability::Retryable,
            Some(Recovery::RetryLater),
            "Transfer timed out",
        ),
        OutcomeCode::Unauthenticated => {
            (Retryability::NeedsUser, None, "Peer authentication failed")
        }
        OutcomeCode::VersionMismatch => {
            (Retryability::Terminal, None, "Peer version is incompatible")
        }
        OutcomeCode::StorageFault => (
            Retryability::Retryable,
            Some(Recovery::RetryLater),
            "Private storage is unavailable",
        ),
        OutcomeCode::PublishFailed => (
            Retryability::Retryable,
            Some(Recovery::RetryLater),
            "Saving the received file failed",
        ),
        OutcomeCode::SourceUnreadable => (
            Retryability::NeedsUser,
            Some(Recovery::RePickSource),
            "Source must be selected again",
        ),
        OutcomeCode::NetworkUnreachable => (
            Retryability::Retryable,
            Some(Recovery::RetryLater),
            "Network is unreachable",
        ),
        OutcomeCode::Internal => (
            Retryability::Retryable,
            Some(Recovery::RetryLater),
            "Transfer ended unexpectedly",
        ),
    };
    let outcome = Outcome::new(code, phase, retry, SafeDisplay::new(display));
    match recovery {
        Some(recovery) => outcome.with_recovery(recovery),
        None => outcome,
    }
}
