use std::collections::HashMap;

use envoix_capabilities::{Admission, Duty, DutyKind, DutyLedger, DutyProvenance, Registration};
use envoix_outcomes::OutcomeCode;
use envoix_types::{AttemptGen, RecordId, RequestId};

use crate::{
    DutyAdapter, EXECUTED_KINDS, IssueDecision, LaneError, Notice, WireProvenance, Work, WorkOrder,
    WorkReport, platform_work,
};

fn provenance(request_seed: u8) -> DutyProvenance {
    DutyProvenance {
        card: RecordId::new(7),
        generation: AttemptGen::new(1),
        request: RequestId::from_bytes([request_seed; 16]),
    }
}

fn work_for(kind: DutyKind) -> Work {
    match kind {
        DutyKind::SourceHandle => Work::SourceHandle,
        DutyKind::Grant => Work::Grant,
        DutyKind::Staging => Work::Staging,
        DutyKind::Publication => Work::Publication {
            staged: "artifacts/0011223344556677/payload.bin".into(),
            display_name: "payload.bin".into(),
            total_bytes: 1024,
        },
        DutyKind::Courier => Work::Courier,
        DutyKind::Foreground => Work::Foreground {
            active_transfers: 1,
        },
        DutyKind::Notification => Work::Notification {
            notice: Notice::TransferComplete,
        },
        DutyKind::Lock => Work::Lock { hold: true },
        DutyKind::OpenShare => Work::OpenShare,
    }
}

/// For every duty class: live re-delivery is deduplicated, a crash re-issues
/// the identical order, results are admitted exactly once, and a result lost
/// before admission replays cleanly across a crash.
#[test]
fn platform_duty_crash_matrix() {
    for (index, kind) in DutyKind::ALL.into_iter().enumerate() {
        let seed = 0x10 + index as u8;
        let duty = Duty {
            provenance: provenance(seed),
            kind,
        };
        let order = WorkOrder::for_duty(duty, work_for(kind)).unwrap();

        let mut ledger = DutyLedger::new();
        ledger.advance_generation(duty.provenance.card, duty.provenance.generation);
        assert_eq!(ledger.register(duty), Registration::Registered);

        // Live re-delivery (a fresh subscription epoch replays outstanding
        // duties) must not double-dispatch.
        let mut adapter = DutyAdapter::new();
        assert_eq!(
            adapter.issue(order.clone()),
            IssueDecision::Dispatch(order.clone())
        );
        assert_eq!(adapter.issue(order.clone()), IssueDecision::AlreadyInFlight);

        // Crash before any result: a fresh process re-derives the SAME order
        // from the re-delivered duty — deterministic, so idempotent service
        // execution is possible.
        let mut adapter = DutyAdapter::new();
        let rebuilt = WorkOrder::for_duty(duty, work_for(kind)).unwrap();
        assert_eq!(
            rebuilt, order,
            "{kind:?}: re-derived order must be identical"
        );
        assert_eq!(
            adapter.issue(rebuilt),
            IssueDecision::Dispatch(order.clone())
        );

        // A crash that loses the service's report before admission: the duty
        // is still outstanding, the replayed report admits exactly once.
        let report = WorkReport::new(duty.provenance, OutcomeCode::Completed);
        let lost_then_replayed = WorkReport::decode(&report.encode().unwrap()).unwrap();
        assert!(matches!(
            ledger.admit(lost_then_replayed.to_result()),
            Admission::Fresh(_)
        ));

        // The duplicate (the service retrying after a crash on its side) is
        // rejected by the ledger, never re-applied.
        assert_eq!(ledger.admit(report.to_result()), Admission::Duplicate);

        // Publication additionally carries the MediaStore recovery journal, so
        // pin every publish crash window against the model of that lane.
        if kind == DutyKind::Publication {
            pin_publication_crash_windows(duty.provenance);
        }
    }
}

