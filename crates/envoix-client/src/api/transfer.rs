//! The transfer handle and the adapters that feed its event stream.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use envoix_protocol::ManifestV1;
use envoix_session::{
    ManifestTransferEvent as SessionManifestEvent, TransferCancelToken, TransferDirection,
    TransferEvent as SessionEvent, TransferSummary,
};
use envoix_types::DataPath;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::error::{Phase, TransferError};
use super::{StampedEvent, TransferEvent};
use crate::PublicError;

/// Measured performance of one transfer, for a stats display or a campaign
/// ledger. Derived entirely from the event stream - no extra instrumentation in
/// the transfer engine - and final once the transfer has ended. Pairs with
/// [`TransferSummary`], which owns the transfer's identity and byte count;
/// this type is purely how *fast* and *by what path*.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransferStats {
    /// Wall-clock transfer duration (started -> completed), in milliseconds.
    pub duration_ms: u64,
    /// Average throughput over the transfer, in bytes per second.
    pub avg_bytes_per_sec: u64,
    /// Peak throughput between two progress samples, in bytes per second.
    pub peak_bytes_per_sec: u64,
    /// Time from *beginning to connect* (pairing done / the `Connecting` event)
    /// to the first data path being selected - so it excludes any wait for the
    /// peer to arrive. `None` if it never connected.
    pub connect_latency_ms: Option<u64>,
    /// Every data path used, in order (e.g. a relay path then its direct
    /// upgrade), so the full path history is visible, not only the final one.
    pub paths: Vec<DataPath>,
}

/// Accumulates [`TransferStats`] as events flow, behind a shared handle so the
/// emitting task and the reading [`Transfer`] observe the same running totals.
#[derive(Clone, Debug)]
pub(crate) struct StatsHandle(Arc<Mutex<StatsInner>>);

#[derive(Default, Debug)]
struct StatsInner {
    connect_start_ms: Option<u64>,
    connected_ms: Option<u64>,
    started_ms: Option<u64>,
    end_ms: Option<u64>,
    bytes: u64,
    last_progress: Option<(u64, u64)>,
    peak_bps: u64,
    paths: Vec<DataPath>,
}

impl StatsHandle {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(StatsInner::default())))
    }

    /// Fold one stamped event into the running stats.
    fn observe(&self, ts_ms: u64, event: &TransferEvent) {
        let mut s = self.0.lock().expect("stats mutex");
        match event {
            TransferEvent::Diagnostic { .. } => {}
            // The connect window opens once pairing is done and we begin
            // connecting; the pairing steps and the Connecting event both mark
            // it, the latest wins - so a room receiver's peer-wait is excluded.
            TransferEvent::Pairing { .. } | TransferEvent::Connecting => {
                s.connect_start_ms = Some(ts_ms);
            }
            TransferEvent::Connected { path } => {
                s.connected_ms.get_or_insert(ts_ms);
                s.paths.push(path.clone());
            }
            TransferEvent::PathChanged { path } => s.paths.push(path.clone()),
            TransferEvent::Started { .. } | TransferEvent::ManifestStarted { .. } => {
                s.started_ms.get_or_insert(ts_ms);
            }
            TransferEvent::Progress {
                bytes_transferred, ..
            } => observe_progress(&mut s, ts_ms, *bytes_transferred),
            TransferEvent::ManifestProgress {
                completed_bytes, ..
            } => observe_progress(&mut s, ts_ms, *completed_bytes),
            TransferEvent::Completed {
                bytes_transferred, ..
            } => {
                s.bytes = *bytes_transferred;
                s.end_ms.get_or_insert(ts_ms);
            }
            TransferEvent::ManifestCompleted { total_bytes, .. } => {
                s.bytes = *total_bytes;
                s.end_ms.get_or_insert(ts_ms);
            }
            _ => {}
        }
    }

    /// A snapshot of the stats so far (final once the transfer has ended).
    pub(crate) fn snapshot(&self) -> TransferStats {
        let s = self.0.lock().expect("stats mutex");
        let duration_ms = match (s.started_ms, s.end_ms) {
            (Some(start), Some(end)) => end.saturating_sub(start),
            _ => 0,
        };
        TransferStats {
            duration_ms,
            avg_bytes_per_sec: s
                .bytes
                .saturating_mul(1000)
                .checked_div(duration_ms)
                .unwrap_or(0),
            peak_bytes_per_sec: s.peak_bps,
            connect_latency_ms: match (s.connect_start_ms, s.connected_ms) {
                (Some(start), Some(connected)) => Some(connected.saturating_sub(start)),
                _ => None,
            },
            paths: s.paths.clone(),
        }
    }

    /// Whether a `Connected` event has been observed - i.e. the transfer
    /// reached a live peer connection. The fallback loop reads this to tell a
    /// pre-connection failure (retry the next source) from a mid-transfer one.
    pub(crate) fn connected(&self) -> bool {
        self.0.lock().expect("stats mutex").connected_ms.is_some()
    }
}

