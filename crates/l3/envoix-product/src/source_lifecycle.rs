//! Where a card's send source is in its acquisition, as durable product state.
//!
//! Step 1 of the reconciled source-lifecycle design. Nothing about a document
//! crosses at create time any more: a sender is born needing a source, the
//! authority publishes that it does, and acquisition happens afterwards under
//! an identity the authority minted ([`SourceAcquisitionKey`]).
//!
//! The states are few. What earns the length here is which combinations are
//! made UNCONSTRUCTIBLE, because the defects this replaces were all of that
//! shape: a `completed` that could not distinguish durable ownership from
//! process-lifetime readability, and a picked document that belonged to no card
//! at all.
//!
//! "Not representable" is enforced, not asserted. The payload-bearing variants
//! are `#[non_exhaustive]`, so another crate can MATCH them but cannot BUILD
//! one — construction goes through the checked constructors here. An earlier
//! version made this claim in prose while every variant was publicly
//! constructible, which is a comment that lies.
//!
//! A hostile storage editor can still write invalid bytes. What stops those
//! becoming live values is the record decoder converting through a DTO rather
//! than deserializing these types directly; that lands with record v5.

use envoix_capabilities::SourceAcquisitionKey;
use envoix_protocol::ContentHash;
use envoix_types::{ByteCount, Direction, OfferedName};

/// Why the authority is asking for a source. Carried so a frontend can say
/// something true without inferring it, and so a repeat is distinguishable
/// from a first ask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePromptReason {
    /// The card was just created and has never held a source.
    Initial,
    /// The platform could not read what was chosen.
    Unreadable,
    /// A grant that existed has gone — revoked, or the document moved.
    PermissionLost,
    /// Storage refused. Distinct from unreadable: the document was fine.
    StorageFault,
    /// The source was held, and reading it through failed. Distinct from
    /// `Unreadable`/`StorageFault` on purpose: those say ACQUISITION failed,
    /// this says acquisition succeeded and STAGING did not, and a frontend
    /// telling a user which happened needs them apart.
    StagingFailed,
    /// The platform failed in a way it could not classify.
    Internal,
}

impl SourcePromptReason {
    /// Whether this describes a FAILURE rather than a first ask. Keeps
    /// `Initial` out of the post-failure gates, where it would be a lie.
    const fn is_failure(self) -> bool {
        !matches!(self, Self::Initial)
    }
}

/// How long the PLATFORM's hold on the document lasts, exactly as the source
/// duty reported it.
///
/// The reason `duty/2` exists. A source duty used to answer `completed`, which
/// said nothing about surviving a restart — so a card could believe it owned a
/// document it would lose.
///
/// **Never rewritten.** An earlier version of this model promoted `Process` to
/// `Persisted` once bytes were copied, which quietly changed the meaning of an
/// admitted duty result: an exact replay of the original
/// `source_acquired(Process)` would then look like a conflict with state that
/// had moved underneath it. What owns the bytes now is [`SourceBacking`], a
/// different question with a different answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceRetention {
    /// Readable in THIS platform process only. Honest and usable: the transfer
    /// can proceed now, and a restart returns the card to awaiting selection.
    Process,
    /// The platform can reopen this after a restart.
    Persisted,
}

/// A document the user chose, bound to exactly one acquisition.
///
/// No URI, path or descriptor: the platform registry owns those under the key,
/// and product state has no scalar that could carry one by mistake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSourceOffer {
    key: SourceAcquisitionKey,
    /// The name as the authority normalized it. Retained because idempotency is
    /// classified over the WHOLE accepted offer: a retry carrying the same key
    /// and a different name was never committed, and answering "already
    /// accepted" would tell the frontend its payload was applied when it was
    /// not.
    display_name: OfferedName,
    /// What the provider SAID the size was — deliberately not the transfer's
    /// total. A provider is untrusted about length; the authoritative total
    /// comes from staging, which counted the bytes.
    reported_size: Option<ByteCount>,
}

