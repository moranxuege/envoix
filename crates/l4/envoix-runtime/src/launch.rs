//! What one attempt needs in hand before it can run.
//!
//! The card actor resolves this while dispatching its own `StartAttempt`, from
//! the record it has just committed. That placement is the whole point: the card
//! is the source authority, and it is the only thing that observes the committed
//! `Ready` and starts the attempt in one step. A host that watched for `Ready`
//! and prepared the source separately would be observing the right fact at the
//! wrong place — subscription updates are drained asynchronously, so there is no
//! happens-before edge between seeing `Ready` and the executor starting.
//!
//! What crosses to the executor is the resolved CAPABILITY, never the
//! acquisition. An executor that was told which acquisition to open would need
//! a registry to open it from, and a registry is a lookup — one step from
//! resolving a source the authority did not name. Handing down an opened session
//! leaves it nothing to look up.

use envoix_attempt_api::AttemptPlan;
use envoix_capabilities::{SourceAcquisitionKey, SourceSession};
use envoix_product::{ContentHash, SourceBacking, SourceLifecycle};
use envoix_types::{ArtifactId, ByteCount, Direction};

/// Which established source to open. Derived inside the card from the committed
/// lifecycle; it reaches no executor, no wire frame and no durable record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLocator {
    /// The provider, reopened through the grant that acquisition took.
    PersistedProvider { acquisition: SourceAcquisitionKey },
    /// An artifact this app owns outright.
    OwnedArtifact { artifact: ArtifactId },
}

impl SourceLocator {
    /// The locator for a source that is ready to send, or `None` for a card that
    /// has no established source to name.
    pub fn of(source: &SourceLifecycle, artifact: ArtifactId) -> Option<Self> {
        let SourceLifecycle::Ready { offer, backing, .. } = source else {
            return None;
        };
        Some(match backing {
            SourceBacking::PersistedProvider => Self::PersistedProvider {
                acquisition: *offer.key(),
            },
            SourceBacking::OwnedArtifact => Self::OwnedArtifact { artifact },
        })
    }
}

/// What staging established about the bytes: how many, and which ones.
///
/// Travels with the session rather than in the plan, because it is a fact about
/// this source and not about the transport. The sender compares the digest
/// against what it actually transmitted before claiming completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StagedIdentity {
    pub total: ByteCount,
    pub digest: ContentHash,
}

/// An opened source, with what it is supposed to contain.
///
/// Move-only and with no accessor that hands the session back out by reference:
/// exactly one attempt reads it, and it is consumed to do so. Two attempts
/// holding one session would be two attempts sending one document.
pub struct PreparedSource {
    session: Box<dyn SourceSession>,
    identity: StagedIdentity,
}

impl PreparedSource {
    pub fn new(session: Box<dyn SourceSession>, identity: StagedIdentity) -> Self {
        Self { session, identity }
    }

    pub const fn identity(&self) -> StagedIdentity {
        self.identity
    }

    /// Takes the session, consuming this. The attempt that calls it is the one
    /// that reads the bytes.
    pub fn into_session(self) -> Box<dyn SourceSession> {
        self.session
    }
}

impl std::fmt::Debug for PreparedSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSource")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Why a source could not be opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceResolveError {
    /// This composition resolves no sources at all. A host with no platform
    /// source sessions answers this for everything.
    Unsupported,
    /// This process is not holding that source. A descriptor does not survive
    /// the process that opened it, so a card restored over a persisted grant
    /// lands here until something rebinds it.
    Absent,
}

/// Opens the bytes of an established source.
///
/// The port the card asks. Its implementation is the composition root's, because
/// what a source IS — a duplicated Android descriptor, a file, an owned artifact
/// — is a platform fact and the runtime holds none of them.
pub trait PreparedSourceResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        locator: SourceLocator,
        identity: StagedIdentity,
    ) -> Result<PreparedSource, SourceResolveError>;
}

/// The resolver for a composition that has no source sessions.
///
/// Answers `Unsupported` for everything, which is honest and is NOT the same as
/// a source having gone away: the card fails either way, but only one of them is
/// about the document the person chose.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSourceSessions;

impl PreparedSourceResolver for NoSourceSessions {
    fn resolve(
        &self,
        _locator: SourceLocator,
        _identity: StagedIdentity,
    ) -> Result<PreparedSource, SourceResolveError> {
        Err(SourceResolveError::Unsupported)
    }
}

/// The I/O one attempt performs.
#[derive(Debug)]
pub enum PreparedAttemptIo {
    Send(PreparedSource),
    /// A receive attempt's staging sink belongs here. It carries nothing yet
    /// because no production `StagingSink` exists — every implementation in the
    /// tree is a test double — and naming a capability this arm cannot supply
    /// would be the same lie as an unwritten artifact.
    Receive,
}

