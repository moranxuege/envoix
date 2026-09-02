//! Canonical Manifest v2 transfer preparation, data plane, and persistence.

mod delivery_v2;
mod destination_v2;
mod job;
mod manifest_v2_engine;
mod persistence_v2;

#[cfg(test)]
mod test_support;

pub use delivery_v2::{
    DeliveryAuthorityErrorV2, ManifestV2DeliveryAuthority, ReceiverDeliveryRecordV2,
    ReceiverDeliveryStoreV2, SenderDeliveryRecordV2, SenderDeliveryStoreV2, SenderTransferPhaseV2,
};
pub use destination_v2::{
    AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES, DestinationDecisionV2, DestinationModeV2,
    DestinationPlanErrorV2, DestinationPlanStoreV2, DestinationRequestV2, DestinationWritePlanV2,
    LocalDestinationProviderV2, POST_SAVE_RESERVE_BYTES, StorageDomainIdentityV2,
    local_allocatable_bytes,
};
pub use job::{
    AddSourceResult, CanonicalTransferJob, DEFAULT_INVENTORY_PAGE_SIZE, InventoryCursor,
    InventoryItem, InventoryPage, InventorySummary, JobLifecycle, LocalSourceOrigin,
    MAX_INVENTORY_PAGE_SIZE, PreparedFileSource, ProviderSourceIssue, SourceDecision, SourceIssue,
    SourceIssueKind, SourceItemId, SourceSelectionInfo, SourceSelectionState, TransferJobError,
    TransferJobStore,
};
pub use manifest_v2_engine::{
    ManifestV2DataError, ManifestV2DataPlane, ManifestV2PayloadSink, ManifestV2ProgressPhase,
    ManifestV2ProgressSink, ManifestV2ResultGate, NoopManifestV2ResultGate,
    ReceiverDataPlaneLedgerV2, ReceiverDataPlaneStoreV2, ReceiverDataPlaneSummaryV2, SavedEntryV2,
    SenderDataPlaneSummaryV2, SenderResumeIntentV2, VerifiedEntryV2, sender_resume_intent,
};

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;

use envoix_types::{DataPath, PairingStep, TransferDirection, TransferId};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

/// Observer for canonical transfer lifecycle and transport diagnostics.
pub trait EventSink: Send + Sync {
    fn on_event(&self, event: TransferEvent);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn on_event(&self, _event: TransferEvent) {}
}

/// Cancellation intent shared by session orchestration and the v2 data plane.
#[derive(Clone, Debug, Default)]
pub struct TransferCancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    paused: AtomicBool,
    notify: Notify,
}

impl TransferCancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::SeqCst);
        self.cancel();
    }

    pub fn is_pause(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        // Create the waiter before checking the flag. `notify_waiters` tracks
        // notifications from a Notified future's creation, which closes the
        // check-then-subscribe race that could otherwise strand shutdown.
        let notified = self.inner.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Stable, secret-free milestones for one transfer attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStage {
    SessionStarted,
    ConnectionReady,
    AuthenticationStarted,
    AuthenticationComplete,
    ManifestOffer,
    ManifestAccepted,
    FirstPayload,
    PayloadComplete,
    DeliveryComplete,
    Canceled,
    Failed,
}

impl TransferStage {
    const fn order(self) -> u8 {
        match self {
            Self::SessionStarted => 0,
            Self::ConnectionReady => 1,
            Self::AuthenticationStarted => 2,
            Self::AuthenticationComplete => 3,
            Self::ManifestOffer => 4,
            Self::ManifestAccepted => 5,
            Self::FirstPayload => 6,
            Self::PayloadComplete => 7,
            Self::DeliveryComplete => 8,
            Self::Canceled | Self::Failed => 9,
        }
    }

    const fn bit(self) -> u16 {
        1 << self.order()
    }

    const fn is_terminal(self) -> bool {
        matches!(self, Self::DeliveryComplete | Self::Canceled | Self::Failed)
    }
}

#[derive(Debug, Default)]
struct TransferStageTimelineState {
    transfer_id: Option<TransferId>,
    seen: u16,
    last_order: Option<u8>,
    last_elapsed_us: u64,
    terminal: bool,
}

impl TransferStageTimelineState {
    fn bind_transfer_id(&mut self, transfer_id: TransferId) {
        if self.transfer_id.is_none() {
            self.transfer_id = Some(transfer_id);
        }
    }

    fn record_at(
        &mut self,
        direction: TransferDirection,
        attempt_id: u64,
        stage: TransferStage,
        elapsed_us: u64,
    ) -> Option<TransferEvent> {
        let order = stage.order();
        if self.terminal
            || self.seen & stage.bit() != 0
            || self.last_order.is_some_and(|last| order < last)
        {
            return None;
        }
        let elapsed_us = elapsed_us.max(self.last_elapsed_us);
        let delta_us = elapsed_us.saturating_sub(self.last_elapsed_us);
        self.seen |= stage.bit();
        self.last_order = Some(order);
        self.last_elapsed_us = elapsed_us;
        self.terminal = stage.is_terminal();
        Some(TransferEvent::StageTiming {
            transfer_id: self.transfer_id.clone(),
            direction,
            attempt_id,
            stage,
            elapsed_us,
            delta_us,
        })
    }
}

static NEXT_TRANSFER_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);

