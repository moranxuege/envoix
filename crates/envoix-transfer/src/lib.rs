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

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use envoix_types::{DataPath, PairingStep, TransferDirection, TransferId};
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
        if self.is_cancelled() {
            return;
        }
        self.inner.notify.notified().await;
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
}
