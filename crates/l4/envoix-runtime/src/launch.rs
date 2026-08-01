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

use std::sync::Arc;

use envoix_outcomes::OutcomeCode;
use tokio::sync::oneshot;

use envoix_attempt_api::AttemptPlan;
use envoix_blob_api::{BlobKey, BlobWorkId, SealedArtifact, SinkSession};
use envoix_capabilities::{SourceAcquisitionKey, SourceSession};
use envoix_product::{ContentHash, SourceBacking, SourceLifecycle};
use envoix_types::{ArtifactId, ByteCount, Direction, RecordId, TransferId};

use crate::port::SourceStagingExecutor;

/// Which established source to open. Derived inside the card from the committed
/// lifecycle; it reaches no executor, no wire frame and no durable record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLocator {
    /// The provider, reopened through the grant that acquisition took.
    PersistedProvider { acquisition: SourceAcquisitionKey },
    /// An artifact this app owns outright, named by the SEAL that made it.
    ///
    /// The blob key, not a bare artifact id: an id is a name and a key is an
    /// incarnation, and a re-derivation produces a new incarnation of the same
    /// id. Opening by id alone could reach the previous run's bytes.
    OwnedArtifact { blob: BlobKey },
}

impl SourceLocator {
    /// The locator for a source that is ready to send, or `None` for a card that
    /// has no established source to name.
    ///
    /// Takes only the lifecycle. An owned artifact is named by its own SEAL, so
    /// nothing has to be passed in beside the record and there is no second
    /// value for the seal to disagree with.
    pub fn of(source: &SourceLifecycle) -> Option<Self> {
        let SourceLifecycle::Ready { offer, backing, .. } = source else {
            return None;
        };
        Some(match backing {
            SourceBacking::PersistedProvider => Self::PersistedProvider {
                acquisition: *offer.key(),
            },
            // The SEAL's artifact, not the card's minted identity. They agree —
            // the reducer refuses a seal that names a different one — but
            // reading the seal is what makes that agreement load-bearing rather
            // than a coincidence two fields happen to share.
            SourceBacking::OwnedArtifact { seal } => Self::OwnedArtifact { blob: seal.blob },
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

/// Where one receive puts its bytes: the blob it will write, opened.
///
/// The mirror of [`PreparedSource`], and move-only for the same reason. Two
/// attempts holding one session would be two attempts writing one artifact,
/// which no card can ask for.
pub struct PreparedReceiveSink {
    session: Box<dyn SinkSession>,
}

impl PreparedReceiveSink {
    pub fn new(session: Box<dyn SinkSession>) -> Self {
        Self { session }
    }

    /// Takes the session, consuming this. The attempt that calls it is the one
    /// that writes the bytes.
    pub fn into_session(self) -> Box<dyn SinkSession> {
        self.session
    }
}

impl std::fmt::Debug for PreparedReceiveSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedReceiveSink")
            .finish_non_exhaustive()
    }
}

/// Why a receive destination could not be opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SinkOpenError {
    /// This composition writes no bulk bytes at all. A host with no store
    /// answers this for everything, and it means a card should never have been
    /// asked to receive — not that the disk failed.
    Unsupported,
    /// The volume is full.
    ///
    /// Kept apart from `Unavailable` because it is the one a person can act on,
    /// and because re-choosing a document does not fix it — so it must never
    /// become a re-pick. The store already draws this distinction for the same
    /// reason; collapsing it here would have spent it at the first boundary it
    /// crossed.
    OutOfSpace,
    /// The store refused: it is already leased to another writer, or it faulted.
    /// Retrying is meaningful; choosing something else is not.
    Unavailable,
}

/// Which destination to open, and under what commissioning.
///
/// Two values as ONE, because a call site that updated the key and forgot the
/// fingerprint would open the right file under the wrong eligibility — and the
/// symptom would be a partial silently adopted for content that never
/// commissioned it.
///
/// The card builds it: the plan supplies card, transfer and artifact, and the
/// COMMITTED record supplies the name and size. A host cannot reconstruct it
/// from the key alone, and deliberately so — a `BlobKey` carries no
/// announcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceptionCommission {
    pub blob: BlobKey,
    pub fingerprint: ContentHash,
}

/// What a card finds when it commissions a destination.
///
/// Two arms because "already sealed" is not a failure and not a retry. Sealed
/// bytes are complete and immutable, so the destination can never be opened for
/// writing again — reporting that as a transient fault promises a retry that
/// cannot succeed, no matter how many times it is taken.
///
/// The witness comes back because it is the only thing that can prove those
/// bytes are what this card was expecting, and only the store can mint one.
#[derive(Debug)]
pub enum ReceiveDestination {
    /// Nothing is sealed here. Write.
    Writable(PreparedReceiveSink),
    /// These bytes are already complete, and the store says so.
    AlreadySealed(SealedArtifact),
}

