use std::collections::{BTreeSet, HashMap, HashSet};

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
        // The source handle became a LIVE kind in F2b, so its own lane gets the
        // same treatment: the platform holds the pick, the card claims it, and
        // a process death is an honest "re-pick" rather than a silent stall.
        if kind == DutyKind::SourceHandle {
            pin_source_pick_crash_windows(duty.provenance);
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

/// A model of the Kotlin source-pick lane: the picker slot is ephemeral, while
/// CARD → document is the durable lifecycle ownership created only on claim.
#[derive(Default)]
struct PickWorld {
    offered: Option<&'static str>,
    bound: HashMap<String, &'static str>,
    owners: HashMap<String, &'static str>,
    persisted: HashSet<&'static str>,
}

impl PickWorld {
    fn offer(&mut self, document: &'static str) {
        self.offered = Some(document);
    }

    fn claim(&mut self, card: &str) -> Option<&'static str> {
        if let Some(bound) = self.bound.get(card) {
            return Some(bound);
        }
        if let Some(owned) = self.owners.get(card).copied() {
            self.bound.insert(card.to_owned(), owned);
            return Some(owned);
        }
        let picked = self.offered.take()?;
        // Android retains first; the journal makes the already-durable card
        // its owner. `recover` owns the crash between those two operations.
        self.persisted.insert(picked);
        self.owners.insert(card.to_owned(), picked);
        self.bound.insert(card.to_owned(), picked);
        Some(picked)
    }

    fn restart(&self) -> Self {
        Self {
            offered: None,
            bound: HashMap::new(),
            owners: self.owners.clone(),
            persisted: self.persisted.clone(),
        }
    }

    fn remove(&mut self, card: &str) {
        let owned = self.owners.remove(card);
        self.bound.remove(card);
        if let Some(document) = owned
            && !self.owners.values().any(|other| *other == document)
        {
            self.persisted.remove(document);
        }
    }

    fn recover(&mut self) {
        self.persisted
            .retain(|document| self.owners.values().any(|owned| owned == document));
    }

    /// The outcome the executor reports for one delivery of the duty.
    fn execute(&mut self, card: &str) -> OutcomeCode {
        match self.claim(card) {
            Some(_) => OutcomeCode::Completed,
            None => OutcomeCode::SourceUnreadable,
        }
    }
}

fn pin_source_pick_crash_windows(provenance: DutyProvenance) {
    let card = String::from(WireProvenance::from_provenance(provenance).card);
    let other = format!("{card}-other");

    // Window 1 — the duty is delivered twice in one process (a re-attach
    // replays the outstanding duty). The second delivery must resolve to the
    // SAME document rather than eat whatever the user picked next.
    let mut world = PickWorld::default();
    world.offer("holiday.mp4");
    assert_eq!(world.execute(&card), OutcomeCode::Completed);
    world.offer("something-else.bin");
    assert_eq!(world.execute(&card), OutcomeCode::Completed);
    assert_eq!(world.bound[&card], "holiday.mp4", "a re-delivery re-binds");
    // ...and the newer pick is still there for the card that asked for it.
    assert_eq!(world.execute(&other), OutcomeCode::Completed);
    assert_eq!(world.bound[&other], "something-else.bin");

    // Window 2 — a pick alone owns no retained capability. A crash before a
    // committed card claims it loses only the ephemeral picker slot.
    let mut world = PickWorld::default();
    world.offer("holiday.mp4");
    assert!(world.persisted.is_empty(), "offer took a persistent grant");
    let mut crashed = PickWorld::default();
    assert_eq!(crashed.execute(&card), OutcomeCode::SourceUnreadable);
    assert_eq!(world.claim(&card), Some("holiday.mp4"), "the model is live");

    // Window 3 — a claimed source survives process memory because its durable
    // card owns the grant and URI together.
    let mut crashed = world.restart();
    assert_eq!(crashed.execute(&card), OutcomeCode::Completed);
    assert_eq!(crashed.bound[&card], "holiday.mp4");

    // Window 4 — a card that never had a pick reports the same thing, so a
    // frontend that asked for a send without one is refused by the platform
    // rather than left waiting.
    let mut world = PickWorld::default();
    assert_eq!(world.execute(&card), OutcomeCode::SourceUnreadable);

    // Window 5 — removal ends ownership and the last owner's grant. Sharing a
    // URI retains it until the final card leaves.
    let mut world = PickWorld::default();
    world.offer("holiday.mp4");
    assert_eq!(world.execute(&card), OutcomeCode::Completed);
    world.owners.insert(other.clone(), "holiday.mp4");
    world.remove(&card);
    assert!(world.persisted.contains("holiday.mp4"));
    world.remove(&other);
    assert!(!world.persisted.contains("holiday.mp4"));

    // Window 6 — Android retained access and the process died before the owner
    // journal committed. Recovery releases every such unowned grant.
    let mut world = PickWorld::default();
    world.persisted.insert("orphaned.bin");
    world.recover();
    assert!(world.persisted.is_empty());

    // The report crosses the lane and the ledger admits it exactly once,
    // whichever outcome it carried.
    for outcome in [OutcomeCode::Completed, OutcomeCode::SourceUnreadable] {
        let report = WorkReport::new(provenance, outcome);
        let decoded = WorkReport::decode(&report.encode().unwrap()).unwrap();
        assert_eq!(decoded, report);
        assert_eq!(decoded.to_result().outcome, outcome);
    }
}

