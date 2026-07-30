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

use envoix_capabilities::{
    SourceAcquisitionFailure, SourceAcquisitionKey, SourceRetention, SourceSeekability,
};
use envoix_protocol::ContentHash;
use envoix_types::{ArtifactId, ByteCount, Direction, OfferedName};

/// Why the authority is asking for a source. Carried so a frontend can say
/// something true without inferring it, and so a repeat is distinguishable
/// from a first ask.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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

impl From<SourceAcquisitionFailure> for SourcePromptReason {
    /// Every way acquisition can fail maps to a reason a user can be shown.
    ///
    /// Total in this direction and NOT in the other: `Initial` and
    /// `StagingFailed` have no acquisition failure to come from, which is the
    /// asymmetry that keeps an adapter from authoring either.
    fn from(failure: SourceAcquisitionFailure) -> Self {
        match failure {
            SourceAcquisitionFailure::Unreadable => Self::Unreadable,
            SourceAcquisitionFailure::PermissionLost => Self::PermissionLost,
            SourceAcquisitionFailure::StorageFault => Self::StorageFault,
            SourceAcquisitionFailure::Internal => Self::Internal,
        }
    }
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
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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

/// The largest source this product can carry from end to end.
///
/// DERIVED, not chosen. The generated read contract's byte counts are `u63`
/// specifically so every native target holds one in a signed 64-bit integer
/// (`schema/read.schema`), so that is the narrowest representation on the whole
/// path and therefore the real ceiling. A product limit below it would be a
/// number invented about somebody else's data; a value above it would be one
/// this product could count and then fail to publish.
///
/// In practice device storage, provider behaviour or a quota fails long before
/// this. That is the honest answer to "what is the maximum file size" — there
/// is no product-imposed one.
pub const MAX_SOURCE_BYTES: u64 = i64::MAX as u64;

impl StagingPlan {
    /// Whether this plan can be true of a source the platform holds under
    /// `retention`. See [`SourceDecodeError::ImpossibleRetention`].
    pub const fn is_possible_with(self, retention: SourceRetention) -> bool {
        match (self, retention) {
            (Self::ProviderStream, SourceRetention::Process) => false,
            (Self::ProviderStream, SourceRetention::Persisted) | (Self::CopyToOwnedArtifact, _) => {
                true
            }
        }
    }

    /// The plan for a source the platform will serve under these terms.
    ///
    /// Streaming is the default and the copy is the exception: copying every
    /// send would double disk for a multi-gigabyte file on a phone. The two
    /// facts that force a copy are the two resume depends on — a grant that a
    /// restart would lose, and a source that cannot be re-read from an offset.
    /// Both come from the platform's own answer, so this is a total function of
    /// what was reported rather than a policy that has to guess.
    pub const fn for_source(retention: SourceRetention, seekability: SourceSeekability) -> Self {
        match (retention, seekability) {
            (SourceRetention::Persisted, SourceSeekability::Seekable) => Self::ProviderStream,
            (SourceRetention::Persisted, SourceSeekability::SequentialOnly)
            | (SourceRetention::Process, SourceSeekability::Seekable)
            | (SourceRetention::Process, SourceSeekability::SequentialOnly) => {
                Self::CopyToOwnedArtifact
            }
        }
    }

    /// What owns the bytes once staging under this plan has finished.
    pub const fn backing(self) -> SourceBacking {
        match self {
            Self::ProviderStream => SourceBacking::PersistedProvider,
            Self::CopyToOwnedArtifact => SourceBacking::OwnedArtifact,
        }
    }
}

/// What owns the bytes once staging has finished.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceBacking {
    /// The provider, reopened through its persisted grant. That grant must be
    /// retained and revalidated.
    PersistedProvider,
    /// An app-private artifact this build owns outright. The provider grant can
    /// be released.
    OwnedArtifact,
}