impl ReceiveDestination {
    /// The writable session, panicking if these bytes are already sealed.
    ///
    /// For callers that have just established there is nothing here — tests
    /// mostly. Production must handle both arms, because "already complete" is
    /// a recovery and not a failure.
    pub fn writable(self) -> Box<dyn SinkSession> {
        match self {
            Self::Writable(sink) => sink.into_session(),
            Self::AlreadySealed(_) => panic!("expected a writable destination"),
        }
    }
}

/// Opens the destination a receive writes into.
///
/// The port the card asks, for the same reason as [`PreparedSourceResolver`]:
/// what a destination IS — a file under an app-private root, a lease on a bulk
/// store — is a platform fact, and the runtime holds none of them.
pub trait PreparedSinkResolver: Send + Sync + 'static {
    fn open(&self, commission: ReceptionCommission) -> Result<ReceiveDestination, SinkOpenError>;
}

/// The resolver for a composition that stores no bulk bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoBulkStorage;

impl PreparedSinkResolver for NoBulkStorage {
    fn open(&self, _commission: ReceptionCommission) -> Result<ReceiveDestination, SinkOpenError> {
        Err(SinkOpenError::Unsupported)
    }
}

/// Which blob one receive writes into.
///
/// DERIVED from the plan, never stored, and derived by one function so the two
/// callers that need it — the card opening the sink, and whoever later checks
/// that a seal names what was asked for — cannot compute two different answers.
///
/// Keyed by the TRANSFER, not the attempt generation: a resume mints a new
/// generation against the same transfer, and a receive keyed on the generation
/// would abandon its partial at every resume. That is the opposite of the
/// derivation key, where the generation is exactly what identifies the work.
pub fn receive_blob(card: RecordId, transfer: TransferId, artifact: ArtifactId) -> BlobKey {
    BlobKey::new(card, BlobWorkId::of_reception(transfer, artifact))
}

/// The platform capabilities a composition supplies to the runtime.
///
/// One value rather than three constructor arguments, because they already
/// travel as a group: each is an `Arc<dyn>` port whose implementation is the
/// composition root's, each is cloned into every card actor, and none is ever
/// supplied without the others. Passing them positionally meant every new port
/// edited every call site in the workspace, which is a cost paid to keep a
/// grouping implicit.
#[derive(Clone)]
pub struct PlatformPorts {
    /// How a card stages the source it was given.
    pub staging: Arc<dyn SourceStagingExecutor>,
    /// How a card opens the source it is about to send.
    pub sources: Arc<dyn PreparedSourceResolver>,
    /// How a card opens the destination it is about to receive into.
    pub sinks: Arc<dyn PreparedSinkResolver>,
}

impl PlatformPorts {
    pub fn new(
        staging: impl SourceStagingExecutor,
        sources: impl PreparedSourceResolver,
        sinks: impl PreparedSinkResolver,
    ) -> Self {
        Self {
            staging: Arc::new(staging),
            sources: Arc::new(sources),
            sinks: Arc::new(sinks),
        }
    }
}

impl std::fmt::Debug for PlatformPorts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformPorts")
            .finish_non_exhaustive()
    }
}

/// The destination an attempt is PROMISED, once its content is admitted.
///
/// A receive cannot be handed a sink at launch, because at launch nothing knows
/// what is being received. Commissioning depends on the peer's announcement, and
/// a destination opened before it can only be commissioned for whatever the last
/// attempt was told — which is the wrong one exactly when a re-picked document
/// arrives.
///
/// So the attempt gets a one-shot RECEIVER and no way to open anything itself.
/// It may accept exactly one destination, chosen by the card, and it has no
/// operation for choosing another.
#[derive(Debug)]
pub struct PendingReceiveSink {
    granted: oneshot::Receiver<Result<PreparedReceiveSink, OutcomeCode>>,
}

impl PendingReceiveSink {
    /// Waits for the card to commission a destination.
    ///
    /// `Err` is a local storage outcome the attempt should end with; a dropped
    /// grant is `Internal`, because a card that went away without commissioning
    /// has not authorized anything.
    pub async fn granted(self) -> Result<PreparedReceiveSink, OutcomeCode> {
        self.granted.await.unwrap_or(Err(OutcomeCode::Internal))
    }
}

/// The card's half of the promise. One shot, and held with the live attempt.
///
/// Move-only and consumed by fulfilling it: a duplicate header, a stale attempt,
/// or an attempt whose grant is already spent cannot obtain a second
/// destination.
#[derive(Debug)]
pub struct ReceiveSinkGrant {
    grant: oneshot::Sender<Result<PreparedReceiveSink, OutcomeCode>>,
}

impl ReceiveSinkGrant {
    /// Hands over the commissioned destination, or the outcome that stopped it.
    ///
    /// A closed receiver means the attempt is gone; the sink is dropped, which
    /// releases its writer lease.
    pub fn fulfill(self, resolved: Result<PreparedReceiveSink, OutcomeCode>) {
        let _ = self.grant.send(resolved);
    }
}

