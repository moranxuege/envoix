//! Reading a bound file through, as the source-staging worker.
//!
//! Lives in the composition root beside [`PreparedIrohExecutor`], and for the
//! same reason: an L4 port is implemented by whoever composes the runtime, never
//! by an L6 platform adapter — which is why the arch gate refuses the latter.
//!
//! This is the FILESYSTEM implementation. It makes `Staging → Ready` provable
//! without a device, and it is what the CLI uses. It is not a stand-in for the
//! Android one: a `content://` URI and a path are different platform facts, and
//! the Android source session is its own slice.
//!
//! [`PreparedIrohExecutor`]: crate::executor::PreparedIrohExecutor
//!
//! What it establishes is what the authority asked for and nothing else — a
//! counted total and a digest of the bytes that were counted. It writes nothing:
//! the streaming case is a read-through, which is the whole reason `Ready` can
//! mean "we know these bytes" without doubling disk.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use envoix_runtime::{
    ContentHash, SourceAcquisitionKey, SourceStagingExecution, SourceStagingExecutor,
    SourceStagingPlan, SourceStagingSignal, StagingPlan, StopToken, stop_channel,
};
use envoix_types::{AttemptGen, ByteCount};
use tokio::sync::mpsc;

/// How much is read between progress reports.
///
/// Progress is an observation, not a durability boundary: the reducer keeps the
/// newest count and the lane coalesces, so this trades report frequency against
/// syscalls rather than against correctness.
const READ_CHUNK_BYTES: usize = 256 * 1024;

/// How this host can open the bytes for one acquisition.
///
/// Two ways because there are two platforms, not two designs: a filesystem has
/// paths and Android has a descriptor its own side opened. Both resolve to a
/// readable file, which is why one worker serves both.
enum BoundSource {
    /// A path this process may open whenever it likes.
    Path(PathBuf),
    /// This process's DUPLICATE of a descriptor the platform opened. The
    /// platform lends its own and closes it; this one is closed here. Two
    /// descriptors, one open file description, one closer each.
    ///
    /// Process-local by nature, and correctly so: an open descriptor means
    /// nothing after the process that holds it dies. A registry that tried to
    /// survive one would be claiming something untrue.
    Descriptor(OwnedFd),
}

/// This host's source registry: how a given acquisition's bytes are reached.
///
/// Keyed by the WHOLE acquisition. A registry keyed by card would let a later
/// generation inherit an earlier one's document, which is the ownership defect
/// the key exists to close — and is the defect Android's own `SourcePicks` had
/// until it was rekeyed.
#[derive(Clone, Default)]
pub struct BoundSourceRegistry {
    sources: Arc<Mutex<HashMap<SourceAcquisitionKey, BoundSource>>>,
}

impl BoundSourceRegistry {
    /// Binds a path to one acquisition. The authority publishes the key; this is
    /// a filesystem host's answer to it.
    pub fn bind(&self, acquisition: SourceAcquisitionKey, path: PathBuf) {
        self.lock().insert(acquisition, BoundSource::Path(path));
    }

    /// Binds this process's duplicate of a platform descriptor to one
    /// acquisition. Returns whether it was taken.
    ///
    /// Takes an `OwnedFd`, not an integer. Borrowing a raw descriptor is unsafe
    /// by nature — it asserts the number is open and will outlive the borrow —
    /// and that assertion is only checkable at the JNI boundary, where the
    /// caller's `ParcelFileDescriptor` is held across the call. So the boundary
    /// makes it and this takes the safe result: no caller of this registry can
    /// get the ownership question wrong, because it is not asked here.
    ///
    /// FIRST BIND WINS for an acquisition. A later bind under a key already
    /// bound is refused and its descriptor closed, because staging has by then
    /// read the source through and established a digest against that exact open
    /// file description — replacing it would leave the attempt reading a
    /// different document than the one the record vouches for, silently. A new
    /// document is a new acquisition, which is what a re-pick mints.
    pub fn adopt_descriptor(&self, acquisition: SourceAcquisitionKey, descriptor: OwnedFd) -> bool {
        let mut sources = self.lock();
        if sources.contains_key(&acquisition) {
            return false;
        }
        sources.insert(acquisition, BoundSource::Descriptor(descriptor));
        true
    }

