//! In-memory dispatch bookkeeping for platform duties.
//!
//! The C6 [`DutyLedger`](envoix_capabilities::DutyLedger) is the exactly-once
//! authority for RESULTS; this adapter owns the two platform-side concerns the
//! ledger deliberately does not:
//!
//! - **Idempotent dispatch while live.** The runtime re-delivers every
//!   outstanding duty on each fresh attachment epoch; a duty already in
//!   flight must not be dispatched to the service twice.
//! - **Publication retention.** A staged artifact backing a publication may
//!   only be released after the host SETTLES the publication (its result was
//!   admitted and the record write committed). A process death wipes this
//!   in-memory state, which is safe by construction: re-delivered duties are
//!   re-dispatched, service execution is idempotent per provenance, and an
//!   unsettled publication keeps its staged copy.

use std::collections::HashMap;

use envoix_capabilities::DutyProvenance;

use crate::duty::{Work, WorkOrder};

/// What the host should do with a duty it wants executed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssueDecision {
    /// Send this order to the service now.
    Dispatch(WorkOrder),
    /// The identical order is already in flight; do nothing.
    AlreadyInFlight,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IssuedDuty {
    order: WorkOrder,
    settled: bool,
}

/// Per-process duty dispatch state. Deliberately NOT durable — see module doc.
#[derive(Debug, Default)]
pub struct DutyAdapter {
    issued: HashMap<DutyProvenance, IssuedDuty>,
}

impl DutyAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides whether `order` needs dispatching. Idempotent per provenance.
    pub fn issue(&mut self, order: WorkOrder) -> IssueDecision {
        let provenance = order.provenance.to_provenance();
        if self.issued.contains_key(&provenance) {
            return IssueDecision::AlreadyInFlight;
        }
        self.issued.insert(
            provenance,
            IssuedDuty {
                order: order.clone(),
                settled: false,
            },
        );
        IssueDecision::Dispatch(order)
    }

    /// Marks a publication (or any duty) settled: its result was admitted by
    /// the ledger AND the consuming record write committed.
    pub fn settle(&mut self, provenance: DutyProvenance) {
        if let Some(issued) = self.issued.get_mut(&provenance) {
            issued.settled = true;
        }
    }

    /// Whether the staged copy behind a publication may be released.
    ///
    /// `false` for unknown provenance: after a process death the adapter
    /// cannot vouch for settlement, so the staged copy stays until the duty is
    /// re-driven to a settled state (never lose the last copy).
    pub fn publication_releasable(&self, provenance: DutyProvenance) -> bool {
        self.issued.get(&provenance).is_some_and(|issued| {
            matches!(issued.order.work, Work::Publication { .. }) && issued.settled
        })
    }

    /// Number of orders dispatched and not yet settled.
    pub fn in_flight(&self) -> usize {
        self.issued
            .values()
            .filter(|issued| !issued.settled)
            .count()
    }
}