impl AcceptedSourceOffer {
    pub const fn new(
        key: SourceAcquisitionKey,
        display_name: OfferedName,
        reported_size: Option<ByteCount>,
    ) -> Self {
        Self {
            key,
            display_name,
            reported_size,
        }
    }

    pub const fn display_name(&self) -> &OfferedName {
        &self.display_name
    }

    /// Whether `candidate` is this same offer in every accepted field.
    ///
    /// Spelled out because "exact" degrading to "same key" is precisely the
    /// defect this fixes: two offers can name one acquisition and still not be
    /// the same offer.
    pub fn is_the_same_offer_as(&self, candidate: &Self) -> bool {
        self == candidate
    }

    pub const fn key(&self) -> &SourceAcquisitionKey {
        &self.key
    }

    pub const fn reported_size(&self) -> Option<ByteCount> {
        self.reported_size
    }
}

/// How the bytes will be read, chosen when staging begins.
///
/// A copy is a different AUTHORITY over the bytes, not a stronger platform
/// grant, so it is recorded rather than inferred. Restore needs to know which
/// plan was in flight: a stream reopens the platform registry entry, a copy
/// reconciles a partial app-private artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagingPlan {
    /// Read straight from the provider. Implies the grant is persisted and the
    /// source is seekable — the two things resume requires — which is why the
    /// owner's streaming decision is expressible as a type rather than a
    /// comment.
    ProviderStream,
    /// Copy into an app-private artifact, because the grant would not survive a
    /// restart or the source cannot seek.
    CopyToOwnedArtifact,
}

/// What owns the bytes once staging has finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceBacking {
    /// The provider, reopened through its persisted grant. That grant must be
    /// retained and revalidated.
    PersistedProvider,
    /// An app-private artifact this build owns outright. The provider grant can
    /// be released.
    OwnedArtifact,
}

/// What staging established about the source — counted by us, not claimed by
/// the provider.
///
/// This exists as a separate type from [`AcceptedSourceOffer::reported_size`]
/// because they are different facts with different trust: a provider states a
/// size, and staging read the bytes. Streaming rather than copying does not
/// change that — the read still happens, it just writes nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedContent {
    name: OfferedName,
    total: ByteCount,
    /// Which bytes staging actually read.
    ///
    /// Without it "staged" means only "once observed a length": a provider can
    /// replace the document with a different one of the same name and size, and
    /// after a restart the identical record would reopen it and send it as what
    /// staging established. A provider-backed attempt verifies against this
    /// before sending.
    content_hash: ContentHash,
}

impl StagedContent {
    pub const fn new(name: OfferedName, total: ByteCount, content_hash: ContentHash) -> Self {
        Self {
            name,
            total,
            content_hash,
        }
    }

    pub const fn content_hash(&self) -> ContentHash {
        self.content_hash
    }

    pub const fn name(&self) -> &OfferedName {
        &self.name
    }

    pub const fn total(&self) -> ByteCount {
        self.total
    }
}

/// Whether an offer under the CURRENT generation may still be the first
/// accepted one.
///
/// A boolean (`can_accept_offer`) would carry the same bit and be a worse
/// type: the failing case must also say which offer was lost and why, and a
/// bare boolean invites being read as "retry allowed" by a caller that has not
/// advanced the generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionGate {
    /// No offer has been accepted under this generation yet.
    ///
    /// `non_exhaustive` so this can be MATCHED anywhere but CONSTRUCTED only in
    /// this crate, through the checked constructors below. Without it,
    /// `RePickRequired { reason: Initial, .. }` compiled in any importing
    /// crate — which made this module's "unconstructible" claim aspirational
    /// rather than true. Rust has no per-field visibility on enum variants, so
    /// this is the mechanism that makes the claim real.
    #[non_exhaustive]
    Selectable { reason: SourcePromptReason },
    /// This generation accepted an offer and then lost it. Only a re-pick —
    /// which advances the generation and mints a new key — makes the card
    /// selectable again, so a late offer under the discharged key cannot
    /// resurrect it.
    #[non_exhaustive]
    RePickRequired {
        reason: SourcePromptReason,
        previous_offer: AcceptedSourceOffer,
    },
}