/// What staging actually achieved, reported by the worker that did it.
///
/// The counterpart of [`StagingPlan`]: the plan is what was commissioned, this
/// is what was performed, and the reducer requires them to agree. They were once
/// the same value — the backing was derived from the plan — and that made a
/// worker's silence about what it had done indistinguishable from a copy.
///
/// [`Self::Copied`] carries the artifact rather than a flag because an
/// `ArtifactId` cannot be produced without writing one. A worker with no copy
/// sink is unable to spell it, which is why a plan it cannot perform fails
/// instead of resting at `Ready` over nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePossession {
    /// Read through where it lies. Nothing was written.
    Streamed,
    /// Copied into an artifact this app owns outright.
    Copied(ArtifactId),
}

impl SourcePossession {
    /// The backing this possession establishes.
    pub const fn backing(self) -> SourceBacking {
        match self {
            Self::Streamed => SourceBacking::PersistedProvider,
            Self::Copied(_) => SourceBacking::OwnedArtifact,
        }
    }

    /// Whether this is what `plan` commissioned.
    ///
    /// Checked rather than assumed. A worker that streamed a copy plan's source
    /// has not produced the artifact the record would claim, and a worker that
    /// copied a stream plan's source has spent disk the authority did not ask
    /// for — neither is a state the record should be able to describe.
    pub const fn performs(self, plan: StagingPlan) -> bool {
        matches!(
            (plan, self),
            (StagingPlan::ProviderStream, Self::Streamed)
                | (StagingPlan::CopyToOwnedArtifact, Self::Copied(_))
        )
    }
}

impl SourceBacking {
    /// Whether this backing can be true of a source the platform held under
    /// `retention`. An owned copy is valid whatever the platform promised —
    /// that is the point of copying — but a provider we intend to REOPEN
    /// requires a grant that survives a restart.
    pub const fn is_possible_with(self, retention: SourceRetention) -> bool {
        match (self, retention) {
            (Self::PersistedProvider, SourceRetention::Process) => false,
            (Self::PersistedProvider, SourceRetention::Persisted) | (Self::OwnedArtifact, _) => {
                true
            }
        }
    }
}

/// What a card is transferring: the name and the number of bytes.
///
/// Deliberately WITHOUT a digest. A receiver learns this from the peer's
/// header, which carries name and size and no full-file hash — that arrives
/// only with `Complete`, after the bytes. Requiring a hash here would make the
/// documented receiver state unconstructible at header admission unless the
/// reducer invented one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferContent {
    name: OfferedName,
    total: ByteCount,
}

impl TransferContent {
    pub const fn new(name: OfferedName, total: ByteCount) -> Self {
        Self { name, total }
    }

    pub const fn name(&self) -> &OfferedName {
        &self.name
    }

    pub const fn total(&self) -> ByteCount {
        self.total
    }
}