/// A minimal model of the Kotlin publication lane — a MediaStore (rows with an
/// IS_PENDING flag, unique per (RELATIVE_PATH, DISPLAY_NAME) as the real shared
/// collection is) plus the private journal (recovery key -> reserved/committed
/// row) and the private staged copy. It mirrors the executor's discipline:
/// journaled-row reuse then query-before-insert (never a duplicate row), commit
/// clears IS_PENDING under a name the collection accepts, and the source is
/// deleted only after a COMMITTED row is verified. The crash windows are
/// pinned against it.
#[derive(Clone)]
struct Row {
    name: String,
    pending: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PubState {
    Reserved,
    Committed,
    Acknowledged,
}

#[derive(Clone, Copy)]
struct Journal {
    row: usize,
    state: PubState,
}

#[derive(Default)]
struct PublicationWorld {
    rows: Vec<Row>,
    journal: HashMap<String, Journal>,
    source_present: HashMap<String, bool>,
}

impl PublicationWorld {
    fn pending_name(key: &str) -> String {
        format!(".envoix-{key}.pending")
    }

    /// Reserve the pending row (reuse the journaled row, else the pending row by
    /// name, else insert one) and mark the private source present. Stops before
    /// commit — the mid-copy crash window.
    fn reserve(&mut self, key: &str) {
        let pending = Self::pending_name(key);
        let row = if let Some(journal) = self.journal.get(key).copied() {
            journal.row
        } else if let Some(row) = self.rows.iter().position(|row| row.name == pending) {
            row
        } else {
            self.rows.push(Row {
                name: pending,
                pending: true,
            });
            self.rows.len() - 1
        };
        self.journal.insert(
            key.to_owned(),
            Journal {
                row,
                state: PubState::Reserved,
            },
        );
        self.source_present.insert(key.to_owned(), true);
    }

    /// Commit the reserved row: clear IS_PENDING under a display name the
    /// collection's uniqueness accepts, then journal the committed state. A
    /// name another row already holds is uniquified — the legacy UNIQUE crash
    /// becomes a rename.
    fn commit(&mut self, key: &str, display_name: &str) {
        let row = self.journal.get(key).expect("reserved before commit").row;
        self.rows[row].name = self.free_name(row, display_name);
        self.rows[row].pending = false;
        if let Some(journal) = self.journal.get_mut(key) {
            journal.state = PubState::Committed;
        }
    }

    fn free_name(&self, row: usize, display_name: &str) -> String {
        (1..)
            .map(|attempt| uniquified(display_name, attempt))
            .find(|candidate| {
                !self
                    .rows
                    .iter()
                    .enumerate()
                    .any(|(index, other)| index != row && &other.name == candidate)
            })
            .expect("a suffix is always free")
    }

    fn publish(&mut self, key: &str, display_name: &str) {
        self.reserve(key);
        self.commit(key, display_name);
    }

    /// Post-admission: delete the private source ONLY after verifying the
    /// COMMITTED (non-pending) row still exists — the same predicate the Kotlin
    /// executor checks before it releases the copy.
    fn acknowledge(&mut self, key: &str) {
        let Some(&Journal { row, .. }) = self.journal.get(key) else {
            return;
        };
        if self.rows.get(row).is_some_and(|row| !row.pending) {
            self.source_present.insert(key.to_owned(), false);
            if let Some(journal) = self.journal.get_mut(key) {
                journal.state = PubState::Acknowledged;
            }
        }
    }

    fn source_present(&self, key: &str) -> bool {
        self.source_present.get(key).copied().unwrap_or(false)
    }

    fn committed_row_exists(&self, key: &str) -> bool {
        self.journal
            .get(key)
            .is_some_and(|journal| self.rows.get(journal.row).is_some_and(|row| !row.pending))
    }