impl SelectionGate {
    /// The first ask for a freshly created sender.
    pub const fn initial() -> Self {
        Self::Selectable {
            reason: SourcePromptReason::Initial,
        }
    }

    /// A fresh ask after a re-pick advanced the generation. Refuses `Initial`,
    /// which would claim a card that has failed never tried.
    pub fn selectable_again(reason: SourcePromptReason) -> Option<Self> {
        reason.is_failure().then_some(Self::Selectable { reason })
    }

    /// This generation lost the source it held.
    pub fn lost(reason: SourcePromptReason, previous_offer: AcceptedSourceOffer) -> Option<Self> {
        reason.is_failure().then_some(Self::RePickRequired {
            reason,
            previous_offer,
        })
    }

    pub const fn reason(&self) -> SourcePromptReason {
        match self {
            Self::Selectable { reason } | Self::RePickRequired { reason, .. } => *reason,
        }
    }

    /// Whether an offer under the current generation can still be accepted.
    pub const fn accepts_an_offer(&self) -> bool {
        matches!(self, Self::Selectable { .. })
    }
}

/// Where the card's send source is.
///
/// Every payload-bearing state REQUIRES what makes it meaningful, so the
/// invalid combinations are absent rather than merely untested: no `Acquiring`
/// without the offer it acquires, no `Staging` without a retention promise, no
/// `Ready` without counted content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLifecycle {
    /// This card receives. It has no source and can never acquire one.
    ///
    /// It still has CONTENT — what the peer says it is sending — and that needs
    /// a durable home once record v5 drops the top-level name and total. Here
    /// keeps "what is this card transferring?" in one place per direction.
    ///
    /// `None` until the peer's header is admitted, which is honest: a card
    /// joined from an invite knows nothing about the file until the sender
    /// says so, and inventing a placeholder is what made the old empty
    /// `offered_name` ambiguous.
    #[non_exhaustive]
    NotRequired { peer_content: Option<StagedContent> },
    /// The authority is asking for a document.
    AwaitingSelection(SelectionGate),
    /// A document was chosen; the platform is being asked to hold it.
    Acquiring(AcceptedSourceOffer),
    /// The platform holds it and the authority is establishing what it
    /// actually contains.
    ///
    /// Usually that means READING the source through once to count it, not
    /// copying it: the owner's decision is that a send streams from the
    /// provider, because copying every document would double disk for a
    /// multi-gigabyte file on a phone. A copy happens only when it must —
    /// a `Process` grant that a restart would lose, or a source that cannot
    /// seek, which resume requires.
    ///
    /// Copying is therefore how `Process` becomes `Persisted`, which is why no
    /// separate "did we copy?" field is needed: an app-private copy is
    /// re-openable by definition, so the retention it leaves is `Persisted`.
    #[non_exhaustive]
    Staging {
        offer: AcceptedSourceOffer,
        /// The duty's answer, frozen. See [`SourceRetention`].
        acquired_retention: SourceRetention,
        plan: StagingPlan,
    },
    /// The content is established and the source can be sent.
    ///
    /// `retention` is what a restart consults: `Persisted` can be reopened,
    /// `Process` cannot and returns the card to awaiting selection however
    /// complete it looked.
    #[non_exhaustive]
    Ready {
        offer: AcceptedSourceOffer,
        /// The duty's answer, frozen. See [`SourceRetention`].
        acquired_retention: SourceRetention,
        backing: SourceBacking,
        content: StagedContent,
    },
}