    /// Drops what an acquisition was bound to, closing a descriptor if it held
    /// one.
    ///
    /// Called when the acquisition is superseded or its card goes away. Without
    /// it a report the authority refused would leave a descriptor resident for
    /// the life of the process — the orphan this registry's process-local
    /// lifetime does not by itself prevent.
    pub fn discard(&self, acquisition: &SourceAcquisitionKey) {
        self.lock().remove(acquisition);
    }

    /// Everything this card owns, discarded. A durable removal names a CARD,
    /// because that is what the authority removes.
    pub fn discard_card(&self, card: envoix_types::RecordId) {
        self.lock().retain(|key, _| key.card() != card);
    }

    /// Discards what this card bound for an acquisition older than the one its
    /// committed record now names.
    ///
    /// Driven by the ACQUISITION's generation, never the record's. The two are not the same —
    /// a resume advances the attempt generation and deliberately keeps the ready
    /// source, so cleaning up by the record's generation closed the descriptor
    /// the still-`Ready` card was about to send from. A re-pick is what mints a
    /// new acquisition, and only then is the old one dead.
    ///
    /// `None` — the card names no acquisition at all — discards NOTHING, and that
    /// is deliberate. Projections are observed asynchronously, so an update from
    /// before a pick can arrive after the descriptor for that pick was bound;
    /// treating "no source" as "close everything" would let a stale observation
    /// close a live handle. What it costs is a descriptor for an abandoned
    /// acquisition living until the next re-pick or the card's removal.
    ///
    /// A descriptor that arrives AFTER the acquisition advanced is likewise not
    /// caught here. Same bound, same reason: the alternative is the registry
    /// holding an authority answer it cannot read without racing the card actor.
    pub fn discard_superseded(&self, card: envoix_types::RecordId, current: Option<AttemptGen>) {
        let Some(current) = current else {
            return;
        };
        self.lock()
            .retain(|key, _| key.card() != card || key.generation() >= current);
    }