fn observe_progress(stats: &mut StatsInner, ts_ms: u64, bytes: u64) {
    if let Some((last_ms, last_bytes)) = stats.last_progress
        && ts_ms > last_ms
        && bytes >= last_bytes
    {
        let bps = (bytes - last_bytes).saturating_mul(1000) / (ts_ms - last_ms);
        stats.peak_bps = stats.peak_bps.max(bps);
    }
    stats.bytes = bytes;
    stats.last_progress = Some((ts_ms, bytes));
}

/// Lock-free cell holding the phase the transfer has reached, updated as
/// lifecycle events flow through the [`EventSender`].
#[derive(Debug)]
pub(crate) struct PhaseCell(AtomicU8);

impl PhaseCell {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU8::new(Phase::Setup as u8)))
    }

    fn store(&self, phase: Phase) {
        self.0.store(phase as u8, Ordering::Relaxed);
    }

    fn load(&self) -> Phase {
        match self.0.load(Ordering::Relaxed) {
            x if x == Phase::Waiting as u8 => Phase::Waiting,
            x if x == Phase::Pairing as u8 => Phase::Pairing,
            x if x == Phase::Connecting as u8 => Phase::Connecting,
            x if x == Phase::Transfer as u8 => Phase::Transfer,
            _ => Phase::Setup,
        }
    }
}

/// The phase a transfer is in once `event` has been observed.
fn phase_of(event: &TransferEvent) -> Phase {
    match event {
        TransferEvent::Diagnostic { .. } => Phase::Setup,
        TransferEvent::Binding { .. } => Phase::Setup,
        TransferEvent::Advertised { .. } => Phase::Waiting,
        TransferEvent::Pairing { .. } => Phase::Pairing,
        TransferEvent::Connecting
        | TransferEvent::Connected { .. }
        | TransferEvent::PathChanged { .. } => Phase::Connecting,
        TransferEvent::Started { .. }
        | TransferEvent::Progress { .. }
        | TransferEvent::Verifying { .. }
        | TransferEvent::Verified { .. }
        | TransferEvent::Confirming { .. }
        | TransferEvent::Completed { .. }
        | TransferEvent::ManifestPreparingEntry { .. }
        | TransferEvent::ManifestPlanned { .. }
        | TransferEvent::ManifestStarted { .. }
        | TransferEvent::ManifestEntryStarted { .. }
        | TransferEvent::ManifestProgress { .. }
        | TransferEvent::ManifestEntryCompleted { .. }
        | TransferEvent::ManifestCompleted { .. }
        | TransferEvent::Failed { .. } => Phase::Transfer,
    }
}

#[derive(Debug)]
struct TransferHandle<R> {
    events: mpsc::UnboundedReceiver<StampedEvent>,
    cancel: TransferCancelToken,
    phase: Arc<PhaseCell>,
    stats: StatsHandle,
    // Option only so `wait(self)` can move it out despite the Drop impl.
    task: Option<JoinHandle<Result<R, PublicError>>>,
}