/// Creates the two halves of one attempt's destination promise.
pub fn receive_sink_gate() -> (ReceiveSinkGrant, PendingReceiveSink) {
    let (grant, granted) = oneshot::channel();
    (ReceiveSinkGrant { grant }, PendingReceiveSink { granted })
}

/// The I/O one attempt performs.
#[derive(Debug)]
pub enum PreparedAttemptIo {
    Send(PreparedSource),
    Receive(PendingReceiveSink),
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

    /// A receive, over the destination it is PROMISED. `None` if the plan is not
    /// a receive.
    pub fn receiving(plan: AttemptPlan, sink: PendingReceiveSink) -> Option<Self> {
        (plan.direction == Direction::Receive).then_some(Self {
            plan,
            io: PreparedAttemptIo::Receive(sink),
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
    use envoix_types::{ArtifactId, AttemptGen, OfferedName, RecordId, RequestId, TransferId};

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

    /// A destination that accepts nothing. These cases are about which I/O may
    /// accompany which direction, so the bytes never matter.
    struct EmptySink;

    impl envoix_blob_api::SinkSession for EmptySink {
        fn resume(&mut self) -> Result<envoix_types::DurablePrefix, envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }

        fn read_partial_at(
            &mut self,
            _offset: ByteCount,
            _destination: &mut [u8],
        ) -> Result<usize, envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }

        fn append(
            &mut self,
            _offset: ByteCount,
            _bytes: &[u8],
        ) -> Result<(), envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }

        fn checkpoint(
            &mut self,
            _prefix: envoix_types::DurablePrefix,
        ) -> Result<(), envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }

        fn reset(&mut self) -> Result<(), envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }

        fn seal(
            self: Box<Self>,
            _expected_size: ByteCount,
            _digest: ContentHash,
        ) -> Result<envoix_blob_api::SealedArtifact, envoix_blob_api::BlobError> {
            unimplemented!("no case here writes")
        }
    }

    fn sink() -> PreparedReceiveSink {
        PreparedReceiveSink::new(Box::new(EmptySink))
    }

    fn pending() -> PendingReceiveSink {
        receive_sink_gate().1
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
        assert!(AttemptLaunch::receiving(plan(Direction::Receive), pending()).is_some());

        assert!(
            AttemptLaunch::sending(plan(Direction::Receive), source()).is_none(),
            "a receive was launched with a source to send"
        );
        assert!(
            AttemptLaunch::receiving(plan(Direction::Send), pending()).is_none(),
            "a send was launched with somewhere to receive"
        );
    }

    /// The promise delivers exactly one destination, and a card that goes away
    /// without commissioning delivers none.
    ///
    /// The second half is the one that matters: a dropped grant must not read as
    /// permission. An attempt that took silence for a destination would have
    /// nowhere to write and no way to say so.
    #[tokio::test]
    async fn a_promised_destination_arrives_once_or_not_at_all() {
        let (grant, pending) = receive_sink_gate();
        grant.fulfill(Ok(sink()));
        assert!(pending.granted().await.is_ok());

        let (grant, pending) = receive_sink_gate();
        grant.fulfill(Err(OutcomeCode::StorageFull));
        assert_eq!(
            pending.granted().await.err(),
            Some(OutcomeCode::StorageFull)
        );

        // The card went away without commissioning anything.
        let (grant, pending) = receive_sink_gate();
        drop(grant);
        assert_eq!(
            pending.granted().await.err(),
            Some(OutcomeCode::Internal),
            "silence is not a destination"
        );
    }

    /// A receive keeps its partial across a resume, so its blob may not be keyed
    /// by the attempt generation — which a resume advances on purpose.
    ///
    /// This is the same distinction the DERIVATION key resolves the other way:
    /// there the generation is exactly what identifies the work, because a
    /// re-derivation must not adopt the previous run's bytes. Written down here
    /// because getting it backwards is silent — the receive would simply lose
    /// its partial at every resume and nothing would report a fault.
    #[test]
    fn a_receive_blob_survives_a_new_attempt_generation() {
        let first = plan(Direction::Receive);
        let mut resumed = first;
        resumed.stamp.generation = AttemptGen::new(2);
        resumed.resume = ResumeIntent::Allowed;

        let key = |plan: AttemptPlan| receive_blob(plan.stamp.card, plan.transfer, plan.artifact);
        assert_eq!(key(first), key(resumed));

        let mut other_transfer = first;
        other_transfer.transfer = TransferId::from_bytes([9; 16]);
        assert_ne!(
            key(first),
            key(other_transfer),
            "two transfers on one card must not share a destination"
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
            SourceLocator::of(&SourceLifecycle::Acquiring(
                AcceptedSourceOffer::of_one_document(
                    acquisition(),
                    OfferedName::from_untrusted("payload.bin").expect("a bounded name"),
                    None,
                )
            )),
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
