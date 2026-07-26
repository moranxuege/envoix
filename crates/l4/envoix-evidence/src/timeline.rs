use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{Mutex, MutexGuard, PoisonError};

use envoix_types::RecordId;
use serde::Serialize;

use crate::model::{
    DiagnosticsDegraded, DiagnosticsStatus, EvidenceRecord, EvidenceValue, SessionKey,
    TimelineEntry,
};

/// Typed sink failure. No implementation can attach error prose or secrets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceSinkError {
    Full,
    Closed,
    Unavailable,
}

/// Write-only evidence port.
///
/// Runtime containment calls this port away from authority execution and
/// ignores its result. The default eviction is best effort so minimal sinks
/// need implement only typed record intake.
pub trait EvidenceSink: Send + Sync + 'static {
    fn record(&self, record: EvidenceRecord) -> Result<(), EvidenceSinkError>;

    fn evict_card(&self, _card: RecordId) -> Result<(), EvidenceSinkError> {
        Ok(())
    }
}

/// A sink for hosts that do not retain diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEvidenceSink;

impl EvidenceSink for NoopEvidenceSink {
    fn record(&self, _record: EvidenceRecord) -> Result<(), EvidenceSinkError> {
        Ok(())
    }
}

/// A read snapshot of one bounded session timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionTimeline {
    session: SessionKey,
    diagnostics: DiagnosticsStatus,
    entries: Vec<TimelineEntry>,
}

impl SessionTimeline {
    pub const fn session(&self) -> SessionKey {
        self.session
    }

    pub const fn diagnostics(&self) -> DiagnosticsStatus {
        self.diagnostics
    }

    pub fn entries(&self) -> &[TimelineEntry] {
        &self.entries
    }
}

struct RetainedSession {
    next_sequence: u64,
    diagnostics: DiagnosticsStatus,
    entries: VecDeque<TimelineEntry>,
}

impl RetainedSession {
    fn new() -> Self {
        Self {
            next_sequence: 1,
            diagnostics: DiagnosticsStatus::Complete,
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, capacity: usize, value: EvidenceValue) {
        if self.entries.len() == capacity {
            self.entries.pop_front();
            match &mut self.diagnostics {
                DiagnosticsStatus::Complete => {
                    self.diagnostics =
                        DiagnosticsStatus::DiagnosticsDegraded(DiagnosticsDegraded::one());
                }
                DiagnosticsStatus::DiagnosticsDegraded(degraded) => degraded.increment(),
            }
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.entries.push_back(TimelineEntry::new(sequence, value));
    }
}

struct StoreState {
    sessions: HashMap<SessionKey, RetainedSession>,
    insertion_order: VecDeque<SessionKey>,
}

/// In-memory bounded evidence projection.
///
/// Session entry and session-count bounds are both fixed at construction.
/// Overflow drops the oldest entry and permanently marks that session
/// `diagnostics_degraded`; session-count overflow evicts the oldest session.
pub struct TimelineStore {
    per_session_capacity: NonZeroUsize,
    session_capacity: NonZeroUsize,
    state: Mutex<StoreState>,
}

impl TimelineStore {
    pub fn new(per_session_capacity: NonZeroUsize, session_capacity: NonZeroUsize) -> Self {
        Self {
            per_session_capacity,
            session_capacity,
            state: Mutex::new(StoreState {
                sessions: HashMap::new(),
                insertion_order: VecDeque::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub const fn per_session_capacity(&self) -> NonZeroUsize {
        self.per_session_capacity
    }

    pub const fn session_capacity(&self) -> NonZeroUsize {
        self.session_capacity
    }

    pub fn session_count(&self) -> usize {
        self.lock().sessions.len()
    }

    /// Every session this store still retains, oldest first. A reader that
    /// needs to re-publish what is held asks the store rather than mirroring
    /// its eviction bookkeeping.
    pub fn sessions(&self) -> Vec<SessionKey> {
        self.lock().insertion_order.iter().copied().collect()
    }

    pub fn snapshot(&self, session: SessionKey) -> Option<SessionTimeline> {
        let state = self.lock();
        let retained = state.sessions.get(&session)?;
        Some(SessionTimeline {
            session,
            diagnostics: retained.diagnostics,
            entries: retained.entries.iter().cloned().collect(),
        })
    }

    fn retain(&self, record: EvidenceRecord) {
        let (session, value) = record.into_parts();
        let mut state = self.lock();
        if !state.sessions.contains_key(&session) {
            if state.sessions.len() == self.session_capacity.get()
                && let Some(oldest) = state.insertion_order.pop_front()
            {
                state.sessions.remove(&oldest);
            }
            state.insertion_order.push_back(session);
            state.sessions.insert(session, RetainedSession::new());
        }
        state
            .sessions
            .get_mut(&session)
            .expect("the session was inserted under the held lock")
            .push(self.per_session_capacity.get(), value);
    }

    fn remove_card(&self, card: RecordId) {
        let mut state = self.lock();
        state.sessions.retain(|session, _| session.card != card);
        state.insertion_order.retain(|session| session.card != card);
    }
}

impl EvidenceSink for TimelineStore {
    fn record(&self, record: EvidenceRecord) -> Result<(), EvidenceSinkError> {
        self.retain(record);
        Ok(())
    }

    fn evict_card(&self, card: RecordId) -> Result<(), EvidenceSinkError> {
        self.remove_card(card);
        Ok(())
    }
}
