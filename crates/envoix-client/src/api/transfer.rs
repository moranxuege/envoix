//! The transfer handle and the adapters that feed its event stream.

use envoix_session::{TransferCancelToken, TransferEvent as SessionEvent, TransferSummary};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::TransferEvent;
use crate::PublicError;

/// A running transfer: observe it through [`Transfer::next_event`], stop it
/// with [`Transfer::cancel`], and take its result with [`Transfer::wait`].
///
/// Dropping the handle cancels the transfer (the peer is notified where the
/// protocol allows it).
#[derive(Debug)]
pub struct Transfer {
    events: mpsc::UnboundedReceiver<TransferEvent>,
    cancel: TransferCancelToken,
    // Option only so `wait(self)` can move it out despite the Drop impl.
    task: Option<JoinHandle<Result<TransferSummary, PublicError>>>,
}

impl Transfer {
    pub(crate) fn new(
        events: mpsc::UnboundedReceiver<TransferEvent>,
        cancel: TransferCancelToken,
        task: JoinHandle<Result<TransferSummary, PublicError>>,
    ) -> Self {
        Self {
            events,
            cancel,
            task: Some(task),
        }
    }

    /// The next lifecycle event, or `None` once the transfer has ended and
    /// all events were consumed.
    pub async fn next_event(&mut self) -> Option<TransferEvent> {
        self.events.recv().await
    }

    /// Requests a graceful stop; the transfer ends with a cancellation error.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Waits for the transfer to finish and returns its outcome. Events not
    /// yet consumed are discarded.
    pub async fn wait(mut self) -> Result<TransferSummary, PublicError> {
        let task = self.task.take().expect("wait consumes the handle");
        match task.await {
            Ok(result) => result,
            Err(error) => Err(PublicError::Transfer(format!(
                "transfer task failed: {error}"
            ))),
        }
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
pub(crate) struct EventSender(mpsc::UnboundedSender<TransferEvent>);

impl EventSender {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<TransferEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self(sender), receiver)
    }

    /// Sends one event; silently dropped when the handle is gone.
    pub(crate) fn emit(&self, event: TransferEvent) {
        let _ = self.0.send(event);
    }
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

        assert_eq!(
            receiver.recv().await.unwrap(),
            TransferEvent::Verifying {
                transfer_id: TransferId::new("t1"),
                direction: TransferDirection::Receive,
                file_name: "a.bin".into(),
                bytes_to_hash: 42,
            }
        );
        assert_eq!(
            receiver.recv().await.unwrap(),
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

        assert_eq!(receiver.recv().await.unwrap(), TransferEvent::Pairing);
        assert_eq!(receiver.recv().await, None);
    }
}