/// The Android half names the same lifecycle the model proves: the Activity
/// can only offer an ephemeral pick, a card claim is the only place a grant is
/// retained, and the service consumes durable removal ids to release it.
#[test]
fn persistable_source_grants_have_exactly_one_lifecycle_owner() {
    const ACTIVITY: &str = include_str!(concat!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
        "MainActivity.kt"
    ));
    const PICKS: &str = include_str!(concat!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
        "SourcePicks.kt"
    ));
    const SERVICE: &str = include_str!(concat!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
        "EnvoixHostService.kt"
    ));
    assert!(
        !code_only(ACTIVITY).contains("takePersistableUriPermission("),
        "an unowned picker result retained a grant"
    );
    for fact in [
        "takePersistableUriPermission(",
        "releasePersistableUriPermission(",
        "putString(card,",
        "remove(card)",
    ] {
        assert!(
            code_only(PICKS).contains(fact),
            "SourcePicks lacks `{fact}`"
        );
    }
    for fact in [
        "NativeHost.pollSourceRelease()",
        "SourcePicks.release(this, removedCard)",
    ] {
        assert!(
            code_only(SERVICE).contains(fact),
            "the service never completes `{fact}`"
        );
    }
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

/// The frontend lane's channel name is ONE name, spelled in three places that
/// no compiler relates: this crate derives it, Kotlin opens the channel with it
/// and Dart listens on it. A typo in either literal is a lane that silently
/// never delivers, which is why the identifier catalog owns the value and this
/// pins the two literals to it.
#[test]
fn frontend_lane_channel_is_one_name() {
    const GRADLE: &str =
        include_str!("../../../../apps/envoix-flutter/android/app/build.gradle.kts");
    const KOTLIN: &str = include_str!(concat!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
        "FrontendLane.kt"
    ));
    const DART: &str = include_str!("../../../../apps/envoix-flutter/lib/lane.dart");

    fn literal(text: &str, declaration: &str, quote: char) -> String {
        text.split_once(declaration)
            .unwrap_or_else(|| panic!("{declaration:?} is declared"))
            .1
            .split(quote)
            .nth(1)
            .unwrap_or_else(|| panic!("{declaration:?} assigns a string literal"))
            .to_owned()
    }

    let namespace = literal(GRADLE, "namespace = ", '"');
    let expected = crate::identifiers::frontend_lane_channel(&namespace);
    assert_eq!(literal(KOTLIN, "const val CHANNEL = ", '"'), expected);
    assert_eq!(literal(DART, "const String laneChannel = ", '\''), expected);

    // The command direction is a second slot in the same namespace, and a typo
    // in either literal is a tap that silently never reaches the host.
    let commands = crate::identifiers::frontend_command_channel(&namespace);
    assert_ne!(commands, expected, "two directions, two slots");
    assert_eq!(
        literal(KOTLIN, "const val COMMAND_CHANNEL = ", '"'),
        commands
    );
    assert_eq!(
        literal(DART, "const String commandChannel = ", '\''),
        commands
    );
    // And each method name, spelled the same on both sides. The pick is a
    // platform capability rather than a transfer verb, but a typo in either
    // literal is a "Choose a file" button that silently does nothing.
    assert_eq!(
        literal(KOTLIN, "const val INTENT = ", '"'),
        literal(DART, "const String intentMethod = ", '\'')
    );
    assert_eq!(
        literal(KOTLIN, "const val PICK_SOURCE = ", '"'),
        literal(DART, "const String pickSourceMethod = ", '\'')
    );
    assert_ne!(
        literal(KOTLIN, "const val PICK_SOURCE = ", '"'),
        literal(KOTLIN, "const val INTENT = ", '"'),
        "two methods, two names"
    );
    // The pick's reply keys, which the Dart side reads back by name. A key the
    // platform sends under one name and Dart reads under another is a picked
    // file that always looks unnamed and zero bytes long.
    for declaration in ["const val DISPLAY_NAME = ", "const val SIZE_BYTES = "] {
        let key = literal(KOTLIN, declaration, '"');
        assert!(
            DART.contains(&format!("granted['{key}']")),
            "the Dart lane never reads the `{key}` the platform sends"
        );
    }
    // And the reply carries THOSE TWO KEYS AND NOTHING ELSE. This is what makes
    // "Dart never holds a URI" a checked property rather than a promise: the
    // only thing that could hand one over is this map, and its shape is pinned
    // to the two scalars the frontend is allowed to know.
    let reply = code_only(KOTLIN)
        .split_once("mapOf(")
        .expect("the pick answers with a map")
        .1
        .split_once(')')
        .expect("the map literal closes")
        .0
        .to_owned();
    let pairs: Vec<&str> = reply.split(" to ").collect();
    // `a to x, b to y` splits into [a, "x, b", y]: every segment but the last
    // ends with the NEXT key, and the first segment is a key on its own.
    let keys: Vec<&str> = pairs[..pairs.len() - 1]
        .iter()
        .map(|segment| segment.rsplit(',').next().unwrap_or(segment).trim())
        .collect();
    assert_eq!(
        keys.len(),
        2,
        "the pick reply carries {} values, not two: {reply}",
        keys.len()
    );
    for key in keys {
        assert!(
            ["DISPLAY_NAME", "SIZE_BYTES"].contains(&key),
            "the pick reply carries {key}; only sanitized metadata may cross"
        );
    }
}