impl SourceLifecycle {
    /// The state a newly created card starts in — a pure function of its
    /// direction. The direction/source invariant lives here: a receiver cannot
    /// be born awaiting a source, a sender cannot be born not needing one.
    pub const fn initial(direction: Direction) -> Self {
        match direction {
            Direction::Receive => Self::NotRequired { peer_content: None },
            Direction::Send => Self::AwaitingSelection(SelectionGate::initial()),
        }
    }

    pub const fn requires_a_source(&self) -> bool {
        !matches!(self, Self::NotRequired { .. })
    }

    /// The acquisition this state is bound to, if any. Inputs match against the
    /// WHOLE key; "the card matches" is not the question.
    pub const fn key(&self) -> Option<&SourceAcquisitionKey> {
        match self {
            Self::NotRequired { .. } | Self::AwaitingSelection(_) => None,
            Self::Acquiring(offer) | Self::Staging { offer, .. } | Self::Ready { offer, .. } => {
                Some(offer.key())
            }
        }
    }

    /// Whether an attempt may start. Only a counted, complete artifact
    /// qualifies — a held URI is not a source that can be sent.
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Whether direction and source state agree. True by construction above; a
    /// decoder reading untrusted bytes is what needs to ask.
    pub const fn agrees_with(&self, direction: Direction) -> bool {
        match direction {
            Direction::Receive => !self.requires_a_source(),
            Direction::Send => self.requires_a_source(),
        }
    }
}

/// What the authority says about an offered document.
///
/// A source offer is SYNCHRONOUS: the frontend is waiting for this, and it
/// holds a platform resource under the offered key that it must release. So
/// every refusal here is typed and terminal. That is deliberately unlike an
/// asynchronous duty or staging result, where a stale arrival is normal after a
/// generation advance and the right answer is silent inertia — turning that
/// into a failure would let the loser of a race overwrite the winner.
///
/// Silence is the one answer this must never give: it would leak the frontend's
/// held resource and invite a blind retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceOfferAnswer {
    /// Bound to this acquisition. The card advances to `Acquiring`.
    Accepted,
    /// This exact offer — every accepted field — was already accepted.
    /// Idempotent re-delivery, not an error: the frontend may have retried
    /// across a process death.
    AlreadyAccepted,
    /// The same acquisition, a DIFFERENT offer. The key was reused with
    /// metadata that was never committed, so neither "accepted" nor "stale" is
    /// true and a frontend told either would be misled about whether its
    /// payload took effect.
    Conflict,
    /// The key named a real card, but not its current acquisition: a re-pick
    /// advanced the generation, or the request is not the one outstanding. The
    /// frontend should release what it holds and wait to be asked again.
    Stale,
    /// No such card. Answered by the RECORD LOOKUP that precedes
    /// [`SourceLifecycle::answer_offer`], never by the lifecycle itself: a
    /// method called on a state that was found cannot discover that nothing
    /// was. Kept in this vocabulary because it is one of the answers a
    /// frontend receives, and it must be able to tell "there is nothing to
    /// wait for" from `Stale`'s "wait to be asked again".
    UnknownCard,
    /// The card receives, so it can never take a source. A forged or confused
    /// offer cannot make a receiver into a sender.
    NotExpected,
}

