use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread;

use envoix_evidence::{EvidenceRecord, EvidenceSink};
use envoix_types::RecordId;

const EVIDENCE_LANE_CAPACITY: usize = 64;

enum EvidenceMessage {
    Record(EvidenceRecord),
    EvictCard(RecordId),
}

/// The authority-side half of a strictly one-way evidence lane.
///
/// Producers only `try_send` into a fixed-capacity queue. A dedicated worker
/// owns the injected sink, so a slow sink can fill this queue but can never
/// stall a card actor. Full/disconnected queues drop evidence. Sink errors and
/// panics are contained on the worker and never return to authority execution.
#[derive(Clone, Default)]
pub(crate) struct EvidencePublisher {
    sender: Option<SyncSender<EvidenceMessage>>,
}

impl EvidencePublisher {
    pub(crate) fn new<S: EvidenceSink>(sink: S) -> Self {
        let (sender, receiver) = sync_channel(EVIDENCE_LANE_CAPACITY);
        let spawn = thread::Builder::new()
            .name("envoix-evidence".to_owned())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    let _ = catch_unwind(AssertUnwindSafe(|| match message {
                        EvidenceMessage::Record(record) => sink.record(record),
                        EvidenceMessage::EvictCard(card) => sink.evict_card(card),
                    }));
                }
            });
        if spawn.is_err() {
            return Self::default();
        }
        Self {
            sender: Some(sender),
        }
    }

    pub(crate) fn publish(&self, record: EvidenceRecord) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(EvidenceMessage::Record(record));
        }
    }

    pub(crate) fn evict_card(&self, card: RecordId) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(EvidenceMessage::EvictCard(card));
        }
    }
}