    /// Opens the bytes for one acquisition.
    ///
    /// A descriptor is opened by DUPLICATING it, so the registry keeps
    /// ownership: a staging run that consumed the descriptor would leave a
    /// second run — a restart, a resume — with nothing.
    ///
    /// A duplicate shares the file DESCRIPTION, and therefore the offset, which
    /// is why [`scan`] reads positionally. Sequential reads through this would
    /// leave a second run starting wherever the first stopped, and it would
    /// report the remainder as the whole file.
    fn open(&self, acquisition: &SourceAcquisitionKey) -> Option<File> {
        match self.lock().get(acquisition)? {
            BoundSource::Path(path) => File::open(path).ok(),
            BoundSource::Descriptor(owned) => owned.try_clone().ok().map(File::from),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SourceAcquisitionKey, BoundSource>> {
        self.sources.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reads the bound source through and reports what it contains.
#[derive(Clone, Default)]
pub struct BoundSourceStaging {
    registry: BoundSourceRegistry,
}

impl BoundSourceStaging {
    pub fn new(registry: BoundSourceRegistry) -> Self {
        Self { registry }
    }
}

impl SourceStagingExecutor for BoundSourceStaging {
    fn start(&self, plan: SourceStagingPlan) -> SourceStagingExecution {
        let (signals_tx, signals) = mpsc::channel(32);
        let (stop, token) = stop_channel();
        // Opened HERE, on the runtime's thread, so a card whose acquisition this
        // host does not hold fails immediately rather than after a task hop.
        //
        // A copy plan opens nothing. This worker reads through and writes
        // nothing, so it cannot produce the artifact a copy establishes, and it
        // does not claim one. Failing is the honest answer until the copy sink
        // exists; the alternative is what the possession split removed, a card
        // resting at `Ready` over an owned artifact that was never written.
        let source = match plan.plan {
            StagingPlan::ProviderStream => self.registry.open(&plan.acquisition),
            StagingPlan::CopyToOwnedArtifact => None,
        };
        tokio::task::spawn_blocking(move || read_through(source, &signals_tx, token));
        SourceStagingExecution { signals, stop }
    }
}

/// The read itself. Blocking on purpose: it is file I/O, and running it on the
/// async pool would stall every other card's actor for the length of a
/// multi-gigabyte file.
fn read_through(
    source: Option<File>,
    signals: &mpsc::Sender<SourceStagingSignal>,
    mut token: StopToken,
) {
    let outcome = match source {
        // Nothing to read: an acquisition this platform does not hold — a
        // descriptor never registered, or one discarded when the acquisition was
        // superseded — or a plan this worker cannot perform. Reading is what
        // failed, which is a different sentence to the acquisition failing, and
        // the reducer has both.
        None => SourceStagingSignal::Failed,
        Some(file) => scan(file, signals, &mut token),
    };
    let _ = signals.blocking_send(outcome);
    // Stopped LAST and always: it is what releases the handles and lets the
    // reducer's retirement be acknowledged, so a worker that skipped it would
    // leave the card retiring forever.
    let _ = signals.blocking_send(SourceStagingSignal::Stopped);
}

/// Reads the whole source POSITIONALLY and reports what it holds.
///
/// Positional (`pread`) rather than sequential, for two reasons that agree. The
/// handle may be a duplicate of a descriptor the platform opened, so it shares
/// an offset with every other duplicate; and the send path reads its source
/// positionally too (`SourceReader`), so staging and sending observe the file
/// the same way. A source that cannot serve a positional read fails here — that
/// is a sequential-only provider, whose plan is [`StagingPlan::CopyToOwnedArtifact`]
/// and whose copy sink is not built yet.
///
/// [`StagingPlan::CopyToOwnedArtifact`]: envoix_product::StagingPlan::CopyToOwnedArtifact
fn scan(
    file: File,
    signals: &mpsc::Sender<SourceStagingSignal>,
    token: &mut StopToken,
) -> SourceStagingSignal {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        if token.requested().is_some() {
            // A stop mid-read establishes nothing. Reporting a partial total as
            // if it were the file's length is exactly the "once observed a
            // length" lie the digest exists to prevent.
            return SourceStagingSignal::Failed;
        }
        match file.read_at(&mut buffer, total) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                total = total.saturating_add(read as u64);
                let _ = signals.blocking_send(SourceStagingSignal::Progress(ByteCount::new(total)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return SourceStagingSignal::Failed,
        }
    }
    SourceStagingSignal::Streamed {
        total: ByteCount::new(total),
        digest: ContentHash::from_bytes(*hasher.finalize().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use envoix_capabilities::{DutyProvenance, SourceAcquisitionKey};
    use envoix_types::{AttemptGen, RecordId, RequestId};

    use super::*;

    fn acquisition() -> SourceAcquisitionKey {
        acquisition_at(1)
    }

    fn scan_once(registry: &BoundSourceRegistry) -> SourceStagingSignal {
        let file = registry.open(&acquisition()).expect("the source opens");
        let (signals, _drain) = mpsc::channel(1024);
        let (_stop, mut token) = stop_channel();
        scan(file, &signals, &mut token)
    }

    fn acquisition_at(generation: u32) -> SourceAcquisitionKey {
        SourceAcquisitionKey::of(DutyProvenance {
            card: RecordId::new(7),
            generation: AttemptGen::new(generation),
            request: RequestId::from_bytes([9; 16]),
        })
    }

    fn a_descriptor() -> OwnedFd {
        OwnedFd::from(std::fs::File::open("/dev/null").expect("a descriptor"))
    }

    /// A resume must not close the source the card is about to send from.
    ///
    /// The attempt generation and the ACQUISITION's generation are different
    /// facts with different lifetimes: resume advances the first and
    /// deliberately keeps the ready source, while only a re-pick mints a new
    /// acquisition. Cleaning up by the record's generation — the same one the
    /// duty ledger uses to refuse a stale duty — closed the descriptor of a card
    /// whose lifecycle still named it.
    #[test]
    fn a_resumed_attempt_keeps_the_source_its_record_still_names() {
        let registry = BoundSourceRegistry::default();
        let bound = acquisition_at(1);
        registry.adopt_descriptor(bound, a_descriptor());

        // The card resumed: the record's generation moved on, the acquisition
        // did not, so the lifecycle still names `bound`.
        registry.discard_superseded(bound.card(), Some(bound.generation()));
        assert!(
            registry.open(&bound).is_some(),
            "a resume closed the source the card was about to send from"
        );

        // A re-pick DOES mint a new acquisition, and only then is the old one
        // dead — so the retention above is not passing because nothing is ever
        // discarded.
        let repicked = acquisition_at(2);
        registry.adopt_descriptor(repicked, a_descriptor());
        registry.discard_superseded(repicked.card(), Some(repicked.generation()));
        assert!(
            registry.open(&bound).is_none(),
            "a re-picked source was kept"
        );
        assert!(registry.open(&repicked).is_some());
    }

    /// A card that names no acquisition discards nothing.
    ///
    /// Projections are observed asynchronously, so an update from before a pick
    /// can arrive after that pick's descriptor was bound. Treating "no source"
    /// as "close everything" lets a stale observation close a live handle.
    #[test]
    fn an_observation_with_no_acquisition_closes_nothing() {
        let registry = BoundSourceRegistry::default();
        let bound = acquisition_at(1);
        registry.adopt_descriptor(bound, a_descriptor());

        registry.discard_superseded(bound.card(), None);

        assert!(
            registry.open(&bound).is_some(),
            "a stale observation closed a live descriptor"
        );
    }

    /// The first bind for an acquisition is the one that counts.
    ///
    /// Staging reads the source through and establishes a digest against one
    /// exact open file description. A later bind replacing it would leave the
    /// attempt reading a different document than the record vouches for, and
    /// nothing would say so — positional reads make concurrent offsets safe, not
    /// the identity of what is being read.
    #[test]
    fn a_second_bind_for_one_acquisition_is_refused() {
        let registry = BoundSourceRegistry::default();
        let bound = acquisition_at(1);

        assert!(registry.adopt_descriptor(bound, a_descriptor()));
        assert!(
            !registry.adopt_descriptor(bound, a_descriptor()),
            "a second document replaced the one staging established"
        );
    }

    /// A second staging run must observe the same file as the first.
    ///
    /// The registry hands out DUPLICATES of a platform descriptor, and a
    /// duplicate shares the file offset. Read sequentially, a stop-then-retry
    /// would resume at the first run's offset and report the remainder as the
    /// whole document — an `Established` with a short total and a digest of a
    /// suffix, which is worse than a failure because the card would rest at
    /// `Ready` believing it.
    #[test]
    fn a_repeated_scan_of_one_descriptor_establishes_the_same_bytes() {
        let mut file = tempfile::NamedTempFile::new().expect("a temp file");
        file.write_all(&vec![0xab_u8; READ_CHUNK_BYTES * 2 + 7])
            .expect("the source is written");
        let descriptor = OwnedFd::from(
            std::fs::File::open(file.path()).expect("the source opens for the platform"),
        );
        let registry = BoundSourceRegistry::default();
        registry.adopt_descriptor(acquisition(), descriptor);

        let first = scan_once(&registry);
        let second = scan_once(&registry);

        let SourceStagingSignal::Streamed { total, digest } = first else {
            panic!("the first scan did not establish the source: {first:?}");
        };
        assert_eq!(total.get(), (READ_CHUNK_BYTES * 2 + 7) as u64);
        assert!(
            matches!(
                second,
                SourceStagingSignal::Streamed { total: again, digest: same }
                    if again == total && same == digest
            ),
            "a second scan of the same descriptor saw different bytes: {second:?}"
        );
    }
}
