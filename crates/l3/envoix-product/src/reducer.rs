use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, ResumeIntent, RetirementAck,
    RetirementIntent,
};
use envoix_capabilities::{AdmittedDutyResult, Duty, DutyKind, DutyProvenance};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{ByteCount, Direction};

use crate::identity::next_generation;
use crate::{
    CapabilityAction, Facts, IdentityError, IdentitySource, NewTransfer, PauseOrigin,
    ProductCommand, ProductEffect, ProductIdentity, ProductInput, ProductState, Quiescence,
    SourceDecision, StorageAction, TransferRecord, WorkerKind,
};

impl TransferRecord {
    pub fn create(
        transfer: NewTransfer,
        identities: &mut impl IdentitySource,
    ) -> Result<(Self, Vec<ProductEffect>), IdentityError> {
        let (identity, generation, receipt_request) = ProductIdentity::mint(identities)?;
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
            receipt_request,
        };
        let effects = if record.facts.source_ready {
            vec![record.start_attempt(false)]
        } else {
            Vec::new()
        };
        Ok((record, effects))
    }

    pub const fn stamp(&self) -> AttemptStamp {
        AttemptStamp {
            card: self.identity.card,
            generation: self.generation,
        }
    }

    pub fn allowed_commands(&self) -> Vec<ProductCommand> {
        if self.facts.remove_requested || matches!(self.quiescence, Quiescence::Retiring { .. }) {
            return Vec::new();
        }
        let mut commands = match self.state {
            ProductState::Preparing => vec![ProductCommand::Cancel],
            state if state.is_active() => {
                vec![ProductCommand::Pause, ProductCommand::Cancel]
            }
            ProductState::Paused(_) | ProductState::Unconfirmed => {
                vec![ProductCommand::Resume, ProductCommand::Cancel]
            }
            ProductState::Failed
                if self
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.retry == Retryability::Retryable) =>
            {
                vec![ProductCommand::Resume]
            }
            ProductState::Failed
                if self.outcome.as_ref().and_then(|outcome| outcome.recovery)
                    == Some(Recovery::RePickSource) =>
            {
                vec![ProductCommand::RePickSource]
            }
            ProductState::Cancelled => vec![ProductCommand::Resume],
            ProductState::Completed | ProductState::Failed => Vec::new(),
            ProductState::Waiting
            | ProductState::Connecting
            | ProductState::Verifying
            | ProductState::Transferring
            | ProductState::Confirming => {
                unreachable!("active states are handled by the guard")
            }
        };
        commands.push(ProductCommand::Remove);
        commands
    }

    pub fn reduce(&mut self, input: ProductInput) -> Result<Vec<ProductEffect>, IdentityError> {
        if self.facts.remove_requested
            && !matches!(
                &input,
                ProductInput::AttemptRetired(_) | ProductInput::StagingRetired { .. }
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
        Ok(Vec::new())
    }

    fn on_restore(&mut self) -> Vec<ProductEffect> {
        match self.state {
            ProductState::Confirming if self.facts.complete_sent => {
                let stamp = self.stamp();
                self.state = ProductState::Unconfirmed;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Confirming));
                self.request_attempt_retirement(RetirementIntent::Cancel);
                vec![
                    ProductEffect::RetireAttempt {
                        stamp,
                        intent: RetirementIntent::Cancel,
                    },
                    ProductEffect::StartMailboxPoll { stamp },
                ]
            }
            state if state.is_active() => {
                let stamp = self.stamp();
                self.state = ProductState::Paused(PauseOrigin::Lost);
                self.phase = Phase::Restoring;
                self.outcome = Some(outcome_for(OutcomeCode::PeerLost, Phase::Restoring));
                self.request_attempt_retirement(RetirementIntent::Cancel);
                vec![ProductEffect::RetireAttempt {
                    stamp,
                    intent: RetirementIntent::Cancel,
                }]
            }
            ProductState::Preparing if !self.facts.source_ready && !self.source_recoverable => {
                let stamp = self.stamp();
                self.state = ProductState::Failed;
                self.phase = Phase::Restoring;
                self.outcome = Some(source_failure(false, Phase::Restoring));
                self.request_staging_retirement(RetirementIntent::Cancel);
                vec![ProductEffect::RetireStaging { stamp }]
            }
            ProductState::Unconfirmed => vec![ProductEffect::StartMailboxPoll {
                stamp: self.stamp(),
            }],
            ProductState::Completed
                if self.quiescence.is_quiescent()
                    && self.direction == Direction::Receive
                    && !self.facts.proof_delivered =>
            {
                vec![self.post_receipt()]
            }
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
        if self.state != ProductState::Transferring
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
        // An Unconfirmed card is awaiting its receipt through the MAILBOX poll —
        // that is the authority now, not the abandoned in-band attempt. The ack
        // proves the attempt released its lease (Quiescent, so a resume may now
        // safely open a fresh generation), but only a Completed outcome may
        // finalize the card: a Cancelled/PeerLost/Timeout ack must NOT discard or
        // fail a send that was in fact delivered and is still pending confirmation.
        if self.state == ProductState::Unconfirmed && outcome != OutcomeCode::Completed {
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