/// The frontend Kotlin sources: the shim between the Dart lane and the JNI
/// verbs. `EnvoixHostService` and `DutyExecutor` are deliberately NOT here —
/// the service owns the lifetime and executes duties, which is exactly what a
/// frontend may not do.
const FRONTEND_KOTLIN: [(&str, &str); 2] = [
    (
        "FrontendLane.kt",
        include_str!(concat!(
            "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
            "FrontendLane.kt"
        )),
    ),
    (
        "MainActivity.kt",
        include_str!(concat!(
            "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
            "MainActivity.kt"
        )),
    ),
];

const ANDROID_MANIFEST: &str =
    include_str!("../../../../apps/envoix-flutter/android/app/src/main/AndroidManifest.xml");

/// The Kotlin with its comments removed. The rule is about what the frontend
/// CALLS, so prose that names a verb in order to say it is never called must
/// not read as a call.
fn code_only(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find("/*").into_iter().chain(rest.find("//")).min() {
        code.push_str(&rest[..at]);
        let (opening, closing) = if rest[at..].starts_with("/*") {
            ("/*", "*/")
        } else {
            ("//", "\n")
        };
        rest = match rest[at + opening.len()..].find(closing) {
            Some(end) => &rest[at + opening.len() + end + closing.len()..],
            None => "",
        };
        code.push('\n');
    }
    code.push_str(rest);
    code
}