impl<R> TransferHandle<R> {
    fn new(
        events: mpsc::UnboundedReceiver<StampedEvent>,
        cancel: TransferCancelToken,
        phase: Arc<PhaseCell>,
        stats: StatsHandle,
        task: JoinHandle<Result<R, PublicError>>,
    ) -> Self {
        Self {
            events,
            cancel,
            phase,
            stats,
            task: Some(task),
        }
    }

    fn phase(&self) -> Phase {
        self.phase.load()
    }

    fn stats(&self) -> TransferStats {
        self.stats.snapshot()
    }

    async fn next_event(&mut self) -> Option<StampedEvent> {
        self.events.recv().await
    }

    fn cancel(&self) {
        self.cancel.cancel();
    }

    fn pause(&self) {
        self.cancel.pause();
    }

    fn cancel_handle(&self) -> TransferCancelToken {
        self.cancel.clone()
    }

    async fn cancel_and_join(mut self) {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return;
        };
        if tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }

    fn detach(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    async fn wait(mut self) -> Result<R, TransferError> {
        let task = self.task.take().expect("wait consumes the handle");
        let result = match task.await {
            Ok(result) => result,
            Err(error) => Err(PublicError::Transfer(format!(
                "transfer task failed: {error}"
            ))),
        };
        result.map_err(|error| TransferError::from_core(error, self.phase.load()))
    }
}

impl<R> Drop for TransferHandle<R> {
    fn drop(&mut self) {
        // Only a still-owned attempt is interrupted; `wait` (finished) and
        // `detach` (deliberately silent) take the task out first.
        if self.task.is_some() {
            self.cancel.cancel();
        }
    }
}

/// A running compatible single-file transfer: observe it through
/// [`Transfer::next_event`], stop it with [`Transfer::cancel`], and take its
/// result with [`Transfer::wait`].
///
/// Dropping the handle cancels the transfer (the peer is notified where the
/// protocol allows it).
#[derive(Debug)]
pub struct Transfer {
    inner: TransferHandle<TransferSummary>,
}

impl Transfer {
    pub(crate) fn new(
        events: mpsc::UnboundedReceiver<StampedEvent>,
        cancel: TransferCancelToken,
        phase: Arc<PhaseCell>,
        stats: StatsHandle,
        task: JoinHandle<Result<TransferSummary, PublicError>>,
    ) -> Self {
        Self {
            inner: TransferHandle::new(events, cancel, phase, stats, task),
        }
    }

    /// The phase this transfer has reached (per its last lifecycle event).
    pub fn phase(&self) -> Phase {
        self.inner.phase()
    }

    /// A snapshot of measured throughput, connect latency, and path history.
    pub fn stats(&self) -> TransferStats {
        self.inner.stats()
    }

    /// The next lifecycle event, or `None` after the event stream ends.
    pub async fn next_event(&mut self) -> Option<StampedEvent> {
        self.inner.next_event().await
    }

    /// Requests a graceful cancel.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Requests a resumable pause.
    pub fn pause(&self) {
        self.inner.pause();
    }

    /// Returns a clonable cancel handle for another task or thread.
    pub fn cancel_handle(&self) -> TransferCancelToken {
        self.inner.cancel_handle()
    }

    /// Cancels the attempt and waits for its write-side cleanup to finish.
    pub(crate) async fn cancel_and_join(self) {
        self.inner.cancel_and_join().await;
    }

    /// Aborts as an infrastructure loss without sending a user interrupt.
    pub(crate) fn detach(self) {
        self.inner.detach();
    }

    /// Waits for the compatible single-file result.
    pub async fn wait(self) -> Result<TransferSummary, TransferError> {
        self.inner.wait().await
    }
}

/// A running additive transfer whose negotiated result may be either one
/// compatible single file or a Manifest transfer set.
#[derive(Debug)]
pub struct TransferSet {
    inner: TransferHandle<envoix_session::SessionTransferSummary>,
}

