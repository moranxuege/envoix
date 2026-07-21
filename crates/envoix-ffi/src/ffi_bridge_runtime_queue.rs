#[derive(uniffi::Object)]
pub struct EnvoixSession {
    runtime: Runtime,
    queue: Arc<Mutex<TransferQueueState>>,
    settings: EnvoixRuntimeSettings,
}

#[derive(Default)]
struct TransferQueueState {
    pending: VecDeque<QueuedTransfer>,
    active: HashMap<String, ActiveTransfer>,
    paused: HashMap<String, QueuedTransfer>,
    pending_pause_actions: HashMap<String, PendingPauseAction>,
    history: VecDeque<FfiTransferActivityRecord>,
    requests: HashMap<String, FfiTransferRequest>,
    discarded: HashSet<String>,
}

struct QueuedTransfer {
    request: FfiTransferRequest,
    observer: Arc<dyn TransferObserver>,
    activity: FfiTransferActivityRecord,
}

#[derive(Debug)]
struct TransferAttemptSource {
    mode: FfiTransferMode,
    source: PeerSource,
    path_policy_override: Option<FfiPathPolicy>,
}

struct TransferAttemptOutcome {
    result: Result<TransferSummary, TransferError>,
    direction: Option<TransferDirection>,
    transfer_started: bool,
    stop_requested: Option<TransferStop>,
}

struct ActiveTransfer {
    control: Option<oneshot::Sender<TransferStop>>,
    limit: usize,
    activity: FfiTransferActivityRecord,
}

struct FinishedActivityNotice {
    observer: Arc<dyn TransferObserver>,
    activity: FfiTransferActivityRecord,
    status: &'static str,
    cleanup_request: Option<FfiTransferRequest>,
}

fn is_finalizing_activity(activity: &FfiTransferActivityRecord) -> bool {
    activity.state == FfiTransferActivityState::Verifying
        && activity.diagnostic_message == "confirming"
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferStop {
    Cancel,
    Pause,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPauseAction {
    Pause,
    Resume,
    Cancel,
}

impl TransferAttemptSource {
    fn new(mode: FfiTransferMode, source: PeerSource) -> Self {
        Self {
            mode,
            source,
            path_policy_override: None,
        }
    }

    fn with_path_policy(mut self, path_policy: FfiPathPolicy) -> Self {
        self.path_policy_override = Some(path_policy);
        self
    }
}

impl TransferQueueState {
    fn contains_activity(&self, activity_id: &str) -> bool {
        self.active.contains_key(activity_id)
            || self
                .pending
                .iter()
                .any(|job| job.activity.activity_id == activity_id)
            || self.paused.contains_key(activity_id)
    }

    fn activities(&self) -> Vec<FfiTransferActivityRecord> {
        let mut seen = HashMap::new();
        let mut records = Vec::new();
        for record in self.history.iter() {
            if self.discarded.contains(&record.activity_id) {
                continue;
            }
            seen.insert(record.activity_id.clone(), ());
            records.push(record.clone());
        }
        for record in self.active.values().map(|active| &active.activity) {
            if !self.discarded.contains(&record.activity_id)
                && !seen.contains_key(&record.activity_id)
            {
                seen.insert(record.activity_id.clone(), ());
                records.push(record.clone());
            }
        }
        for job in self.pending.iter() {
            if !self.discarded.contains(&job.activity.activity_id)
                && !seen.contains_key(&job.activity.activity_id)
            {
                seen.insert(job.activity.activity_id.clone(), ());
                records.push(job.activity.clone());
            }
        }
        for job in self.paused.values() {
            if !self.discarded.contains(&job.activity.activity_id)
                && !seen.contains_key(&job.activity.activity_id)
            {
                seen.insert(job.activity.activity_id.clone(), ());
                records.push(job.activity.clone());
            }
        }
        records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        records
    }

    fn activity(&self, activity_id: &str) -> Option<FfiTransferActivityRecord> {
        self.activities()
            .into_iter()
            .find(|record| record.activity_id == activity_id)
    }

    fn push_history(&mut self, activity: FfiTransferActivityRecord) {
        if self.discarded.contains(&activity.activity_id) {
            return;
        }
        self.history
            .retain(|record| record.activity_id != activity.activity_id);
        self.history.push_front(activity);
        if self.history.len() > TRANSFER_ACTIVITY_HISTORY_CAP {
            self.history.truncate(TRANSFER_ACTIVITY_HISTORY_CAP);
        }
        let retained_ids = self
            .history
            .iter()
            .map(|record| record.activity_id.clone())
            .chain(self.active.keys().cloned())
            .chain(
                self.pending
                    .iter()
                    .map(|job| job.activity.activity_id.clone()),
            )
            .chain(self.paused.keys().cloned())
            .collect::<HashSet<_>>();
        self.requests
            .retain(|activity_id, _| retained_ids.contains(activity_id));
    }

    fn can_start(&self, next_limit: usize) -> bool {
        let limit = self
            .active
            .values()
            .map(|active| active.limit)
            .chain(std::iter::once(next_limit))
            .min()
            .unwrap_or(1)
            .max(1);
        self.active.len() < limit
    }
}