/// Every identifier `text` uses as `owner.<identifier>`.
fn members_of(text: &str, owner: &str) -> BTreeSet<String> {
    let needle = format!("{owner}.");
    text.match_indices(&needle)
        .map(|(at, _)| {
            text[at + needle.len()..]
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect()
        })
        .filter(|name: &String| !name.is_empty())
        .collect()
}

/// Every top-level type and function a generated Kotlin artifact declares.
fn declared_types(generated: &str) -> BTreeSet<String> {
    generated
        .lines()
        .filter_map(|line| {
            let declaration = [
                "enum class ",
                "data class ",
                "sealed class ",
                "class ",
                "object ",
                "fun ",
            ]
            .into_iter()
            .find_map(|keyword| line.strip_prefix(keyword))?;
            let name: String = declaration
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The frontend Kotlin holds every JNI verb in its hand and is checked by
/// nothing else: Dart cannot reach the host at all except through it, so this
/// is the layer that actually decides whether Pillar 7 holds. Putting
/// `NativeHost.shutdown()` in `onCancel` — a Dart isolate ending every transfer
/// in the process — otherwise passes every headless gate this repository has.
///
/// The PERMITTED set is the rule, and it is data: every verb `NativeHost`
/// declares is denied here unless it is named, so a verb added to the lane
/// tomorrow is refused to the frontend by default rather than needing someone
/// to remember a new forbidden string.
#[test]
fn the_frontend_kotlin_speaks_only_the_observer_vocabulary() {
    const NATIVE_HOST: &str = include_str!(concat!(
        "../../../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/",
        "NativeHost.kt"
    ));
    /// Open an attachment, drain its frames, and hand over ONE
    /// frontend-originated intent frame. Everything else on the lane is a
    /// lifetime verb — `boot` and `shutdown` decide whether transfers exist at
    /// all, `pollWork` and `reportDuty` are the service's own duty loop — and a
    /// frontend has none of those. `intent` is a mutation the AUTHORITY
    /// resolves: the frontend hands over bytes and is told what happened, which
    /// is the opposite of deciding.
    const PERMITTED_VERBS: [&str; 3] = ["attach", "pollFrame", "intent"];
    /// The token constant the lane compares against; not a verb.
    const PERMITTED_CONSTANTS: [&str; 1] = ["NO_ATTACHMENT"];
    /// A cold launch has to start something and this is the only entry point a
    /// user has. Ending the service is the one thing a frontend must not spell.
    const PERMITTED_SERVICE_CONTROL: [&str; 1] = ["startForegroundService"];

    let declared: BTreeSet<String> = NATIVE_HOST
        .lines()
        .filter_map(|line| line.trim().strip_prefix("external fun "))
        .map(|declaration| {
            declaration
                .chars()
                .take_while(|character| character.is_alphanumeric())
                .collect()
        })
        .collect();
    // Vacuity: the set this test denies from has to be the real lane.
    for verb in [
        "boot",
        "shutdown",
        "reportDuty",
        "pollWork",
        "pollSourceRelease",
    ] {
        assert!(
            declared.contains(verb),
            "{verb} is no longer a NativeHost verb; this test denies from the wrong set"
        );
    }
    for verb in PERMITTED_VERBS {
        assert!(
            declared.contains(verb),
            "{verb} is permitted to the frontend but is not a verb at all"
        );
    }

    let generated: BTreeSet<String> = declared_types(include_str!(
        "../../../l5/envoix-bindings/generated/kotlin/EnvoixRead.kt"
    ))
    .union(&declared_types(include_str!(
        "../../../l5/envoix-bindings/generated/kotlin/EnvoixCommand.kt"
    )))
    .cloned()
    .collect();
    assert!(
        generated.contains("ReadFrame") && generated.contains("EnvoixCommandCodec"),
        "the generated contract types were not read"
    );

    let mut reached = BTreeSet::new();
    for (file, prose) in FRONTEND_KOTLIN {
        let source = &code_only(prose);
        for member in members_of(source, "NativeHost") {
            assert!(
                PERMITTED_VERBS.contains(&member.as_str())
                    || PERMITTED_CONSTANTS.contains(&member.as_str()),
                "{file} reaches NativeHost.{member}, which an observer may not spell"
            );
            reached.insert(member);
        }
        for call in [
            "startService",
            "stopService",
            "bindService",
            "unbindService",
            "stopSelf",
        ] {
            assert!(
                PERMITTED_SERVICE_CONTROL.contains(&call) || !source.contains(call),
                "{file} calls {call}: the frontend owns no lifetime (Pillar 7)"
            );
        }
        for kind in &generated {
            assert!(
                !source.contains(kind.as_str()),
                "{file} names the generated type {kind}: Kotlin shuttles bytes and \
                 decodes nothing"
            );
        }
    }
    // A lane that never attaches, or never polls, is a frontend that shows an
    // empty screen forever — and would satisfy every denial above.
    for verb in PERMITTED_VERBS {
        assert!(
            reached.contains(verb),
            "the frontend never calls NativeHost.{verb}"
        );
    }
}

/// One Activity, one engine, one lane, one consumer of a destructive queue.
///
/// A LAUNCHER activity has to be exported, so the launch mode is the mechanism:
/// `singleInstance` means the system holds at most one instance of it, alone in
/// its task, however it is started. The attachment token makes a second
/// consumer's frames a typed refusal rather than silent theft; this removes the
/// second consumer.
#[test]
fn one_activity_means_one_frontend() {
    let activity = ANDROID_MANIFEST
        .split_once("android:name=\".MainActivity\"")
        .expect("the manifest declares MainActivity")
        .1
        .split_once("</activity>")
        .expect("the activity declaration closes")
        .0;
    assert!(
        activity.contains("android:launchMode=\"singleInstance\""),
        "MainActivity must be launch-bounded to a single instance"
    );
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

    // Handling a kind is not executing it honestly. The source duty's whole
    // job is to answer whether a readable source is actually held for the card,
    // so its arm must consult the platform rather than report success on
    // arrival — the difference between "the file is there" and "we were asked".
    // A device is what proves the Android calls behave; this proves the Kotlin
    // asks the question at all, which is the half that can rot silently.
    let bind = KOTLIN_EXECUTOR
        .split_once("private fun bindSource(")
        .expect("the executor binds a picked source")
        .1
        .split_once("\n    }")
        .expect("the function body ends")
        .0;
    for required in ["SourcePicks.claim(", "SourcePicks.readable("] {
        assert!(
            bind.contains(required),
            "bindSource never reaches {required}: it reports without asking"
        );
    }
    // Both ways of not holding a readable source — no pick at all, and a pick
    // whose grant has gone — answer `source_unreadable`, and exactly one path
    // answers `completed`. Counting rather than matching a shape, so reformatting
    // the arm is free while collapsing a failure into success is not.
    assert_eq!(
        bind.matches("\"source_unreadable\"").count(),
        2,
        "bindSource has {} unreadable answers, not two: {bind}",
        bind.matches("\"source_unreadable\"").count()
    );
    assert_eq!(
        bind.matches("\"completed\"").count(),
        1,
        "bindSource claims success on more than the one path that earns it"
    );

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