/// Monotonic stage recorder shared by session orchestration and the data plane.
///
/// `Instant` remains internal; projections receive only elapsed microseconds.
pub struct TransferStageTimeline {
    events: Arc<dyn EventSink>,
    direction: TransferDirection,
    attempt_id: u64,
    started_at: Instant,
    inner: Mutex<TransferStageTimelineInner>,
}

#[derive(Debug, Default)]
struct TransferStageTimelineInner {
    state: TransferStageTimelineState,
    pending_events: VecDeque<TransferEvent>,
    dispatching: bool,
}

impl TransferStageTimeline {
    pub fn new(
        events: Arc<dyn EventSink>,
        transfer_id: Option<TransferId>,
        direction: TransferDirection,
    ) -> Self {
        Self {
            events,
            direction,
            attempt_id: NEXT_TRANSFER_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed),
            started_at: Instant::now(),
            inner: Mutex::new(TransferStageTimelineInner {
                state: TransferStageTimelineState {
                    transfer_id,
                    ..TransferStageTimelineState::default()
                },
                ..TransferStageTimelineInner::default()
            }),
        }
    }

    pub fn bind_transfer_id(&self, transfer_id: TransferId) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            .bind_transfer_id(transfer_id);
    }

    pub fn record(&self, stage: TransferStage) {
        let elapsed_us = u64::try_from(self.started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
        let should_dispatch = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let event = inner
                .state
                .record_at(self.direction, self.attempt_id, stage, elapsed_us);
            let Some(event) = event else {
                return;
            };
            inner.pending_events.push_back(event);
            if inner.dispatching {
                false
            } else {
                inner.dispatching = true;
                true
            }
        };
        if !should_dispatch {
            return;
        }
        loop {
            let event = {
                let mut inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match inner.pending_events.pop_front() {
                    Some(event) => Some(event),
                    None => {
                        inner.dispatching = false;
                        None
                    }
                }
            };
            let Some(event) = event else {
                break;
            };
            self.events.on_event(event);
        }
    }
}

/// Typed facts consumed by CLI and native projections. Human-readable strings
/// are diagnostic only and never drive transfer state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferEvent {
    Diagnostic {
        message: String,
    },
    Pairing {
        step: PairingStep,
    },
    Connecting,
    Connected {
        path: DataPath,
    },
    PathChanged {
        path: DataPath,
    },
    Progress {
        transfer_id: TransferId,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    ManifestV2Phase {
        transfer_id: TransferId,
        direction: TransferDirection,
        phase: ManifestV2ProgressPhase,
    },
    StageTiming {
        transfer_id: Option<TransferId>,
        direction: TransferDirection,
        attempt_id: u64,
        stage: TransferStage,
        elapsed_us: u64,
        delta_us: u64,
    },
}

#[cfg(test)]
mod cancellation_tests {
    use std::time::Duration;

    use super::TransferCancelToken;

    #[tokio::test]
    async fn cancellation_wakes_every_registered_waiter_and_remains_sticky() {
        let token = TransferCancelToken::new();
        let waiters = (0..32)
            .map(|_| {
                let token = token.clone();
                tokio::spawn(async move { token.cancelled().await })
            })
            .collect::<Vec<_>>();

        tokio::task::yield_now().await;
        token.cancel();

        for waiter in waiters {
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("registered cancellation waiter timed out")
                .expect("registered cancellation waiter panicked");
        }
        tokio::time::timeout(Duration::from_secs(1), token.cancelled())
            .await
            .expect("late cancellation waiter timed out");
    }
}

