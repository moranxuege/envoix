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
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use envoix_runtime::{
    ContentHash, SourceAcquisitionKey, SourceStagingExecution, SourceStagingExecutor,
    SourceStagingPlan, SourceStagingSignal, StopToken, stop_channel,
};
use envoix_types::ByteCount;
use tokio::sync::mpsc;

/// How much is read between progress reports.
///
/// Progress is an observation, not a durability boundary: the reducer keeps the
/// newest count and the lane coalesces, so this trades report frequency against
/// syscalls rather than against correctness.
const READ_CHUNK_BYTES: usize = 256 * 1024;

/// This host's source registry: which path a given acquisition owns.
///
/// Keyed by the WHOLE acquisition, exactly as Android's is. A registry keyed by
/// card would let a later generation inherit an earlier one's document, which is
/// the ownership defect the key exists to close.
#[derive(Clone, Default)]
pub struct BoundSourceRegistry {
    sources: Arc<Mutex<HashMap<SourceAcquisitionKey, PathBuf>>>,
}

impl BoundSourceRegistry {
    /// Binds a path to one acquisition. The authority publishes the key; this is
    /// the local answer to it.
    pub fn bind(&self, acquisition: SourceAcquisitionKey, path: PathBuf) {
        self.lock().insert(acquisition, path);
    }

    fn path(&self, acquisition: &SourceAcquisitionKey) -> Option<PathBuf> {
        self.lock().get(acquisition).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SourceAcquisitionKey, PathBuf>> {
        self.sources.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reads the bound file through and reports what it contains.
#[derive(Clone, Default)]
pub struct FileSourceStaging {
    registry: BoundSourceRegistry,
}

impl FileSourceStaging {
    pub fn new(registry: BoundSourceRegistry) -> Self {
        Self { registry }
    }
}

impl SourceStagingExecutor for FileSourceStaging {
    fn start(&self, plan: SourceStagingPlan) -> SourceStagingExecution {
        let (signals_tx, signals) = mpsc::channel(32);
        let (stop, token) = stop_channel();
        let path = self.registry.path(&plan.acquisition);
        tokio::task::spawn_blocking(move || read_through(path, &signals_tx, token));
        SourceStagingExecution { signals, stop }
    }
}

/// The read itself. Blocking on purpose: it is file I/O, and running it on the
/// async pool would stall every other card's actor for the length of a
/// multi-gigabyte file.
fn read_through(
    path: Option<PathBuf>,
    signals: &mpsc::Sender<SourceStagingSignal>,
    mut token: StopToken,
) {
    let outcome = match path {
        // The authority commissioned work for an acquisition this platform does
        // not hold. Reading is what failed, which is a different sentence to the
        // acquisition failing — the reducer has both.
        None => SourceStagingSignal::Failed,
        Some(path) => scan(&path, signals, &mut token),
    };
    let _ = signals.blocking_send(outcome);
    // Stopped LAST and always: it is what releases the handles and lets the
    // reducer's retirement be acknowledged, so a worker that skipped it would
    // leave the card retiring forever.
    let _ = signals.blocking_send(SourceStagingSignal::Stopped);
}

fn scan(
    path: &PathBuf,
    signals: &mpsc::Sender<SourceStagingSignal>,
    token: &mut StopToken,
) -> SourceStagingSignal {
    let Ok(mut file) = std::fs::File::open(path) else {
        return SourceStagingSignal::Failed;
    };
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
        match file.read(&mut buffer) {
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
    SourceStagingSignal::Established {
        total: ByteCount::new(total),
        digest: ContentHash::from_bytes(*hasher.finalize().as_bytes()),
    }
}