impl TransferSet {
    pub(crate) fn new(
        events: mpsc::UnboundedReceiver<StampedEvent>,
        cancel: TransferCancelToken,
        phase: Arc<PhaseCell>,
        stats: StatsHandle,
        task: JoinHandle<Result<envoix_session::SessionTransferSummary, PublicError>>,
    ) -> Self {
        Self {
            inner: TransferHandle::new(events, cancel, phase, stats, task),
        }
    }

    /// The phase this transfer has reached (per its last lifecycle event).
    pub fn phase(&self) -> Phase {
        self.inner.phase()
    }

    /// A snapshot of measured throughput, connect latency, and path history.
    pub fn stats(&self) -> TransferStats {
        self.inner.stats()
    }

    /// The next lifecycle event, or `None` after the event stream ends.
    pub async fn next_event(&mut self) -> Option<StampedEvent> {
        self.inner.next_event().await
    }

    /// Requests a graceful cancel.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Requests a resumable pause.
    pub fn pause(&self) {
        self.inner.pause();
    }

    /// Returns a clonable cancel handle for another task or thread.
    pub fn cancel_handle(&self) -> TransferCancelToken {
        self.inner.cancel_handle()
    }

    /// Waits for the negotiated single-file or Manifest result.
    pub async fn wait(self) -> Result<envoix_session::SessionTransferSummary, TransferError> {
        self.inner.wait().await
    }
}

/// Emits lifecycle events the legacy layers have no hook for (binding,
/// advertising, pairing), and is cloned into the adapter sinks.
#[derive(Clone, Debug)]
pub(crate) struct EventSender {
    sender: mpsc::UnboundedSender<StampedEvent>,
    phase: Arc<PhaseCell>,
    stats: StatsHandle,
}

impl EventSender {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<StampedEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                sender,
                phase: PhaseCell::new(),
                stats: StatsHandle::new(),
            },
            receiver,
        )
    }

    /// The cell tracking the phase implied by the events sent so far.
    pub(crate) fn phase_cell(&self) -> Arc<PhaseCell> {
        self.phase.clone()
    }

    /// The handle accumulating this transfer's stats from the event stream.
    pub(crate) fn stats_handle(&self) -> StatsHandle {
        self.stats.clone()
    }

    /// Sends one event, stamped with the emission time and advancing the
    /// tracked phase and stats; silently dropped when the handle is gone.
    pub(crate) fn emit(&self, event: TransferEvent) {
        self.phase.store(phase_of(&event));
        let ts_ms = unix_now_ms();
        self.stats.observe(ts_ms, &event);
        // Fold the transfer id onto the ambient transfer span the first time it
        // is known, so subsequent client log lines correlate with the peer's.
        match &event {
            TransferEvent::Started { transfer_id, .. } => {
                tracing::Span::current()
                    .record("transfer_id", tracing::field::display(transfer_id));
            }
            TransferEvent::ManifestStarted { manifest_id, .. } => {
                tracing::Span::current()
                    .record("transfer_id", tracing::field::display(manifest_id));
            }
            _ => {}
        }
        let _ = self.sender.send(StampedEvent { ts_ms, event });
    }
}

/// Current Unix time in whole milliseconds.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Adapts the session-layer event sink onto the unified client stream.
pub(crate) struct SessionEventAdapter(pub(crate) EventSender);

impl envoix_session::EventSink for SessionEventAdapter {
    fn on_event(&self, event: SessionEvent) {
        self.0.emit(event.into());
    }
}

impl envoix_session::ManifestEventSink for SessionEventAdapter {
    fn on_manifest_plan(&self, direction: TransferDirection, manifest: &ManifestV1) {
        self.0.emit(TransferEvent::ManifestPlanned {
            direction,
            manifest: manifest.clone(),
        });
    }

    fn on_manifest_event(&self, event: SessionManifestEvent) {
        self.0.emit(event.into());
    }
}