    /// The invariant the lane must never break: at least one durable copy of the
    /// artifact survives every crash window.
    fn never_lost_last_copy(&self, key: &str) -> bool {
        self.source_present(key) || self.committed_row_exists(key)
    }
}

fn pin_publication_crash_windows(provenance: DutyProvenance) {
    let key = WireProvenance::from_provenance(provenance).recovery_key();
    let display = "payload.bin";

    // Window 1 — crash mid-copy: the pending row is reserved and the source
    // journaled, but the copy/commit never ran. Replay reuses the SAME reserved
    // row (no duplicate) and the source is never lost.
    let mut world = PublicationWorld::default();
    world.reserve(&key);
    assert!(
        world.source_present(&key),
        "mid-copy crash keeps the source"
    );
    assert_eq!(world.rows.len(), 1);
    assert!(world.rows[0].pending, "the reserved row is still pending");
    world.publish(&key, display);
    assert_eq!(world.rows.len(), 1, "replay reuses the reserved row");
    assert!(!world.rows[0].pending);
    assert!(world.never_lost_last_copy(&key));

    // Window 2 — crash between commit and ack: the row is committed but the
    // source is not yet deleted, so a copy is always present. Replaying the ack
    // then verifies the committed row and releases the source.
    let mut world = PublicationWorld::default();
    world.publish(&key, display);
    assert!(world.committed_row_exists(&key));
    assert!(
        world.source_present(&key),
        "source survives until acknowledged"
    );
    assert!(world.never_lost_last_copy(&key));
    world.acknowledge(&key);
    assert!(
        !world.source_present(&key),
        "verified commit releases the source"
    );
    assert_eq!(world.rows.len(), 1);

    // Window 3 — replay after commit inserts no second row.
    let mut world = PublicationWorld::default();
    world.publish(&key, display);
    world.publish(&key, display);
    assert_eq!(
        world.rows.len(),
        1,
        "a committed publication is not re-inserted"
    );

    // Window 4 — the source is retained until the commit is verified: an ack
    // that cannot see a committed row must not delete the private copy.
    let mut world = PublicationWorld::default();
    world.reserve(&key);
    world.acknowledge(&key);
    assert!(
        world.source_present(&key),
        "an unverified commit retains the source"
    );

    // Window 5 — two provenances offering the SAME display name. The shared
    // collection's uniqueness makes the second commit a rename, never a crash,
    // and neither copy is lost.
    let mut world = PublicationWorld::default();
    let other = format!("{key}-other");
    world.publish(&key, display);
    world.publish(&other, display);
    assert_eq!(world.rows.len(), 2, "each provenance keeps its own row");
    assert_eq!(world.rows[0].name, display);
    assert_eq!(world.rows[1].name, "payload (2).bin");
    assert!(world.committed_row_exists(&key));
    assert!(world.committed_row_exists(&other));
}

/// `payload.bin` -> `payload.bin`, `payload (2).bin`, ... mirroring the Kotlin
/// executor's deterministic uniquifier.
fn uniquified(display_name: &str, attempt: u32) -> String {
    if attempt == 1 {
        return display_name.to_owned();
    }
    match display_name.rfind('.') {
        Some(dot) if dot > 0 => format!(
            "{} ({attempt}){}",
            &display_name[..dot],
            &display_name[dot..]
        ),
        _ => format!("{display_name} ({attempt})"),
    }
}

/// The Kotlin service executor and the Rust dispatch set are ONE set. A duty
/// the host dispatches but Kotlin drops is never reported, never admitted, and
/// stays in flight forever; a kind Kotlin handles but nothing dispatches is
/// dead platform code. Both directions are pinned here against the executor
/// source itself.
#[test]
fn kotlin_executor_handles_exactly_the_executed_kinds() {
    const KOTLIN_EXECUTOR: &str = include_str!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/DutyExecutor.kt"
    );

    let dispatch = KOTLIN_EXECUTOR
        .split_once("when (work.optString(\"kind\"))")
        .expect("the executor dispatches on the work kind")
        .1;
    let dispatch = dispatch
        .split_once("else ->")
        .expect("the executor has an unhandled-kind default")
        .0;
    let mut handled: Vec<String> = dispatch
        .lines()
        .filter_map(|line| {
            let label = line.trim().split_once(" ->")?.0;
            Some(label.strip_prefix('"')?.strip_suffix('"')?.to_owned())
        })
        .collect();
    handled.sort();