#[cfg(test)]
mod stage_timing_tests {
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::{
        EventSink, TransferEvent, TransferStage, TransferStageTimeline, TransferStageTimelineState,
    };
    use envoix_types::{TransferDirection, TransferId};

    fn stage(event: Option<TransferEvent>) -> Option<(TransferStage, u64, u64)> {
        match event {
            Some(TransferEvent::StageTiming {
                stage,
                elapsed_us,
                delta_us,
                ..
            }) => Some((stage, elapsed_us, delta_us)),
            _ => None,
        }
    }

    #[test]
    fn stage_timing_is_ordered_once_only_and_monotonic() {
        let mut timeline = TransferStageTimelineState::default();
        timeline.bind_transfer_id(TransferId::new("job-1"));

        assert_eq!(
            stage(timeline.record_at(
                TransferDirection::Send,
                7,
                TransferStage::SessionStarted,
                10
            )),
            Some((TransferStage::SessionStarted, 10, 10))
        );
        assert_eq!(
            stage(timeline.record_at(TransferDirection::Send, 7, TransferStage::FirstPayload, 30)),
            Some((TransferStage::FirstPayload, 30, 20))
        );
        assert!(
            timeline
                .record_at(TransferDirection::Send, 7, TransferStage::FirstPayload, 40)
                .is_none()
        );
        assert!(
            timeline
                .record_at(
                    TransferDirection::Send,
                    7,
                    TransferStage::ManifestAccepted,
                    50
                )
                .is_none()
        );
        assert_eq!(
            stage(timeline.record_at(
                TransferDirection::Send,
                7,
                TransferStage::PayloadComplete,
                20
            )),
            Some((TransferStage::PayloadComplete, 30, 0))
        );
    }

    #[test]
    fn failed_or_canceled_timeline_cannot_report_completion() {
        for terminal in [TransferStage::Failed, TransferStage::Canceled] {
            let mut timeline = TransferStageTimelineState::default();
            assert!(
                timeline
                    .record_at(
                        TransferDirection::Receive,
                        9,
                        TransferStage::SessionStarted,
                        0
                    )
                    .is_some()
            );
            assert!(
                timeline
                    .record_at(TransferDirection::Receive, 9, terminal, 5)
                    .is_some()
            );
            assert!(
                timeline
                    .record_at(
                        TransferDirection::Receive,
                        9,
                        TransferStage::DeliveryComplete,
                        6
                    )
                    .is_none()
            );
        }
    }

    #[test]
    fn stage_names_are_stable_snake_case() {
        assert_eq!(
            serde_json::to_string(&TransferStage::FirstPayload).unwrap(),
            r#""first_payload""#
        );
        assert_eq!(
            serde_json::from_str::<TransferStage>(r#""delivery_complete""#).unwrap(),
            TransferStage::DeliveryComplete
        );
    }

    struct BlockingFirstEventSink {
        stages: Mutex<Vec<TransferStage>>,
        first_entered: mpsc::Sender<()>,
        release_first: Mutex<mpsc::Receiver<()>>,
    }

    impl EventSink for BlockingFirstEventSink {
        fn on_event(&self, event: TransferEvent) {
            let TransferEvent::StageTiming { stage, .. } = event else {
                return;
            };
            if stage == TransferStage::SessionStarted {
                self.first_entered.send(()).unwrap();
                self.release_first.lock().unwrap().recv().unwrap();
            }
            self.stages.lock().unwrap().push(stage);
        }
    }

    #[test]
    fn concurrent_records_dispatch_in_state_order_without_holding_the_state_lock() {
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let sink = Arc::new(BlockingFirstEventSink {
            stages: Mutex::new(Vec::new()),
            first_entered: first_entered_tx,
            release_first: Mutex::new(release_first_rx),
        });
        let timeline = Arc::new(TransferStageTimeline::new(
            sink.clone(),
            None,
            TransferDirection::Receive,
        ));

        let first_timeline = timeline.clone();
        let first = thread::spawn(move || first_timeline.record(TransferStage::SessionStarted));
        first_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first sink call");

        let second_timeline = timeline.clone();
        let second = thread::spawn(move || second_timeline.record(TransferStage::ConnectionReady));
        second.join().unwrap();
        release_first_tx.send(()).unwrap();
        first.join().unwrap();

        assert_eq!(
            *sink.stages.lock().unwrap(),
            vec![
                TransferStage::SessionStarted,
                TransferStage::ConnectionReady
            ]
        );
    }
}