impl From<SessionManifestEvent> for TransferEvent {
    fn from(event: SessionManifestEvent) -> Self {
        match event {
            SessionManifestEvent::PreparingEntry {
                manifest_id,
                entry_id,
                relative_path,
                size,
            } => Self::ManifestPreparingEntry {
                manifest_id,
                entry_id,
                relative_path,
                size,
            },
            SessionManifestEvent::Started {
                manifest_id,
                direction,
                file_count,
                directory_count,
                total_bytes,
            } => Self::ManifestStarted {
                manifest_id,
                direction,
                file_count,
                directory_count,
                total_bytes,
            },
            SessionManifestEvent::EntryStarted {
                manifest_id,
                entry_id,
                transfer_id,
                relative_path,
                total_bytes,
                bytes_resumed,
            } => Self::ManifestEntryStarted {
                manifest_id,
                entry_id,
                transfer_id,
                relative_path,
                total_bytes,
                bytes_resumed,
            },
            SessionManifestEvent::Progress {
                manifest_id,
                entry_id,
                entry_bytes,
                entry_total_bytes,
                completed_bytes,
                total_bytes,
            } => Self::ManifestProgress {
                manifest_id,
                entry_id,
                entry_bytes,
                entry_total_bytes,
                completed_bytes,
                total_bytes,
            },
            SessionManifestEvent::EntryCompleted {
                manifest_id,
                result,
            } => Self::ManifestEntryCompleted {
                manifest_id,
                result,
            },
            SessionManifestEvent::Completed { summary } => Self::ManifestCompleted {
                manifest_id: summary.manifest_id,
                file_count: summary.file_count,
                directory_count: summary.directory_count,
                total_bytes: summary.total_bytes,
                entries: summary.entries,
            },
        }
    }
}

/// Maps a session-layer event onto the unified client event vocabulary.
impl From<SessionEvent> for TransferEvent {
    fn from(event: SessionEvent) -> Self {
        match event {
            SessionEvent::Diagnostic { message } => TransferEvent::Diagnostic { message },
            SessionEvent::Started {
                transfer_id,
                direction,
                file_name,
                total_bytes,
                bytes_resumed,
            } => TransferEvent::Started {
                transfer_id,
                direction,
                file_name,
                total_bytes,
                bytes_resumed,
            },
            SessionEvent::Progress {
                transfer_id,
                bytes_transferred,
                total_bytes,
            } => TransferEvent::Progress {
                transfer_id,
                bytes_transferred,
                total_bytes,
            },
            SessionEvent::HashStarted {
                transfer_id,
                direction,
                file_name,
                bytes_to_hash,
            } => TransferEvent::Verifying {
                transfer_id,
                direction,
                file_name,
                bytes_to_hash,
            },
            SessionEvent::HashCompleted {
                transfer_id,
                direction,
                file_name,
                bytes_hashed,
            } => TransferEvent::Verified {
                transfer_id,
                direction,
                file_name,
                bytes_hashed,
            },
            SessionEvent::Confirming {
                transfer_id,
                file_hash,
            } => TransferEvent::Confirming {
                transfer_id,
                file_hash,
            },
            SessionEvent::Completed {
                transfer_id,
                file_name,
                bytes_transferred,
            } => TransferEvent::Completed {
                transfer_id,
                file_name,
                bytes_transferred,
            },
            SessionEvent::Failed { direction, reason } => {
                let reason_code = super::event::SessionFailureCode::classify(&reason);
                TransferEvent::Failed {
                    direction,
                    reason,
                    reason_code,
                }
            }
            SessionEvent::Pairing { step } => TransferEvent::Pairing { step },
            SessionEvent::Connecting => TransferEvent::Connecting,
            SessionEvent::Connected { path } => TransferEvent::Connected { path },
            SessionEvent::PathChanged { path } => TransferEvent::PathChanged { path },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoix_protocol::{
        ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryResultV1, ManifestEntryV1,
        ManifestHashAlgorithm, ManifestId,
    };
    use envoix_session::{
        EventSink as _, ManifestEventSink as _, ManifestTransferSummary, TransferDirection,
    };
    use envoix_types::TransferId;

    #[tokio::test]
    async fn detach_aborts_the_task_and_says_nothing() {
        let (_events_tx, events) = mpsc::unbounded_channel();
        let cancel = TransferCancelToken::new();
        // The task holds `alive_tx`; an abort drops it without sending.
        let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _keep = alive_tx;
            std::future::pending::<Result<TransferSummary, PublicError>>().await
        });
        let transfer = Transfer::new(
            events,
            cancel.clone(),
            PhaseCell::new(),
            StatsHandle::new(),
            task,
        );

        transfer.detach();

        alive_rx.await.unwrap_err(); // aborted, not completed
        assert!(
            !cancel.is_cancelled(),
            "detach is not a user intent: the interrupt token must stay untouched \
             (a triggered token sends an interrupt frame the peer reads as cancel)"
        );
    }