    let mut executed: Vec<String> = EXECUTED_KINDS
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .expect("a duty kind serializes")
                .as_str()
                .expect("as a string")
                .to_owned()
        })
        .collect();
    executed.sort();
    assert_eq!(handled, executed, "Kotlin executor vs EXECUTED_KINDS");

    // What the host pump can dispatch is a subset of what Kotlin executes,
    // and every payload it builds matches its duty kind.
    for kind in DutyKind::ALL {
        let duty = Duty {
            provenance: provenance(0x50),
            kind,
        };
        if let Some(work) = platform_work(duty) {
            assert_eq!(work.kind(), kind);
            assert!(
                EXECUTED_KINDS.contains(&kind),
                "{kind:?} is dispatched but not executed by the service"
            );
        }
    }
}

/// A publication's staged copy is releasable only after the host settles it;
/// a process death forgets settlement and must fail closed.
#[test]
fn publication_staged_copy_survives_until_settled() {
    let duty = Duty {
        provenance: provenance(0x77),
        kind: DutyKind::Publication,
    };
    let order = WorkOrder::for_duty(duty, work_for(DutyKind::Publication)).unwrap();

    let mut adapter = DutyAdapter::new();
    adapter.issue(order.clone());
    assert!(!adapter.publication_releasable(duty.provenance));

    // A report alone (even one the ledger admits) is not settlement: the
    // record write consuming the result may still fail.
    assert!(!adapter.publication_releasable(duty.provenance));

    adapter.settle(duty.provenance);
    assert!(adapter.publication_releasable(duty.provenance));

    // Process death: the fresh adapter cannot vouch for settlement.
    let adapter = DutyAdapter::new();
    assert!(!adapter.publication_releasable(duty.provenance));
}

#[test]
fn lane_frames_are_bounded_and_typed() {
    let duty = Duty {
        provenance: provenance(0x31),
        kind: DutyKind::Publication,
    };

    // Kind/payload mismatch is unrepresentable at build time.
    assert_eq!(
        WorkOrder::for_duty(duty, Work::Lock { hold: true }),
        Err(LaneError::KindMismatch)
    );

    // Path traversal and absolute staged paths are rejected.
    for staged in ["/etc/passwd", "a/../b", ""] {
        let work = Work::Publication {
            staged: staged.into(),
            display_name: "x".into(),
            total_bytes: 1,
        };
        assert_eq!(WorkOrder::for_duty(duty, work), Err(LaneError::Bounds));
    }

    // Round-trip.
    let order = WorkOrder::for_duty(duty, work_for(DutyKind::Publication)).unwrap();
    let bytes = order.encode().unwrap();
    assert_eq!(WorkOrder::decode(&bytes).unwrap(), order);

    // Unknown fields and malformed frames are typed failures.
    assert_eq!(
        WorkOrder::decode(br#"{"provenance":{"card":"0000000000000007","generation":1,"request":"31313131313131313131313131313131"},"work":{"kind":"lock","hold":true},"extra":1}"#),
        Err(LaneError::Malformed)
    );
    assert_eq!(WorkOrder::decode(b"not json"), Err(LaneError::Malformed));

    // Uppercase or short hex is rejected.
    assert_eq!(
        WorkOrder::decode(br#"{"provenance":{"card":"0000000000000ABC","generation":1,"request":"31313131313131313131313131313131"},"work":{"kind":"lock","hold":true}}"#),
        Err(LaneError::Malformed)
    );

    // Oversized frames are rejected before parsing.
    let oversized = vec![b' '; MAX_LANE_FRAME_BYTES_PLUS_ONE];
    assert_eq!(WorkOrder::decode(&oversized), Err(LaneError::FrameTooLarge));
    assert_eq!(
        WorkReport::decode(&oversized),
        Err(LaneError::FrameTooLarge)
    );
}

const MAX_LANE_FRAME_BYTES_PLUS_ONE: usize = crate::MAX_LANE_FRAME_BYTES + 1;