/// What staging established about the SOURCE — counted by us, not claimed by
/// the provider, and identified.
///
/// The digest separates this from [`TransferContent`]: staging read the bytes
/// and can say which ones, so a provider that swaps the document afterwards
/// cannot pass as what was staged. `reported_size` is the third, least trusted
/// fact — what the provider merely claimed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedContent {
    content: TransferContent,
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
    pub const fn new(content: TransferContent, content_hash: ContentHash) -> Self {
        Self {
            content,
            content_hash,
        }
    }

    pub const fn content(&self) -> &TransferContent {
        &self.content
    }

    pub const fn content_hash(&self) -> ContentHash {
        self.content_hash
    }

    pub const fn name(&self) -> &OfferedName {
        self.content.name()
    }

    pub const fn total(&self) -> ByteCount {
        self.content.total()
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(into = "SourceLifecycleDto", try_from = "SourceLifecycleDto")]
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
    NotRequired {
        peer_content: Option<TransferContent>,
    },
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
    /// Copying does NOT rewrite `acquired_retention` — that stays the duty's
    /// own answer. What a copy changes is the BACKING, which is why `plan`
    /// exists: an app-private copy is re-openable whatever the platform
    /// originally promised.
    #[non_exhaustive]
    Staging {
        offer: AcceptedSourceOffer,
        /// The duty's answer, frozen. See [`SourceRetention`].
        acquired_retention: SourceRetention,
        plan: StagingPlan,
    },
    /// The content is established and the source can be sent.
    ///
    /// `backing` is what a restart consults, NOT `acquired_retention`: an
    /// `OwnedArtifact` reopens its own bytes and is valid even when the
    /// platform only ever promised this process, while a `PersistedProvider`
    /// must revalidate the grant it depends on.
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

    /// The platform holds the document; establish what it contains.
    pub const fn staging(
        offer: AcceptedSourceOffer,
        acquired_retention: SourceRetention,
        plan: StagingPlan,
    ) -> Self {
        Self::Staging {
            offer,
            acquired_retention,
            plan,
        }
    }

    /// Staging held the document and could not read it through.
    ///
    /// A distinct entry point from [`Self::lost`] because the reason is one no
    /// acquisition failure can produce: acquisition SUCCEEDED here and reading
    /// did not, and a frontend telling a user which happened needs them apart.
    pub fn staging_failed(offer: AcceptedSourceOffer) -> Self {
        Self::AwaitingSelection(SelectionGate::RePickRequired {
            reason: SourcePromptReason::StagingFailed,
            previous_offer: offer,
        })
    }

    /// The acquisition failed, so this generation has lost the document it
    /// accepted and only a re-pick reopens the card.
    ///
    /// Total, unlike [`SelectionGate::lost`]: an ACQUISITION failure is always
    /// a failure reason, so there is no `None` arm here for a caller to unwrap
    /// or explain. That is the whole reason the platform's failure vocabulary
    /// is a separate, smaller type.
    pub fn lost(offer: AcceptedSourceOffer, failure: SourceAcquisitionFailure) -> Self {
        Self::AwaitingSelection(SelectionGate::RePickRequired {
            reason: failure.into(),
            previous_offer: offer,
        })
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

    /// What this card is transferring, once anything knows.
    ///
    /// One place, derived rather than stored beside the lifecycle: a sender
    /// learns it from staging, a receiver from the peer's header, and both used
    /// to be copied into top-level record fields that could then disagree with
    /// the lifecycle. `None` is honest — a minted send between create and offer
    /// genuinely has no document.
    pub const fn content(&self) -> Option<&TransferContent> {
        match self {
            Self::NotRequired { peer_content } => peer_content.as_ref(),
            // Chosen but not yet read: the provider's claimed name is the best
            // that is true, and it has no counted total.
            Self::AwaitingSelection(_) | Self::Acquiring(_) | Self::Staging { .. } => None,
            Self::Ready { content, .. } => Some(content.content()),
        }
    }

    /// The name to show, including the provisional one a chosen-but-unstaged
    /// document has. Deliberately separate from [`content`]: this may be the
    /// provider's claim, which is not authoritative and must never become a
    /// total.
    pub fn display_name(&self) -> Option<&OfferedName> {
        match self {
            Self::NotRequired { peer_content } => peer_content.as_ref().map(TransferContent::name),
            Self::AwaitingSelection(_) => None,
            Self::Acquiring(offer) | Self::Staging { offer, .. } => Some(offer.display_name()),
            Self::Ready { content, .. } => Some(content.name()),
        }
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
            TransferContent::new(
                OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
                ByteCount::new(total),
            ),
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

    /// A receiver's content must be buildable from what the PEER HEADER
    /// carries, and that is name and size — the full-file digest arrives only
    /// with `Complete`, after the bytes.
    ///
    /// Typing this as `StagedContent`, which requires a hash, made the
    /// documented receiver state unconstructible at header admission unless the
    /// reducer invented a digest. Inventing one would be worse than the missing
    /// field: it would be a verification value that verifies nothing.
    #[test]
    fn a_receivers_content_needs_no_digest_it_cannot_have_yet() {
        let announced = TransferContent::new(
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            ByteCount::new(4096),
        );
        let receiving = SourceLifecycle::NotRequired {
            peer_content: Some(announced.clone()),
        };
        let SourceLifecycle::NotRequired {
            peer_content: Some(stored),
        } = &receiving
        else {
            panic!("a receiver holds what the peer announced");
        };
        assert_eq!(stored, &announced);
        assert!(!receiving.requires_a_source());

        // The sender's staged content is the same two facts PLUS the digest,
        // so one is not the other and neither can stand in for it.
        let staged = StagedContent::new(announced.clone(), ContentHash::from_bytes([3; 32]));
        assert_eq!(staged.content(), &announced);
        assert_eq!(staged.name(), announced.name());
        assert_eq!(staged.total(), announced.total());
    }

    /// Serde must not be able to build what the constructors refuse.
    ///
    /// `#[non_exhaustive]` stops another CRATE constructing an invalid variant;
    /// it does nothing about a hostile storage editor. The domain type is
    /// therefore serialized through a plain mirror, and the only way back is
    /// TryFrom, which re-runs the checks. Here: bytes claiming a card that
    /// FAILED never tried.
    #[test]
    fn stored_bytes_cannot_construct_a_gate_the_constructors_refuse() {
        let honest = SourceLifecycle::AwaitingSelection(
            SelectionGate::lost(SourcePromptReason::PermissionLost, offer(1))
                .expect("a failure reason"),
        );
        let encoded = serde_json::to_string(&honest).expect("encodes");
        assert_eq!(
            serde_json::from_str::<SourceLifecycle>(&encoded).expect("round trips"),
            honest
        );

        // The one substitution the type system cannot prevent in stored bytes.
        let forged = encoded.replace("permission_lost", "initial");
        assert_ne!(forged, encoded, "the reason must appear in the bytes");
        assert!(
            serde_json::from_str::<SourceLifecycle>(&forged).is_err(),
            "a re-pick gate claiming Initial must not decode"
        );
    }

    /// An unknown field is a different build's record, not something to accept
    /// with the rest silently applied.
    #[test]
    fn stored_bytes_with_an_unknown_field_are_refused() {
        let state = SourceLifecycle::Acquiring(offer(1));
        let encoded = serde_json::to_string(&state).expect("encodes");
        let extended = encoded.replace(r#"{"offer""#, r#"{"surprise":1,"offer""#);
        assert_ne!(extended, encoded);
        assert!(serde_json::from_str::<SourceLifecycle>(&extended).is_err());
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
            TransferContent::new(
                OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
                ByteCount::new(4096),
            ),
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

// ---- the durable mirror ----
//
// The domain types above are NOT deserialized directly. Serde constructs
// whatever the bytes say, which would walk straight past every checked
// constructor and hand back an invalid live value — a receiver holding a
// source, a `RePickRequired` claiming `Initial`, a key naming another card.
// `#[non_exhaustive]` stops another CRATE building those; it does not stop a
// hostile storage editor.
//
// So the wire shape is these plain mirrors, and the only way back into the
// domain is `TryFrom`, which re-runs the checks. Bytes that fail become a
// typed decode error, which the record layer quarantines.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceKeyDto {
    card: envoix_types::RecordId,
    generation: envoix_types::AttemptGen,
    request: envoix_types::RequestId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OfferDto {
    key: SourceKeyDto,
    display_name: OfferedName,
    reported_size: Option<ByteCount>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferContentDto {
    name: OfferedName,
    total: ByteCount,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContentDto {
    content: TransferContentDto,
    content_hash: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) enum GateDto {
    Selectable {
        reason: SourcePromptReason,
    },
    RePickRequired {
        reason: SourcePromptReason,
        previous_offer: OfferDto,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) enum SourceLifecycleDto {
    NotRequired {
        peer_content: Option<TransferContentDto>,
    },
    AwaitingSelection {
        gate: GateDto,
    },
    Acquiring {
        offer: OfferDto,
    },
    Staging {
        offer: OfferDto,
        acquired_retention: SourceRetention,
        plan: StagingPlan,
    },
    Ready {
        offer: OfferDto,
        acquired_retention: SourceRetention,
        backing: SourceBacking,
        content: ContentDto,
    },
}

/// Why stored bytes are not a source state this build will make live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceDecodeError {
    /// A post-failure gate claiming the card never tried.
    ImpossiblePromptReason,
    /// A retention the plan or the backing beside it cannot be true with.
    ///
    /// `ProviderStream` means the grant is persisted and the source can seek —
    /// a `Process` grant satisfies neither. `PersistedProvider` means the bytes
    /// are reopened through a grant that survives a restart, which a `Process`
    /// grant is by definition not. Both products decoded before this: the DTO
    /// exposed the raw pair and rebuilt it without asking whether the two facts
    /// could hold together.
    ImpossibleRetention,
    /// A byte count past the narrowest representation this product can carry
    /// end to end. See [`MAX_SOURCE_BYTES`].
    SourceTooLarge,
}

impl core::fmt::Display for SourceDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ImpossiblePromptReason => formatter
                .write_str("a post-failure selection gate cannot claim the card never tried"),
            Self::ImpossibleRetention => formatter
                .write_str("the platform retention cannot be true beside this plan or backing"),
            Self::SourceTooLarge => {
                formatter.write_str("the source byte count exceeds the representable maximum")
            }
        }
    }
}

/// An accepted offer whose provider claim is representable.
///
/// `reported_size` is the provider's, so hostile bytes can put anything in it.
/// It is advisory and never becomes a total — but a value this product cannot
/// carry end to end must not become a live one either, or a frontend would be
/// handed a size it cannot render.
fn checked_offer(offer: OfferDto) -> Result<AcceptedSourceOffer, SourceDecodeError> {
    let offer: AcceptedSourceOffer = offer.into();
    match offer.reported_size() {
        Some(size) if size.get() > MAX_SOURCE_BYTES => Err(SourceDecodeError::SourceTooLarge),
        _ => Ok(offer),
    }
}

impl From<&SourceAcquisitionKey> for SourceKeyDto {
    fn from(key: &SourceAcquisitionKey) -> Self {
        let provenance = key.provenance();
        Self {
            card: provenance.card,
            generation: provenance.generation,
            request: provenance.request,
        }
    }
}

impl From<SourceKeyDto> for SourceAcquisitionKey {
    fn from(dto: SourceKeyDto) -> Self {
        Self::of(envoix_capabilities::DutyProvenance {
            card: dto.card,
            generation: dto.generation,
            request: dto.request,
        })
    }
}

impl From<&AcceptedSourceOffer> for OfferDto {
    fn from(offer: &AcceptedSourceOffer) -> Self {
        Self {
            key: offer.key().into(),
            display_name: offer.display_name().clone(),
            reported_size: offer.reported_size(),
        }
    }
}

impl From<OfferDto> for AcceptedSourceOffer {
    fn from(dto: OfferDto) -> Self {
        Self::new(dto.key.into(), dto.display_name, dto.reported_size)
    }
}

impl From<&TransferContent> for TransferContentDto {
    fn from(content: &TransferContent) -> Self {
        Self {
            name: content.name().clone(),
            total: content.total(),
        }
    }
}

impl From<TransferContentDto> for TransferContent {
    fn from(dto: TransferContentDto) -> Self {
        Self::new(dto.name, dto.total)
    }
}

impl From<&StagedContent> for ContentDto {
    fn from(content: &StagedContent) -> Self {
        Self {
            content: content.content().into(),
            content_hash: content.content_hash().to_bytes(),
        }
    }
}

impl From<ContentDto> for StagedContent {
    fn from(dto: ContentDto) -> Self {
        Self::new(
            dto.content.into(),
            ContentHash::from_bytes(dto.content_hash),
        )
    }
}

impl From<SourceLifecycle> for SourceLifecycleDto {
    fn from(state: SourceLifecycle) -> Self {
        Self::from(&state)
    }
}

impl From<&SourceLifecycle> for SourceLifecycleDto {
    fn from(state: &SourceLifecycle) -> Self {
        match state {
            SourceLifecycle::NotRequired { peer_content } => Self::NotRequired {
                peer_content: peer_content.as_ref().map(Into::into),
            },
            SourceLifecycle::AwaitingSelection(gate) => Self::AwaitingSelection {
                gate: match gate {
                    SelectionGate::Selectable { reason } => GateDto::Selectable { reason: *reason },
                    SelectionGate::RePickRequired {
                        reason,
                        previous_offer,
                    } => GateDto::RePickRequired {
                        reason: *reason,
                        previous_offer: previous_offer.into(),
                    },
                },
            },
            SourceLifecycle::Acquiring(offer) => Self::Acquiring {
                offer: offer.into(),
            },
            SourceLifecycle::Staging {
                offer,
                acquired_retention,
                plan,
            } => Self::Staging {
                offer: offer.into(),
                acquired_retention: *acquired_retention,
                plan: *plan,
            },
            SourceLifecycle::Ready {
                offer,
                acquired_retention,
                backing,
                content,
            } => Self::Ready {
                offer: offer.into(),
                acquired_retention: *acquired_retention,
                backing: *backing,
                content: content.into(),
            },
        }
    }
}

impl TryFrom<SourceLifecycleDto> for SourceLifecycle {
    type Error = SourceDecodeError;

    fn try_from(dto: SourceLifecycleDto) -> Result<Self, Self::Error> {
        Ok(match dto {
            SourceLifecycleDto::NotRequired { peer_content } => Self::NotRequired {
                peer_content: peer_content.map(Into::into),
            },
            SourceLifecycleDto::AwaitingSelection { gate } => Self::AwaitingSelection(match gate {
                // The checked constructors, re-run. Bytes claiming a card that
                // failed never tried are refused rather than made live.
                GateDto::Selectable { reason } => match reason {
                    SourcePromptReason::Initial => SelectionGate::initial(),
                    failure => SelectionGate::selectable_again(failure)
                        .ok_or(SourceDecodeError::ImpossiblePromptReason)?,
                },
                GateDto::RePickRequired {
                    reason,
                    previous_offer,
                } => SelectionGate::lost(reason, previous_offer.into())
                    .ok_or(SourceDecodeError::ImpossiblePromptReason)?,
            }),
            SourceLifecycleDto::Acquiring { offer } => Self::Acquiring(checked_offer(offer)?),
            SourceLifecycleDto::Staging {
                offer,
                acquired_retention,
                plan,
            } => {
                if !plan.is_possible_with(acquired_retention) {
                    return Err(SourceDecodeError::ImpossibleRetention);
                }
                Self::Staging {
                    offer: checked_offer(offer)?,
                    acquired_retention,
                    plan,
                }
            }
            SourceLifecycleDto::Ready {
                offer,
                acquired_retention,
                backing,
                content,
            } => {
                if !backing.is_possible_with(acquired_retention) {
                    return Err(SourceDecodeError::ImpossibleRetention);
                }
                let content: StagedContent = content.into();
                if content.total().get() > MAX_SOURCE_BYTES {
                    return Err(SourceDecodeError::SourceTooLarge);
                }
                Self::Ready {
                    offer: checked_offer(offer)?,
                    acquired_retention,
                    backing,
                    content,
                }
            }
        })
    }
}