    #[tokio::test]
    async fn cancel_and_join_fires_the_token_and_waits_for_the_task() {
        let (_events_tx, events) = mpsc::unbounded_channel();
        let cancel = TransferCancelToken::new();
        // A cooperative engine: ends as soon as the token fires.
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            token.cancelled().await;
            Err(PublicError::Transfer("cancelled".into()))
        });
        let transfer = Transfer::new(
            events,
            cancel.clone(),
            PhaseCell::new(),
            StatsHandle::new(),
            task,
        );

        transfer.cancel_and_join().await;

        assert!(cancel.is_cancelled(), "discard is an explicit user intent");
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_and_join_aborts_a_wedged_task() {
        let (_events_tx, events) = mpsc::unbounded_channel();
        let cancel = TransferCancelToken::new();
        // A wedged engine: ignores the token (e.g. an unbounded await).
        let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _keep = alive_tx;
            std::future::pending::<Result<TransferSummary, PublicError>>().await
        });
        let transfer = Transfer::new(
            events,
            cancel.clone(),
            PhaseCell::new(),
            StatsHandle::new(),
            task,
        );

        transfer.cancel_and_join().await; // paused clock: grace elapses instantly

        alive_rx.await.unwrap_err(); // the wedged task was aborted, not leaked
    }

    #[tokio::test]
    async fn plain_drop_still_cancels_a_live_attempt() {
        let (_events_tx, events) = mpsc::unbounded_channel();
        let cancel = TransferCancelToken::new();
        let task = tokio::spawn(async {
            std::future::pending::<Result<TransferSummary, PublicError>>().await
        });
        let transfer = Transfer::new(
            events,
            cancel.clone(),
            PhaseCell::new(),
            StatsHandle::new(),
            task,
        );

        drop(transfer);

        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn session_adapter_maps_legacy_events() {
        let (sender, mut receiver) = EventSender::channel();
        let adapter = SessionEventAdapter(sender);

        adapter.on_event(SessionEvent::HashStarted {
            transfer_id: TransferId::new("t1"),
            direction: TransferDirection::Receive,
            file_name: "a.bin".into(),
            bytes_to_hash: 42,
        });
        adapter.on_event(SessionEvent::Completed {
            transfer_id: TransferId::new("t1"),
            file_name: "a.bin".into(),
            bytes_transferred: 42,
        });

        let first = receiver.recv().await.unwrap();
        assert!(first.ts_ms > 0);
        assert_eq!(
            first.event,
            TransferEvent::Verifying {
                transfer_id: TransferId::new("t1"),
                direction: TransferDirection::Receive,
                file_name: "a.bin".into(),
                bytes_to_hash: 42,
            }
        );
        assert_eq!(
            receiver.recv().await.unwrap().event,
            TransferEvent::Completed {
                transfer_id: TransferId::new("t1"),
                file_name: "a.bin".into(),
                bytes_transferred: 42,
            }
        );
    }

    #[tokio::test]
    async fn session_adapter_maps_every_manifest_event() {
        let (sender, mut receiver) = EventSender::channel();
        let adapter = SessionEventAdapter(sender);
        let manifest_id = ManifestId::new("manifest-client-events");
        let transfer_id = TransferId::new("manifest-client-events:1");
        let manifest = ManifestV1 {
            manifest_id: manifest_id.clone(),
            entries: vec![
                ManifestEntryV1 {
                    entry_id: 0,
                    relative_path: "album".into(),
                    kind: ManifestEntryKind::Directory,
                    size: 0,
                    hash: None,
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 1,
                    relative_path: "album/photo.jpg".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: 120,
                    hash: Some([7; 32]),
                    modified_at_unix_ms: None,
                },
            ],
            file_count: 1,
            directory_count: 1,
            root_count: 1,
            total_bytes: 120,
            hash_algorithm: ManifestHashAlgorithm::Blake3_256,
        };
        let result = ManifestEntryResultV1 {
            entry_id: 1,
            status: ManifestEntryResultStatus::Completed,
            offered_relative_path: "album/photo.jpg".into(),
            final_relative_path: Some("album/photo.jpg".into()),
            failure_code: None,
        };

        adapter.on_manifest_plan(TransferDirection::Receive, &manifest);

        for event in [
            SessionManifestEvent::PreparingEntry {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                relative_path: "album/photo.jpg".into(),
                size: 120,
            },
            SessionManifestEvent::Started {
                manifest_id: manifest_id.clone(),
                direction: TransferDirection::Receive,
                file_count: 1,
                directory_count: 1,
                total_bytes: 120,
            },
            SessionManifestEvent::EntryStarted {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                transfer_id: transfer_id.clone(),
                relative_path: "album/photo.jpg".into(),
                total_bytes: 120,
                bytes_resumed: 20,
            },
            SessionManifestEvent::Progress {
                manifest_id: manifest_id.clone(),
                entry_id: 1,
                entry_bytes: 70,
                entry_total_bytes: 120,
                completed_bytes: 70,
                total_bytes: 120,
            },
            SessionManifestEvent::EntryCompleted {
                manifest_id: manifest_id.clone(),
                result: result.clone(),
            },
            SessionManifestEvent::Completed {
                summary: ManifestTransferSummary {
                    manifest_id: manifest_id.clone(),
                    file_count: 1,
                    directory_count: 1,
                    total_bytes: 120,
                    entries: vec![result.clone()],
                },
            },
        ] {
            adapter.on_manifest_event(event);
        }

        let mut actual = Vec::new();
        for _ in 0..7 {
            actual.push(receiver.recv().await.unwrap().event);
        }
        assert_eq!(
            actual,
            vec![
                TransferEvent::ManifestPlanned {
                    direction: TransferDirection::Receive,
                    manifest: manifest.clone(),
                },
                TransferEvent::ManifestPreparingEntry {
                    manifest_id: manifest_id.clone(),
                    entry_id: 1,
                    relative_path: "album/photo.jpg".into(),
                    size: 120,
                },
                TransferEvent::ManifestStarted {
                    manifest_id: manifest_id.clone(),
                    direction: TransferDirection::Receive,
                    file_count: 1,
                    directory_count: 1,
                    total_bytes: 120,
                },
                TransferEvent::ManifestEntryStarted {
                    manifest_id: manifest_id.clone(),
                    entry_id: 1,
                    transfer_id,
                    relative_path: "album/photo.jpg".into(),
                    total_bytes: 120,
                    bytes_resumed: 20,
                },
                TransferEvent::ManifestProgress {
                    manifest_id: manifest_id.clone(),
                    entry_id: 1,
                    entry_bytes: 70,
                    entry_total_bytes: 120,
                    completed_bytes: 70,
                    total_bytes: 120,
                },
                TransferEvent::ManifestEntryCompleted {
                    manifest_id: manifest_id.clone(),
                    result: result.clone(),
                },
                TransferEvent::ManifestCompleted {
                    manifest_id,
                    file_count: 1,
                    directory_count: 1,
                    total_bytes: 120,
                    entries: vec![result],
                },
            ]
        );
    }

    #[tokio::test]
    async fn channel_closes_when_all_senders_drop() {
        let (sender, mut receiver) = EventSender::channel();
        sender.emit(TransferEvent::Pairing {
            step: envoix_types::PairingStep::Joining,
        });
        drop(sender);

        assert_eq!(
            receiver.recv().await.unwrap().event,
            TransferEvent::Pairing {
                step: envoix_types::PairingStep::Joining,
            }
        );
        assert!(receiver.recv().await.is_none());
    }

    #[test]
    fn stats_accumulate_from_the_event_stream() {
        use super::super::TransferMode;
        let addr = "1.2.3.4:5".parse().unwrap();
        let h = StatsHandle::new();
        h.observe(
            1000,
            &TransferEvent::Binding {
                direction: TransferDirection::Send,
                mode: TransferMode::Room,
            },
        );
        h.observe(1100, &TransferEvent::Connecting);
        h.observe(
            1200,
            &TransferEvent::Connected {
                path: DataPath::Relay { url: "r".into() },
            },
        );
        h.observe(
            1300,
            &TransferEvent::Started {
                transfer_id: TransferId::new("t"),
                direction: TransferDirection::Send,
                file_name: "f".into(),
                total_bytes: 1000,
                bytes_resumed: 0,
            },
        );
        let progress = |bytes| TransferEvent::Progress {
            transfer_id: TransferId::new("t"),
            bytes_transferred: bytes,
            total_bytes: 1000,
        };
        h.observe(1300, &progress(0));
        h.observe(
            1400,
            &TransferEvent::PathChanged {
                path: DataPath::Direct { addr },
            },
        );
        h.observe(1400, &progress(1000)); // 1000 B in 100 ms -> 10_000 B/s peak
        h.observe(
            1800,
            &TransferEvent::Completed {
                transfer_id: TransferId::new("t"),
                file_name: "f".into(),
                bytes_transferred: 1000,
            },
        );

        let stats = h.snapshot();
        assert_eq!(stats.duration_ms, 500); // started 1300 -> completed 1800
        assert_eq!(stats.avg_bytes_per_sec, 2000); // 1000 * 1000 / 500
        assert_eq!(stats.peak_bytes_per_sec, 10_000);
        assert_eq!(stats.connect_latency_ms, Some(100)); // connecting 1100 -> connected 1200
        assert_eq!(
            stats.paths,
            vec![
                DataPath::Relay { url: "r".into() },
                DataPath::Direct { addr }
            ]
        );
    }

    #[test]
    fn stats_use_aggregate_manifest_progress() {
        let manifest_id = ManifestId::new("manifest-stats");
        let h = StatsHandle::new();
        h.observe(
            1_000,
            &TransferEvent::ManifestStarted {
                manifest_id: manifest_id.clone(),
                direction: TransferDirection::Send,
                file_count: 2,
                directory_count: 1,
                total_bytes: 150,
            },
        );
        let progress = |completed_bytes| TransferEvent::ManifestProgress {
            manifest_id: manifest_id.clone(),
            entry_id: 1,
            entry_bytes: completed_bytes,
            entry_total_bytes: 150,
            completed_bytes,
            total_bytes: 150,
        };
        h.observe(1_100, &progress(50));
        h.observe(1_200, &progress(150));
        h.observe(
            1_400,
            &TransferEvent::ManifestCompleted {
                manifest_id,
                file_count: 2,
                directory_count: 1,
                total_bytes: 150,
                entries: Vec::new(),
            },
        );

        let stats = h.snapshot();
        assert_eq!(stats.duration_ms, 400);
        assert_eq!(stats.avg_bytes_per_sec, 375);
        assert_eq!(stats.peak_bytes_per_sec, 1_000);
    }
}
