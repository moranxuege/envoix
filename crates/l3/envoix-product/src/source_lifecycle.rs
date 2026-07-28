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
//! A hostile storage editor can always write invalid bytes. "Not representable"
//! means such bytes cannot become a live value: these constructors will not
//! build one.

use envoix_capabilities::SourceAcquisitionKey;
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

/// How long the platform's hold on the document actually lasts.
///
/// The reason `duty/2` exists. A source duty used to answer `completed`, which
/// said nothing about surviving a restart — so a card could believe it owned a
/// document it would lose. Two different promises, now two different values.
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
    /// What the provider SAID the size was — deliberately not the transfer's
    /// total. A provider is untrusted about length; the authoritative total
    /// comes from staging, which counted the bytes.
    reported_size: Option<ByteCount>,
}

impl AcceptedSourceOffer {
    pub const fn new(key: SourceAcquisitionKey, reported_size: Option<ByteCount>) -> Self {
        Self { key, reported_size }
    }

    pub const fn key(&self) -> &SourceAcquisitionKey {
        &self.key
    }

    pub const fn reported_size(&self) -> Option<ByteCount> {
        self.reported_size
    }
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
}

impl StagedContent {
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
    Selectable { reason: SourcePromptReason },
    /// This generation accepted an offer and then lost it. Only a re-pick —
    /// which advances the generation and mints a new key — makes the card
    /// selectable again, so a late offer under the discharged key cannot
    /// resurrect it.
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
    NotRequired,
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
    Staging {
        offer: AcceptedSourceOffer,
        retention: SourceRetention,
    },
    /// The content is established and the source can be sent.
    ///
    /// `retention` is what a restart consults: `Persisted` can be reopened,
    /// `Process` cannot and returns the card to awaiting selection however
    /// complete it looked.
    Ready {
        offer: AcceptedSourceOffer,
        retention: SourceRetention,
        content: StagedContent,
    },
}

impl SourceLifecycle {
    /// The state a newly created card starts in — a pure function of its
    /// direction. The direction/source invariant lives here: a receiver cannot
    /// be born awaiting a source, a sender cannot be born not needing one.
    pub const fn initial(direction: Direction) -> Self {
        match direction {
            Direction::Receive => Self::NotRequired,
            Direction::Send => Self::AwaitingSelection(SelectionGate::initial()),
        }
    }

    pub const fn requires_a_source(&self) -> bool {
        !matches!(self, Self::NotRequired)
    }

    /// The acquisition this state is bound to, if any. Inputs match against the
    /// WHOLE key; "the card matches" is not the question.
    pub const fn key(&self) -> Option<&SourceAcquisitionKey> {
        match self {
            Self::NotRequired | Self::AwaitingSelection(_) => None,
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

    fn offer(generation: u32) -> AcceptedSourceOffer {
        AcceptedSourceOffer::new(key(generation), Some(ByteCount::new(4096)))
    }

    /// The direction/source invariant, in the one place that decides it. A
    /// receiver born awaiting a source, or a sender born not needing one, are
    /// the two states this makes unreachable.
    #[test]
    fn the_starting_state_is_a_function_of_direction_alone() {
        let receiving = SourceLifecycle::initial(Direction::Receive);
        assert_eq!(receiving, SourceLifecycle::NotRequired);
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
        assert!(SourceLifecycle::NotRequired.key().is_none());
        assert!(
            SourceLifecycle::AwaitingSelection(SelectionGate::initial())
                .key()
                .is_none()
        );

        let bound = [
            SourceLifecycle::Acquiring(offer(2)),
            SourceLifecycle::Staging {
                offer: offer(2),
                retention: SourceRetention::Process,
            },
            SourceLifecycle::Ready {
                offer: offer(2),
                retention: SourceRetention::Persisted,
                content: StagedContent::new(
                    OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
                    ByteCount::new(4096),
                ),
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
                retention: SourceRetention::Persisted,
            }
            .is_ready()
        );
        assert!(
            SourceLifecycle::Ready {
                offer: offer(1),
                retention: SourceRetention::Persisted,
                content: StagedContent::new(
                    OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
                    ByteCount::new(1),
                ),
            }
            .is_ready()
        );
    }

    /// The provider's claim about size and the counted total are different
    /// facts and are stored as different fields, so one cannot be read as the
    /// other. A provider is untrusted about length.
    #[test]
    fn a_reported_size_is_never_the_transfers_total() {
        let lying = AcceptedSourceOffer::new(key(1), Some(ByteCount::new(10)));
        let counted = StagedContent::new(
            OfferedName::from_untrusted("report.pdf").expect("a bounded name"),
            ByteCount::new(4096),
        );
        let ready = SourceLifecycle::Ready {
            offer: lying.clone(),
            retention: SourceRetention::Persisted,
            content: counted,
        };
        let SourceLifecycle::Ready { offer, content, .. } = &ready else {
            panic!("constructed ready");
        };
        assert_eq!(offer.reported_size(), Some(ByteCount::new(10)));
        assert_eq!(content.total(), ByteCount::new(4096));

        // A provider that reported nothing is normal and is not a failure.
        assert!(
            AcceptedSourceOffer::new(key(1), None)
                .reported_size()
                .is_none()
        );
    }
}