/// One attempt, with the I/O its direction requires.
///
/// The constructors are the check. An `Option<PreparedSource>` beside a plan
/// permits a send with no source and a receive with one, and both are states no
/// card can be in — so the type refuses to represent them rather than every
/// executor remembering to.
#[derive(Debug)]
pub struct AttemptLaunch {
    plan: AttemptPlan,
    io: PreparedAttemptIo,
}

impl AttemptLaunch {
    /// A send, over the source it will read. `None` if the plan is not a send.
    pub fn sending(plan: AttemptPlan, source: PreparedSource) -> Option<Self> {
        (plan.direction == Direction::Send).then_some(Self {
            plan,
            io: PreparedAttemptIo::Send(source),
        })
    }

    /// A receive. `None` if the plan is not a receive.
    pub fn receiving(plan: AttemptPlan) -> Option<Self> {
        (plan.direction == Direction::Receive).then_some(Self {
            plan,
            io: PreparedAttemptIo::Receive,
        })
    }

    pub const fn plan(&self) -> AttemptPlan {
        self.plan
    }

    pub fn into_parts(self) -> (AttemptPlan, PreparedAttemptIo) {
        (self.plan, self.io)
    }
}

#[cfg(test)]
mod tests {
    use envoix_attempt_api::ResumeIntent;
    use envoix_capabilities::{DutyProvenance, SourceReadError};
    use envoix_product::AcceptedSourceOffer;
    use envoix_types::{AttemptGen, OfferedName, RecordId, RequestId, TransferId};

    use super::*;

    struct EmptySource;

    impl SourceSession for EmptySource {
        fn read_at(
            &mut self,
            _offset: ByteCount,
            _destination: &mut [u8],
        ) -> Result<usize, SourceReadError> {
            Ok(0)
        }
    }

    fn identity() -> StagedIdentity {
        StagedIdentity {
            total: ByteCount::new(9),
            digest: ContentHash::from_bytes([4; 32]),
        }
    }

    fn source() -> PreparedSource {
        PreparedSource::new(Box::new(EmptySource), identity())
    }

    fn plan(direction: Direction) -> AttemptPlan {
        AttemptPlan {
            stamp: envoix_attempt_api::AttemptStamp {
                card: RecordId::new(5),
                generation: AttemptGen::new(1),
            },
            direction,
            transfer: TransferId::from_bytes([1; 16]),
            artifact: ArtifactId::from_bytes([2; 16]),
            resume: ResumeIntent::Fresh,
        }
    }

    fn acquisition() -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(DutyProvenance {
            card: RecordId::new(5),
            generation: AttemptGen::new(1),
            request: RequestId::from_bytes([6; 16]),
        })
    }

    /// The direction and the I/O cannot disagree.
    ///
    /// An `Option<PreparedSource>` beside a plan represents a send with no source
    /// and a receive with one — states no card can be in — and leaves every
    /// executor to notice. The constructors refuse instead.
    #[test]
    fn a_launch_cannot_carry_the_wrong_io_for_its_direction() {
        assert!(AttemptLaunch::sending(plan(Direction::Send), source()).is_some());
        assert!(AttemptLaunch::receiving(plan(Direction::Receive)).is_some());

        assert!(
            AttemptLaunch::sending(plan(Direction::Receive), source()).is_none(),
            "a receive was launched with a source to send"
        );
        assert!(
            AttemptLaunch::receiving(plan(Direction::Send)).is_none(),
            "a send was launched with nothing to send"
        );
    }

    /// Only an ESTABLISHED source can be located. A card that has merely been
    /// given a document has no counted, identified bytes to open.
    ///
    /// The `Ready` cases are not here because `Ready` has no public constructor
    /// — that is the property four commits of this arc exist to hold, and adding
    /// a way to mint one for a test would spend it. They are covered where a real
    /// card reaches `Ready`, in `tests/lifecycle.rs`.
    #[test]
    fn a_source_that_is_only_held_cannot_be_located() {
        assert_eq!(
            SourceLocator::of(
                &SourceLifecycle::Acquiring(AcceptedSourceOffer::new(
                    acquisition(),
                    OfferedName::from_untrusted("payload.bin").expect("a bounded name"),
                    None,
                )),
                ArtifactId::from_bytes([2; 16])
            ),
            None,
            "a card that only holds a document named bytes to send"
        );
    }

    /// A composition with no source sessions says so, and says it differently
    /// from a source that has gone away.
    #[test]
    fn the_null_resolver_is_unsupported_not_absent() {
        assert!(matches!(
            NoSourceSessions.resolve(
                SourceLocator::PersistedProvider {
                    acquisition: acquisition()
                },
                identity()
            ),
            Err(SourceResolveError::Unsupported)
        ));
    }
}
