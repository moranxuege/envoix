//! The transfer handle and the adapters that feed its event stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use envoix_session::{TransferCancelToken, TransferEvent as SessionEvent, TransferSummary};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::error::{Phase, TransferError};
use super::{StampedEvent, TransferEvent};
use crate::PublicError;

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
        TransferEvent::Binding { .. } => Phase::Setup,
        TransferEvent::Advertised { .. } => Phase::Waiting,
        TransferEvent::Pairing => Phase::Pairing,
        TransferEvent::Connecting
        | TransferEvent::Connected { .. }
        | TransferEvent::PathChanged { .. } => Phase::Connecting,
        TransferEvent::Started { .. }
        | TransferEvent::Progress { .. }
        | TransferEvent::Verifying { .. }
        | TransferEvent::Verified { .. }
        | TransferEvent::Completed { .. }
        | TransferEvent::Failed { .. } => Phase::Transfer,
    }
}

/// A running transfer: observe it through [`Transfer::next_event`], stop it
/// with [`Transfer::cancel`], and take its result with [`Transfer::wait`].
///
/// Dropping the handle cancels the transfer (the peer is notified where the
/// protocol allows it).
#[derive(Debug)]
pub struct Transfer {
    events: mpsc::UnboundedReceiver<StampedEvent>,
    cancel: TransferCancelToken,
    phase: Arc<PhaseCell>,
    // Option only so `wait(self)` can move it out despite the Drop impl.
    task: Option<JoinHandle<Result<TransferSummary, PublicError>>>,
}

impl Transfer {
    pub(crate) fn new(
        events: mpsc::UnboundedReceiver<StampedEvent>,
        cancel: TransferCancelToken,
        phase: Arc<PhaseCell>,
        task: JoinHandle<Result<TransferSummary, PublicError>>,
    ) -> Self {
        Self {
            events,
            cancel,
            phase,
            task: Some(task),
        }
    }

    /// The phase this transfer has reached (per its last lifecycle event).
    pub fn phase(&self) -> Phase {
        self.phase.load()
    }

    /// The next lifecycle event (with its emission time), or `None` once the
    /// transfer has ended and all events were consumed.
    pub async fn next_event(&mut self) -> Option<StampedEvent> {
        self.events.recv().await
    }

    /// Requests a graceful stop; the transfer ends with a cancellation error.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Waits for the transfer to finish and returns its outcome. Events not
    /// yet consumed are discarded.
    pub async fn wait(mut self) -> Result<TransferSummary, TransferError> {
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

impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Emits lifecycle events the legacy layers have no hook for (binding,
/// advertising, pairing), and is cloned into the adapter sinks.
#[derive(Clone, Debug)]
pub(crate) struct EventSender {
    sender: mpsc::UnboundedSender<StampedEvent>,
    phase: Arc<PhaseCell>,
}

impl EventSender {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<StampedEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                sender,
                phase: PhaseCell::new(),
            },
            receiver,
        )
    }

    /// The cell tracking the phase implied by the events sent so far.
    pub(crate) fn phase_cell(&self) -> Arc<PhaseCell> {
        self.phase.clone()
    }

    /// Sends one event, stamped with the emission time and advancing the
    /// tracked phase; silently dropped when the handle is gone.
    pub(crate) fn emit(&self, event: TransferEvent) {
        self.phase.store(phase_of(&event));
        let _ = self.sender.send(StampedEvent {
            ts_ms: unix_now_ms(),
            event,
        });
    }
}

/// Current Unix time in whole milliseconds.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Adapts the legacy transfer-progress sink onto the unified stream.
pub(crate) struct SessionEventAdapter(pub(crate) EventSender);

impl envoix_session::EventSink for SessionEventAdapter {
    fn on_event(&self, event: SessionEvent) {
        self.0.emit(match event {
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
            SessionEvent::Completed {
                transfer_id,
                bytes_transferred,
            } => TransferEvent::Completed {
                transfer_id,
                bytes_transferred,
            },
            SessionEvent::Failed { direction, reason } => {
                TransferEvent::Failed { direction, reason }
            }
            SessionEvent::Connecting => TransferEvent::Connecting,
            SessionEvent::Connected { path } => TransferEvent::Connected { path },
            SessionEvent::PathChanged { path } => TransferEvent::PathChanged { path },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoix_session::{EventSink as _, TransferDirection};
    use envoix_types::TransferId;

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
                bytes_transferred: 42,
            }
        );
    }

    #[tokio::test]
    async fn channel_closes_when_all_senders_drop() {
        let (sender, mut receiver) = EventSender::channel();
        sender.emit(TransferEvent::Pairing);
        drop(sender);

        assert_eq!(receiver.recv().await.unwrap().event, TransferEvent::Pairing);
        assert!(receiver.recv().await.is_none());
    }
}
