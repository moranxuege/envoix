use envoix_attempt_api::{
    AttemptEvent, AttemptEventKind, AttemptPlan, AttemptStamp, ResumeIntent, RetirementAck,
    RetirementIntent,
};
use envoix_capabilities::{
    AdmittedDutyResult, AdmittedSourceResult, Duty, DutyKind, DutyProvenance,
    SourceAcquisitionFailure, SourceAcquisitionKey, SourceReport, SourceRetention,
};
use envoix_outcomes::{Outcome, OutcomeCode, Phase, Recovery, Retryability, SafeDisplay};
use envoix_types::{ArtifactId, ByteCount, Direction, RequestId, TransferId};

use crate::identity::next_generation;
use crate::{
    AcceptedSourceOffer, CapabilityAction, Facts, IdentityError, IdentitySource, NewTransfer,
    PauseOrigin, ProductCommand, ProductEffect, ProductIdentity, ProductInput, ProductState,
    Quiescence, SelectionGate, SourceBacking, SourceLifecycle, SourceOfferAnswer, SourcePossession,
    SourceStagingPlan, StagedContent, StagingPlan, StagingWork, StorageAction, TransferContent,
    TransferRecord, WorkerKind,
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
        // A pure function of direction: a receiver needs no source, a sender is
        // born asking for one. The two states that would contradict the card's
        // own direction are unreachable from here.
        let source = SourceLifecycle::initial(transfer.direction);
        // And so is everything else about the card's opening position. The
        // caller used to state it — a name, a total, and a `SourceDecision`
        // that could claim a source was ready for a card holding none — which
        // is how a created record could contradict its own lifecycle on the
        // very first commit.
        let (state, quiescence, phase) = if source.requires_a_source() {
            (
                ProductState::Preparing,
                // Quiescent, not `Running { Staging }`: nothing is working
                // while the card waits for a person to choose a document. The
                // old value claimed a staging worker that did not exist, which
                // is why re-pick and resume had to test around it.
                Quiescence::Quiescent,
                Phase::Preparing,
            )
        } else {
            (
                ProductState::Connecting,
                Quiescence::Running {
                    worker: WorkerKind::Attempt,
                },
                Phase::Pairing,
            )
        };
        let record = Self {
            identity,
            direction: transfer.direction,
            source,
            participation: transfer.participation,
            state,
            quiescence,
            generation,
            phase,
            bytes: ByteCount::new(0),
            bytes_resumed: ByteCount::new(0),
            outcome: None,
            facts: Facts::default(),
            pairing: transfer.pairing,
            create_request_id: None,
            receipt_request,
            command_ledger: crate::CommandLedger::default(),
        };
        // A post-commit effect, so the card is durable before the first attempt
        // starts (`SF02`): identity comes before work.
        //
        // A card that needs a document raises NO effect here. Asking a person
        // to choose one is not platform work the authority commissions — it is
        // an affordance the card publishes, and read/9 carries it as the
        // `pick_source` action with the acquisition key this record derives.
        // Issuing the handle duty here instead asked the platform to bind a
        // document that did not exist yet.
        let effects = if record.source_is_ready() {
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

    /// The authoritative byte count, or zero when nothing has established one.
    ///
    /// Derived from the lifecycle rather than stored beside it: a stored copy
    /// is a second authority that can disagree, which is what the top-level
    /// field was. Zero means "not yet known" and is why every comparison here
    /// guards on it — a provider's claimed size is deliberately NOT promoted
    /// into this, so an unstaged card has no total at all.
    pub fn total(&self) -> ByteCount {
        self.source
            .content()
            .map_or(ByteCount::new(0), TransferContent::total)
    }

    /// The acquisition the platform is currently being asked to hold, if any.
    ///
    /// Returns the OFFER, not a boolean: a caller deciding whether an
    /// outstanding acquisition duty is still live has to compare provenances,
    /// and "this card is acquiring something" cannot answer that.
    pub const fn acquiring_offer(&self) -> Option<&AcceptedSourceOffer> {
        match &self.source {
            SourceLifecycle::Acquiring(offer) => Some(offer),
            SourceLifecycle::NotRequired { .. }
            | SourceLifecycle::AwaitingSelection(_)
            | SourceLifecycle::Staging { .. }
            | SourceLifecycle::Ready { .. } => None,
        }
    }

    /// Whether an attempt may start: the source is established, or none was
    /// ever needed. The single readiness authority, replacing the stored
    /// `Facts.source_ready` that could disagree with the lifecycle beside it.
    pub const fn source_is_ready(&self) -> bool {
        match &self.source {
            SourceLifecycle::NotRequired { .. } => true,
            SourceLifecycle::AwaitingSelection(_)
            | SourceLifecycle::Acquiring(_)
            | SourceLifecycle::Staging { .. } => false,
            SourceLifecycle::Ready { .. } => true,
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
            ProductInput::SourceOffered { offer } => self.on_source_offered(offer)?,
            ProductInput::SourceSettled(result) => self.on_source_settled(&result),
            ProductInput::StageProgress { stamp, transferred } => {
                self.on_stage_progress(stamp, transferred)
            }
            ProductInput::StageComplete {
                stamp,
                content,
                possession,
            } => self.on_stage_complete(stamp, content, possession),
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
            // `Preparing` joins this arm because it is now REACHABLE while
            // quiescent: a card waiting for a person to choose a document has
            // no worker at all. Before the lifecycle became the source
            // authority, every `Preparing` card claimed a staging worker, so
            // this combination could not occur and the arm did not cover it —
            // which would have left a freshly minted send impossible to cancel.
            Quiescence::Quiescent
                if matches!(
                    self.state,
                    ProductState::Paused(_) | ProductState::Unconfirmed | ProductState::Preparing
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
        // An unfinished acquisition is ABANDONED with the card, not kept. The
        // grant and the staging worker are being torn down, and nothing was
        // ever established from them — so the card returns to asking, and a
        // later re-pick has a gate that can actually accept an answer. Holding
        // `Acquiring`/`Staging` here would leave a cancelled card whose only
        // remaining command is Remove.
        if matches!(
            self.source,
            SourceLifecycle::Acquiring(_) | SourceLifecycle::Staging { .. }
        ) {
            self.source = SourceLifecycle::AwaitingSelection(SelectionGate::initial());
        }
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
        // Resume restarts an ATTEMPT, and an attempt needs a source. A card
        // that has none is not resumable at all: advancing the generation would
        // discharge the acquisition key it is waiting on, so the picker's answer
        // would arrive stale and the card would wait forever. `RePickSource` is
        // the command that moves it, and `allowed_commands` offers exactly that
        // because it asks these handlers rather than declaring a policy beside
        // them.
        if !self.source_is_ready() {
            return Ok(Vec::new());
        }
        let mut effects = self.exit_effects();
        self.generation = next_generation(self.generation)?;
        self.outcome = None;
        self.state = ProductState::Connecting;
        self.quiescence = Quiescence::Running {
            worker: WorkerKind::Attempt,
        };
        self.phase = Phase::Pairing;
        effects.push(self.start_attempt(resume));
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

    /// Binds a document to the acquisition that asked for it.
    ///
    /// The authority answers by comparing the WHOLE offer against the whole key
    /// it is currently asking for — anything else is a different acquisition,
    /// and a card match alone is how a picked document could satisfy a request
    /// it was never chosen for. An offer that is not accepted changes nothing:
    /// it is the caller's to be told about, not the record's to absorb.
    fn on_source_offered(
        &mut self,
        offer: AcceptedSourceOffer,
    ) -> Result<Vec<ProductEffect>, IdentityError> {
        if self.answer_source_offer(&offer) != SourceOfferAnswer::Accepted {
            return Ok(Vec::new());
        }
        self.source = SourceLifecycle::Acquiring(offer);
        // NOW there is something to bind, so now the duty is issued. It is
        // post-commit, so the acquisition is durable before the platform is
        // asked to hold what it names — a crash between the two re-issues the
        // same duty under the same key rather than binding a document to a card
        // that does not remember accepting it.
        Ok(vec![self.acquire_source()])
    }

    /// The acquisition an offer must name right now.
    ///
    /// DERIVED rather than stored a second time: it is this card, this
    /// generation, and the source request this record already mints for its
    /// duty. One place it can be wrong — and the same value the read projection
    /// publishes as the `pick_source` action, so what a frontend is told to
    /// answer with is what the authority will accept, by construction.
    pub fn current_acquisition(&self) -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(DutyProvenance {
            card: self.identity.card,
            generation: self.generation,
            request: self.source_request(),
        })
    }

    /// What the authority would answer this offer, given the acquisition it is
    /// currently asking for.
    pub fn answer_source_offer(&self, offer: &AcceptedSourceOffer) -> SourceOfferAnswer {
        self.source.answer_offer(&self.current_acquisition(), offer)
    }

    /// Applies the platform's answer about the acquisition it was asked for.
    ///
    /// Only `Acquiring` moves. Every other lifecycle state is inert, and that
    /// is not laziness: a duty result is an asynchronous, at-least-once
    /// observation, so a late arrival after a re-pick or a completed staging is
    /// NORMAL. Turning it into a failure would let the loser of a race
    /// overwrite the winner — the opposite of the synchronous source offer,
    /// which must always answer because a frontend is holding a resource.
    fn on_source_settled(&mut self, result: &AdmittedSourceResult) -> Vec<ProductEffect> {
        let SourceLifecycle::Acquiring(offer) = &self.source else {
            return Vec::new();
        };
        // The whole key, never the card: a result naming this card under a
        // superseded generation answers an acquisition that no longer exists.
        if !offer.key().is(&result.acquisition()) {
            return Vec::new();
        }
        let offer = offer.clone();
        match result.report() {
            SourceReport::Acquired(acquired) => {
                // The platform must have answered about THIS selection. An
                // adapter describing a different number of documents is
                // answering about something nobody offered, and binding it would
                // stage a card over documents it never accepted.
                if acquired.items().len() != offer.selection().len() {
                    self.source = SourceLifecycle::lost(offer, SourceAcquisitionFailure::Internal);
                    self.quiescence = Quiescence::Quiescent;
                    self.state = ProductState::Failed;
                    self.outcome = Some(source_failure(Phase::Preparing));
                    return Vec::new();
                }
                // The aggregate decides the plan — a selection streams only if
                // EVERY document survives a restart and can seek — while the
                // per-item answers are kept, because recovery is decided per
                // document and cannot be recomputed from a fold.
                //
                // `None` is a selection nothing this build can serve: several
                // documents have to be produced into one thing, and no archive
                // derivation exists. The intake refuses such an offer, so this
                // is unreachable today — and it fails honestly rather than
                // commissioning a plan that means something else.
                let Some(plan) = StagingPlan::for_selection(offer.selection(), acquired) else {
                    self.source = SourceLifecycle::lost(offer, SourceAcquisitionFailure::Internal);
                    self.quiescence = Quiescence::Quiescent;
                    self.state = ProductState::Failed;
                    self.outcome = Some(source_failure(Phase::Preparing));
                    return Vec::new();
                };
                let acquired = acquired.clone();
                let offer_for_work = offer.clone();
                self.source = SourceLifecycle::staging(offer, acquired, plan);
                self.quiescence = Quiescence::Running {
                    worker: WorkerKind::Staging,
                };
                // The staging worker owns the card from here, and this is what
                // starts it — post-commit, so the card is durably `Staging`
                // before anything touches the document.
                // What the worker is handed, derived from what the card just
                // committed. The artifact and the fingerprint are not stored a
                // second time: both follow from the record, so carrying them
                // here is the reducer telling the worker rather than the record
                // holding two copies that could drift.
                return vec![self.start_staging(plan, &offer_for_work, result.acquisition())];
            }
            SourceReport::Failed(failure) => {
                // The generation is NOT advanced here. Only `RePickSource`
                // advances it, and it must, because a fresh key is what stops a
                // late answer under the discharged key resurrecting this card.
                self.source = SourceLifecycle::lost(offer, *failure);
                self.quiescence = Quiescence::Quiescent;
                self.state = ProductState::Failed;
                self.outcome = Some(source_failure(Phase::Preparing));
            }
        }
        Vec::new()
    }

    /// Asks for a document again, under a fresh acquisition.
    ///
    /// The guard reads the LIFECYCLE, not the outcome's recovery hint. The hint
    /// was a second authority for "does this card need a document?", and it
    /// disagreed with the lifecycle in both directions: a card cancelled before
    /// anyone chose carried no hint and could never be restarted, while the
    /// hint could outlive the state that produced it.
    ///
    /// `Preparing` is excluded because that IS the state with an ask already
    /// outstanding — re-asking would discharge the key the picker is currently
    /// answering.
    fn on_repick_source(&mut self) -> Result<Vec<ProductEffect>, IdentityError> {
        if !self.quiescence.is_quiescent()
            || !self.source.requires_a_source()
            || self.source_is_ready()
            || self.state == ProductState::Preparing
        {
            return Ok(Vec::new());
        }
        // The gate is what makes the card selectable again, and the reason it
        // failed for is carried across so the next ask can say why it is being
        // made. Without this the card stayed in `RePickRequired` under a fresh
        // generation: the picker was opened and its answer was then refused,
        // because a lost gate accepts no offer at all.
        let gate = match &self.source {
            // Already selectable — a card that was cancelled before anyone
            // chose. Nothing failed, so the next ask is still the first one.
            SourceLifecycle::AwaitingSelection(gate) if gate.accepts_an_offer() => gate.clone(),
            SourceLifecycle::AwaitingSelection(gate) => {
                SelectionGate::selectable_again(gate.reason())
                    .expect("a gate that cannot accept an offer failed for a reason")
            }
            // Unreachable: the guard above required a source this card lacks,
            // and `on_cancel` releases an unfinished acquisition.
            SourceLifecycle::NotRequired { .. }
            | SourceLifecycle::Acquiring(_)
            | SourceLifecycle::Staging { .. }
            | SourceLifecycle::Ready { .. } => return Ok(Vec::new()),
        };
        self.generation = next_generation(self.generation)?;
        self.source = SourceLifecycle::AwaitingSelection(gate);
        self.state = ProductState::Preparing;
        // Nothing is working while the picker is open.
        self.quiescence = Quiescence::Quiescent;
        self.phase = Phase::Preparing;
        self.clear_progress();
        self.outcome = None;
        // No effect. The generation moved, so the card's derived acquisition key
        // is fresh, and the `pick_source` action the next projection publishes
        // carries it — which is RS04's missing half. A duty here would ask the
        // platform to bind a document the user has not chosen yet.
        Ok(Vec::new())
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
            // A ready source the PROVIDER still holds. The descriptor died with
            // the process, and the document may have changed while we were gone
            // — neither is knowable from the record — so the card re-acquires
            // and re-reads rather than opening an attempt that would discover
            // both on its first read, after spending a connection.
            //
            // An OWNED artifact takes the arm below instead: those bytes are
            // ours, sealed, and immutable, so re-deriving them because a process
            // died would be absurd. The store validates the seal when the
            // attempt opens it, and the send hashes what it transmits either
            // way.
            ProductState::Preparing
                if matches!(
                    &self.source,
                    SourceLifecycle::Ready {
                        backing: SourceBacking::PersistedProvider,
                        ..
                    }
                ) =>
            {
                let SourceLifecycle::Ready { offer, .. } = self.source.clone() else {
                    unreachable!("the guard matched Ready")
                };
                self.clear_progress();
                self.source = SourceLifecycle::Acquiring(offer);
                vec![self.acquire_source()]
            }
            ProductState::Preparing if self.source_is_ready() => {
                self.clear_progress();
                self.state = ProductState::Connecting;
                self.phase = Phase::Pairing;
                self.quiescence = Quiescence::Running {
                    worker: WorkerKind::Attempt,
                };
                vec![self.start_attempt(false)]
            }
            // Still waiting to be given a document, and that is not a failure:
            // nothing went wrong, the process simply died before anyone chose.
            // The old code failed the card here, because a stored boolean could
            // not tell "no source yet" from "the source is gone". No effect —
            // the ask is republished by the projection, and a restore must
            // never auto-open a picker.
            ProductState::Preparing
                if matches!(
                    &self.source,
                    SourceLifecycle::AwaitingSelection(gate) if gate.accepts_an_offer()
                ) =>
            {
                Vec::new()
            }
            // The acquisition was outstanding when the process died. Re-issue
            // the SAME duty under the SAME key: it is idempotent by provenance,
            // so this recovers a lost dispatch without inventing a result.
            ProductState::Preparing if matches!(self.source, SourceLifecycle::Acquiring(_)) => {
                vec![self.acquire_source()]
            }
            // The staging worker died mid-read, but the grant it was reading
            // through is `Persisted` — the platform said it would survive a
            // restart, and that promise is exactly what this arm consults.
            //
            // Re-issuing the ACQUIRE duty, not the staging, because the read
            // needs a handle and every handle this process held is gone. The
            // duty is the same one under the same acquisition key (the request
            // is derived, the generation unchanged), so the platform re-claims
            // the SAME document from its ownership journal and answers again —
            // or answers that the grant did not survive after all, which is the
            // only honest way to learn it. Nothing is invented on the record's
            // word alone.
            ProductState::Preparing
                if matches!(
                    &self.source,
                    SourceLifecycle::Staging { acquired, .. }
                        if acquired.retention() == SourceRetention::Persisted
                ) =>
            {
                let SourceLifecycle::Staging { offer, .. } = self.source.clone() else {
                    unreachable!("the guard matched Staging")
                };
                // The bytes counted before the crash were never established, so
                // the count goes with them. Carrying it would show progress
                // against a read that has not restarted.
                self.clear_progress();
                self.source = SourceLifecycle::Acquiring(offer);
                vec![self.acquire_source()]
            }
            // A production that was interrupted over a grant this process cannot
            // have. The INPUT is gone for good — `Process` means exactly that —
            // but the OUTPUT may not be: a seal published before the crash is
            // durable, immutable, and was produced under this commissioning.
            //
            // So the bulk state is asked FIRST, by re-commissioning the same
            // work. The worker adopts a matching seal without touching the
            // source at all, and answers `Failed` when there is nothing to
            // adopt — which is a re-pick, the same place asking the platform
            // would have arrived, minus throwing away an artifact the card
            // already owns.
            ProductState::Preparing
                if matches!(
                    &self.source,
                    SourceLifecycle::Staging {
                        plan: StagingPlan::ProduceOwnedArtifact { .. },
                        ..
                    }
                ) =>
            {
                let SourceLifecycle::Staging { offer, plan, .. } = self.source.clone() else {
                    unreachable!("the guard matched Staging")
                };
                // Whatever was counted before the crash was never established.
                self.clear_progress();
                self.quiescence = Quiescence::Running {
                    worker: WorkerKind::Staging,
                };
                let acquisition = *offer.key();
                vec![self.start_staging(plan, &offer, acquisition)]
            }
            // A document was held and the process died holding it, and nothing
            // above could recover it: a `Process` grant on a plan that produced
            // nothing to adopt. Asking the platform a question its answer
            // already settled would spend a round trip to be told what the
            // record says.
            ProductState::Preparing => {
                self.state = ProductState::Failed;
                self.phase = Phase::Restoring;
                self.outcome = Some(source_failure(Phase::Restoring));
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
            || (self.total().get() != 0 && transferred.get() > self.total().get())
        {
            return Vec::new();
        }
        self.bytes = transferred;
        Vec::new()
    }

    fn on_stage_complete(
        &mut self,
        stamp: AttemptStamp,
        content: StagedContent,
        possession: SourcePossession,
    ) -> Vec<ProductEffect> {
        if !self.is_current(stamp)
            || self.state != ProductState::Preparing
            || self.quiescence
                != (Quiescence::Running {
                    worker: WorkerKind::Staging,
                })
        {
            return Vec::new();
        }
        // Only a card that WAS staging can finish staging. Before the lifecycle
        // was the authority this arm did not exist, and completion could
        // manufacture a ready source for a card that had never held a document.
        let SourceLifecycle::Staging {
            offer,
            acquired,
            plan,
        } = self.source.clone()
        else {
            return Vec::new();
        };
        // The worker must have PERFORMED the plan it was given. The backing was
        // once derived from the plan alone, which meant a worker that only read a
        // copy plan's source through still produced `Ready { OwnedArtifact }` —
        // a card claiming possession of an artifact nobody had written, which a
        // restart would then try to reopen.
        //
        // This turns "this host cannot perform that plan" into an honest failure
        // instead of a lie on the record. It is NOT proof that a copy happened:
        // `ArtifactId::from_bytes` is public, so a dishonest worker can still
        // name an artifact it never wrote, and the id is not retained on the
        // backing to check later. Both want a witness only the bulk store can
        // mint; neither is reachable while no worker emits `Copied` at all.
        if !possession.performs(plan) {
            return self.fail_staging(stamp);
        }
        // A witness has to describe THIS card's commissioned work. It cannot be
        // forged — only the blob store mints one — but a genuine seal for
        // something else is still the wrong bytes, and the reducer is the only
        // place that knows what was asked for.
        //
        // The artifact identity is the sharp one: staging vouches for what the
        // attempt will open, and the attempt opens the card's own minted
        // artifact. A seal naming a different one would have staging vouch for
        // X while the attempt reads Y — which nothing downstream could detect,
        // because both are real artifacts.
        if let SourcePossession::Derived(sealed) = possession {
            let StagingPlan::ProduceOwnedArtifact { derivation } = plan else {
                return self.fail_staging(stamp);
            };
            let seal = sealed.fact();
            let describes_this_work = seal.blob.card() == self.identity.card
                && seal.blob.artifact() == self.identity.artifact
                && seal.blob.work()
                    == envoix_blob_api::BlobWorkId::of_derivation(
                        offer.key().generation(),
                        self.identity.artifact,
                    )
                && seal.fingerprint == derivation.fingerprint(&offer);
            // And the content the card is about to rest on is the SEAL's, not a
            // second account of the same bytes: one value, so there is nothing
            // for the two to disagree about.
            let describes_these_bytes =
                seal.length == content.total() && seal.digest == content.content_hash();
            if !describes_this_work || !describes_these_bytes {
                return self.fail_staging(stamp);
            }
        }
        // Staging counted the bytes and can say which ones; that is what makes
        // the card's content authoritative, so it is recorded where the
        // lifecycle keeps it and nowhere else.
        let total = content.total();
        // Contradictory evidence, not something to clamp into `Ready`: durable
        // progress above the final total means the two observations cannot both
        // be true, and a record whose own invariants cannot explain it must not
        // be authored. Staging failed instead.
        if self.bytes.get() > total.get() {
            return self.fail_staging(stamp);
        }
        self.source = SourceLifecycle::Ready {
            offer,
            acquired,
            backing: possession.backing(),
            content,
        };
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
        self.fail_staging(stamp)
    }

    /// The card held a document and staging could not read it through.
    ///
    /// The lifecycle goes back to awaiting a selection under a reason no
    /// acquisition can produce, so the card is re-pickable and says why. The
    /// generation does not move here — `RePickSource` owns that, and moving it
    /// early would discharge the key before anything asked for a new document.
    fn fail_staging(&mut self, stamp: AttemptStamp) -> Vec<ProductEffect> {
        if let SourceLifecycle::Staging { offer, .. } = self.source.clone() {
            self.source = SourceLifecycle::staging_failed(offer);
        }
        self.state = ProductState::Failed;
        self.outcome = Some(source_failure(Phase::Preparing));
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
            || (self.total().get() != 0 && transferred.get() > self.total().get())
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
        if result.duty() == self.receipt_duty() && result.outcome() == Some(OutcomeCode::Completed)
        {
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
            // The attempt could not send from the source the record vouches for
            // — it changed under the sender, or it stopped being readable. The
            // LIFECYCLE has to move, not just the state: `source_failure` offers
            // `RePickSource`, and `RePickSource` is refused while the source is
            // still `Ready`, so failing without invalidating it advertises a
            // recovery the command guard then denies.
            //
            // Only a send has a source to invalidate. A receive that reports this
            // is describing the peer's problem, and there is nothing here to
            // re-pick.
            OutcomeCode::SourceUnreadable if self.direction == Direction::Send => {
                if let SourceLifecycle::Acquiring(offer)
                | SourceLifecycle::Staging { offer, .. }
                | SourceLifecycle::Ready { offer, .. } = self.source.clone()
                {
                    self.source = SourceLifecycle::staging_failed(offer);
                }
                self.state = ProductState::Failed;
                self.outcome = Some(source_failure(self.phase));
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
                if self.state == ProductState::Preparing && self.source_is_ready() =>
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
        self.bytes = self.total();
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

    /// Commissions the staging worker for what the card has committed.
    ///
    /// One place, because restore commissions the same work a fresh acquisition
    /// does — and a second construction of it could drift in the fingerprint,
    /// which is the value that decides whether a partial artifact is eligible.
    fn start_staging(
        &self,
        plan: StagingPlan,
        offer: &AcceptedSourceOffer,
        acquisition: SourceAcquisitionKey,
    ) -> ProductEffect {
        let work = match plan {
            StagingPlan::ProviderStream { item } => StagingWork::Stream { item },
            StagingPlan::ProduceOwnedArtifact { derivation } => StagingWork::Produce {
                artifact: self.identity.artifact,
                derivation,
                fingerprint: derivation.fingerprint(offer),
            },
        };
        ProductEffect::StartSourceStaging {
            plan: SourceStagingPlan {
                stamp: self.stamp(),
                acquisition,
                work,
            },
        }
    }

    fn acquire_source(&self) -> ProductEffect {
        ProductEffect::CapabilityDuty {
            duty: self.source_duty(),
            action: CapabilityAction::AcquireSource,
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

/// The one outcome a source failure can have.
///
/// It used to have two, chosen by a stored `source_recoverable` boolean. Once
/// readiness is derived from the lifecycle the retry-without-the-user arm is
/// unreachable, and it was never honest anyway: every source failure leaves the
/// card in `RePickRequired`, which by construction cannot accept an offer under
/// the discharged key. A card that said "retry later" had no retry that worked.
fn source_failure(phase: Phase) -> Outcome {
    Outcome::new(
        OutcomeCode::SourceUnreadable,
        phase,
        Retryability::NeedsUser,
        SafeDisplay::new("Source must be selected again"),
    )
    .with_recovery(Recovery::RePickSource)
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