impl SourceLifecycle {
    /// Whether `offered` may be accepted, given the acquisition the authority
    /// is currently asking for and where this card's source is.
    ///
    /// `expected` is REQUIRED and is the whole point. An awaiting card holds no
    /// offer, so it has no key of its own to compare against — and a version of
    /// this that took only `offered` accepted any key at all, including another
    /// card's, which reopened the exact ownership defect
    /// [`SourceAcquisitionKey`] exists to close. The expected key is derived by
    /// the authority (`reducer.rs`'s source duty provenance) rather than stored
    /// twice.
    pub fn answer_offer(
        &self,
        expected: &SourceAcquisitionKey,
        candidate: &AcceptedSourceOffer,
    ) -> SourceOfferAnswer {
        // Every state that already holds an offer answers the same way about
        // it, so the classification lives in one place: equal in every accepted
        // field is a retry, the same key with different fields is a conflict,
        // and a different key is not this acquisition at all.
        fn against(
            accepted: &AcceptedSourceOffer,
            candidate: &AcceptedSourceOffer,
        ) -> SourceOfferAnswer {
            if accepted.is_the_same_offer_as(candidate) {
                SourceOfferAnswer::AlreadyAccepted
            } else if accepted.key().is(candidate.key()) {
                SourceOfferAnswer::Conflict
            } else {
                SourceOfferAnswer::Stale
            }
        }

        match self {
            // A receiver has no acquisition to name, so nothing can match.
            Self::NotRequired { .. } => SourceOfferAnswer::NotExpected,
            Self::AwaitingSelection(SelectionGate::Selectable { .. }) => {
                if expected.is(candidate.key()) {
                    SourceOfferAnswer::Accepted
                } else {
                    // Another card, a superseded generation, or a request that
                    // is not the outstanding one.
                    SourceOfferAnswer::Stale
                }
            }
            // This generation accepted an offer and then lost it. It cannot
            // accept a NEW one — only a re-pick reopens it — but the answer to
            // the offer it did accept must stay recoverable, or a frontend
            // retrying across a process death is told its committed offer was
            // stale.
            Self::AwaitingSelection(SelectionGate::RePickRequired { previous_offer, .. }) => {
                match against(previous_offer, candidate) {
                    SourceOfferAnswer::AlreadyAccepted => SourceOfferAnswer::AlreadyAccepted,
                    SourceOfferAnswer::Conflict => SourceOfferAnswer::Conflict,
                    _ => SourceOfferAnswer::Stale,
                }
            }
            Self::Acquiring(offer) | Self::Staging { offer, .. } | Self::Ready { offer, .. } => {
                against(offer, candidate)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use envoix_capabilities::DutyProvenance;
    use envoix_types::{AttemptGen, RecordId, RequestId};

    use super::*;

    fn key(generation: u32) -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(DutyProvenance {
            card: RecordId::new(0x51),
            generation: AttemptGen::new(generation),
            request: RequestId::from_bytes([0xab; 16]),
        })
    }

    fn staged(total: u64) -> StagedContent {
        StagedContent::new(
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            ByteCount::new(total),
            ContentHash::from_bytes([7; 32]),
        )
    }

    fn offer(generation: u32) -> AcceptedSourceOffer {
        AcceptedSourceOffer::new(
            key(generation),
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            Some(ByteCount::new(4096)),
        )
    }

    /// The direction/source invariant, in the one place that decides it. A
    /// receiver born awaiting a source, or a sender born not needing one, are
    /// the two states this makes unreachable.
    #[test]
    fn the_starting_state_is_a_function_of_direction_alone() {
        let receiving = SourceLifecycle::initial(Direction::Receive);
        assert_eq!(
            receiving,
            SourceLifecycle::NotRequired { peer_content: None }
        );
        assert!(!receiving.requires_a_source());
        assert!(receiving.agrees_with(Direction::Receive));
        assert!(!receiving.agrees_with(Direction::Send));

        let sending = SourceLifecycle::initial(Direction::Send);
        assert_eq!(
            sending,
            SourceLifecycle::AwaitingSelection(SelectionGate::initial())
        );
        assert!(sending.requires_a_source());
        assert!(sending.agrees_with(Direction::Send));
        assert!(!sending.agrees_with(Direction::Receive));
    }

    /// `Initial` means "never tried". A gate reached after a failure cannot
    /// claim it, so the constructors refuse rather than quietly accepting a
    /// reason that would mislead whoever renders it.
    #[test]
    fn a_post_failure_gate_cannot_claim_the_card_never_tried() {
        assert!(SelectionGate::selectable_again(SourcePromptReason::Initial).is_none());
        assert!(SelectionGate::lost(SourcePromptReason::Initial, offer(1)).is_none());

        for reason in [
            SourcePromptReason::Unreadable,
            SourcePromptReason::PermissionLost,
            SourcePromptReason::StorageFault,
            SourcePromptReason::Internal,
        ] {
            assert_eq!(
                SelectionGate::selectable_again(reason)
                    .expect("a failure reason is selectable again")
                    .reason(),
                reason
            );
            assert!(SelectionGate::lost(reason, offer(1)).is_some());
        }
    }

    /// The gate is what stops a late offer under a discharged key resurrecting
    /// a generation that already lost its source. Only a re-pick — which
    /// advances the generation — reopens it.
    #[test]
    fn a_generation_that_lost_its_source_accepts_no_further_offer() {
        let lost = SelectionGate::lost(SourcePromptReason::PermissionLost, offer(1))
            .expect("a failure reason");
        assert!(!lost.accepts_an_offer());

        let reopened = SelectionGate::selectable_again(SourcePromptReason::PermissionLost)
            .expect("a failure reason");
        assert!(reopened.accepts_an_offer());
        assert!(SelectionGate::initial().accepts_an_offer());
    }

    /// Every state that names an acquisition names the WHOLE key, and the
    /// states that name none say so. An input is matched against this, and a
    /// card match is not the question.
    #[test]
    fn only_bound_states_name_an_acquisition() {
        assert!(
            SourceLifecycle::NotRequired { peer_content: None }
                .key()
                .is_none()
        );
        assert!(
            SourceLifecycle::AwaitingSelection(SelectionGate::initial())
                .key()
                .is_none()
        );

        let bound = [
            SourceLifecycle::Acquiring(offer(2)),
            SourceLifecycle::Staging {
                offer: offer(2),
                acquired_retention: SourceRetention::Process,
                plan: StagingPlan::CopyToOwnedArtifact,
            },
            SourceLifecycle::Ready {
                offer: offer(2),
                acquired_retention: SourceRetention::Persisted,
                backing: SourceBacking::PersistedProvider,
                content: staged(4096),
            },
        ];
        for state in bound {
            assert_eq!(state.key(), Some(&key(2)));
            // A key from another generation is a different acquisition.
            assert_ne!(state.key(), Some(&key(3)));
        }
    }

    /// Only counted, complete content makes a card sendable. Holding the
    /// document is not the same as having staged it, which is the distinction
    /// the old `SourceDecision::Ready` could not draw for a platform source.
    #[test]
    fn holding_a_document_is_not_being_ready_to_send() {
        assert!(!SourceLifecycle::Acquiring(offer(1)).is_ready());
        assert!(
            !SourceLifecycle::Staging {
                offer: offer(1),
                acquired_retention: SourceRetention::Persisted,
                plan: StagingPlan::ProviderStream,
            }
            .is_ready()
        );
        assert!(
            SourceLifecycle::Ready {
                offer: offer(1),
                acquired_retention: SourceRetention::Persisted,
                backing: SourceBacking::OwnedArtifact,
                content: staged(1),
            }
            .is_ready()
        );
    }

    /// The two facts one word used to carry.
    ///
    /// Both of these acquired a PERSISTED grant, and they must restore
    /// differently: the streamed one reopens the provider and revalidates its
    /// grant, the copied one reopens its own artifact and may release the
    /// grant. A single `retention` field could not tell them apart, and
    /// promoting Process to Persisted after a copy would have changed the
    /// meaning of an admitted duty result under an exact replay.
    #[test]
    fn retention_records_the_duty_and_backing_records_the_bytes() {
        let streamed = SourceLifecycle::Ready {
            offer: offer(1),
            acquired_retention: SourceRetention::Persisted,
            backing: SourceBacking::PersistedProvider,
            content: staged(4096),
        };
        let copied = SourceLifecycle::Ready {
            offer: offer(1),
            // The platform only ever promised this process; copying is what
            // made the bytes durable, and the duty's answer is left alone.
            acquired_retention: SourceRetention::Process,
            backing: SourceBacking::OwnedArtifact,
            content: staged(4096),
        };
        assert_ne!(streamed, copied);
        assert!(streamed.is_ready() && copied.is_ready());

        let (SourceLifecycle::Ready { backing: a, .. }, SourceLifecycle::Ready { backing: b, .. }) =
            (&streamed, &copied)
        else {
            panic!("both are ready");
        };
        assert_ne!(a, b, "restore must be able to tell these apart");
    }

    /// Same name, same length, different bytes. Without the digest a provider
    /// could swap the document after staging measured it and the identical
    /// record would send the replacement as what staging established.
    #[test]
    fn staged_content_identifies_the_bytes_not_just_their_length() {
        let measured = staged(4096);
        let swapped = StagedContent::new(
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            ByteCount::new(4096),
            ContentHash::from_bytes([9; 32]),
        );
        assert_eq!(measured.name(), swapped.name());
        assert_eq!(measured.total(), swapped.total());
        assert_ne!(measured.content_hash(), swapped.content_hash());
        assert_ne!(measured, swapped);
    }

    /// The provider's claim about size and the counted total are different
    /// facts and are stored as different fields, so one cannot be read as the
    /// other. A provider is untrusted about length.
    #[test]
    fn a_reported_size_is_never_the_transfers_total() {
        let lying = AcceptedSourceOffer::new(
            key(1),
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            Some(ByteCount::new(10)),
        );
        let ready = SourceLifecycle::Ready {
            offer: lying.clone(),
            acquired_retention: SourceRetention::Persisted,
            backing: SourceBacking::PersistedProvider,
            content: staged(4096),
        };
        let SourceLifecycle::Ready { offer, content, .. } = &ready else {
            panic!("constructed ready");
        };
        assert_eq!(offer.reported_size(), Some(ByteCount::new(10)));
        assert_eq!(content.total(), ByteCount::new(4096));

        // A provider that reported nothing is normal and is not a failure.
        assert!(
            AcceptedSourceOffer::new(
                key(1),
                OfferedName::from_untrusted("x").expect("a bounded name"),
                None
            )
            .reported_size()
            .is_none()
        );
    }
}

#[cfg(test)]
mod offer_tests {
    use envoix_capabilities::DutyProvenance;
    use envoix_types::{AttemptGen, RecordId, RequestId};

    use super::*;

    fn key_of(card: u64, generation: u32, request: u8) -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(DutyProvenance {
            card: RecordId::new(card),
            generation: AttemptGen::new(generation),
            request: RequestId::from_bytes([request; 16]),
        })
    }

    fn offer_of(key: SourceAcquisitionKey) -> AcceptedSourceOffer {
        named_offer(key, "report.pdf")
    }

    fn named_offer(key: SourceAcquisitionKey, name: &str) -> AcceptedSourceOffer {
        AcceptedSourceOffer::new(
            key,
            OfferedName::from_untrusted(name).expect("a bounded name"),
            None,
        )
    }

    /// The ownership fix, stated as behaviour. A document offered for one
    /// acquisition must not satisfy another however similar.
    #[test]
    fn an_offer_for_another_acquisition_is_never_accepted() {
        let current = key_of(0x51, 2, 0xaa);
        let bound = SourceLifecycle::Acquiring(offer_of(current));

        assert_eq!(
            bound.answer_offer(&current, &offer_of(current)),
            SourceOfferAnswer::AlreadyAccepted
        );
        for other in [
            key_of(0x52, 2, 0xaa), // another card
            key_of(0x51, 3, 0xaa), // the same card after a re-pick
            key_of(0x51, 2, 0xab), // the same attempt, another request
        ] {
            assert_eq!(
                bound.answer_offer(&current, &offer_of(other)),
                SourceOfferAnswer::Stale
            );
        }
    }

    /// An AWAITING card holds no offer, so it has no key of its own. A version
    /// of this that compared nothing answered `Accepted` to any key at all,
    /// which reopened the ownership defect `SourceAcquisitionKey` closes.
    #[test]
    fn an_awaiting_card_accepts_only_the_acquisition_it_was_asked_for() {
        let expected = key_of(0x51, 2, 0xaa);
        let asking = SourceLifecycle::initial(Direction::Send);

        assert_eq!(
            asking.answer_offer(&expected, &offer_of(expected)),
            SourceOfferAnswer::Accepted
        );
        for other in [
            key_of(0x52, 2, 0xaa),
            key_of(0x51, 99, 0xaa),
            key_of(0x51, 2, 0xbb),
        ] {
            assert_eq!(
                asking.answer_offer(&expected, &offer_of(other)),
                SourceOfferAnswer::Stale,
                "{other:?} is not the acquisition this card was asked for"
            );
        }
    }

    /// "Exact" is not key equality. A retry carrying the same key and different
    /// metadata was NEVER committed, so answering `AlreadyAccepted` would tell
    /// the frontend its payload took effect when it did not.
    #[test]
    fn the_same_key_with_different_metadata_is_a_conflict() {
        let key = key_of(0x51, 2, 0xaa);
        let accepted = named_offer(key, "report.pdf");

        for state in [
            SourceLifecycle::Acquiring(accepted.clone()),
            SourceLifecycle::AwaitingSelection(
                SelectionGate::lost(SourcePromptReason::PermissionLost, accepted.clone())
                    .expect("a failure reason"),
            ),
        ] {
            assert_eq!(
                state.answer_offer(&key, &accepted),
                SourceOfferAnswer::AlreadyAccepted
            );
            assert_eq!(
                state.answer_offer(&key, &named_offer(key, "other.pdf")),
                SourceOfferAnswer::Conflict
            );
        }
    }

    /// The cell the first implementation got wrong. A generation that lost its
    /// source cannot accept a NEW offer, but the answer to the offer it DID
    /// accept must stay recoverable — otherwise a frontend retrying across a
    /// process death is told its committed offer was stale.
    #[test]
    fn a_lost_generation_still_recognises_the_offer_it_accepted() {
        let key = key_of(0x51, 1, 0xaa);
        let accepted = offer_of(key);
        let lost = SourceLifecycle::AwaitingSelection(
            SelectionGate::lost(SourcePromptReason::PermissionLost, accepted.clone())
                .expect("a failure reason"),
        );

        assert_eq!(
            lost.answer_offer(&key, &accepted),
            SourceOfferAnswer::AlreadyAccepted
        );
        // A different acquisition is still refused: only a re-pick reopens it.
        assert_eq!(
            lost.answer_offer(&key, &offer_of(key_of(0x51, 2, 0xaa))),
            SourceOfferAnswer::Stale
        );
    }

    /// A receiver cannot be turned into a sender by an offer, forged or
    /// confused.
    #[test]
    fn a_receiver_refuses_every_offer() {
        let receiving = SourceLifecycle::initial(Direction::Receive);
        let key = key_of(0x51, 1, 0xaa);
        assert_eq!(
            receiving.answer_offer(&key, &offer_of(key)),
            SourceOfferAnswer::NotExpected
        );
    }

    /// Every answer is terminal and distinct. A frontend holds a platform
    /// resource under the offered key while it waits, so silence would leak it
    /// and invite a blind retry — and collapsing two answers would leave the
    /// frontend unable to tell "wait to be asked again" from "there is nothing
    /// to wait for".
    #[test]
    fn the_offer_answers_are_distinct() {
        let answers = [
            SourceOfferAnswer::Accepted,
            SourceOfferAnswer::AlreadyAccepted,
            SourceOfferAnswer::Conflict,
            SourceOfferAnswer::Stale,
            SourceOfferAnswer::UnknownCard,
            SourceOfferAnswer::NotExpected,
        ];
        for (index, answer) in answers.iter().enumerate() {
            for other in &answers[index + 1..] {
                assert_ne!(answer, other);
            }
        }
    }
}
