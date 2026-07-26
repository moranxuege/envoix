//! F1b: the Flutter attachment, against a host that is actually running.
//!
//! The frames the Dart side is fed here are produced by booting a real `Host`
//! over real durable storage, creating a real card and draining the real frame
//! pump — not hand-written fixtures. What the Dart side then does with them is
//! executed, not asserted about: the app's own `Attachment` runs in a Dart VM
//! and reports what it surfaced.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use envoix_bindings::command::{COMMAND_SCHEMA_ID, CommandBody, decode_command_frame};
use envoix_bindings::lag_frame;
use envoix_bindings::read::{
    CardUpdateKindView, CardUpdateView, CardView, CommandKindView, DiagnosticsStatusView,
    EvidenceTimelineView, ProductStateView, QuiescenceView, READ_SCHEMA_ID, ReadBody, ReadFrame,
    decode_read_frame, encode_read_frame,
};
use envoix_host_android::{AttachmentToken, FramePoll, Host};
use envoix_runtime::LosslessUpdateKind;
use envoix_types::RecordId;

/// Set this to run the host's gates without the Dart replay. It is named in
/// the failure message on purpose: a decode gate that skips itself when a
/// toolchain is missing proves nothing about the frontend.
const SKIP_DART: &str = "ENVOIX_FLUTTER_SKIP_DART";

/// The one card this test and the on-device instrumentation both create.
const OFFERED_NAME: &str = "f1b-card.bin";
const TOTAL_BYTES: u64 = 4096;

/// How long a drain waits for the frame it asserts about. A satisfied drain
/// returns at once, so this only bounds a FAILING one — generous on purpose,
/// because the whole workspace's tests share this machine and a command's
/// commit crosses a real barrier and a real worker teardown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// A frontend attaches to a running host and renders live truth.
///
/// The Rust half boots the host, creates a durable card, opens a frontend
/// attachment and drains the frames that attachment produced. The Dart half is
/// the app's real view model, run over exactly those bytes.
#[test]
fn flutter_attaches_and_decodes_live_frames() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    let card = host
        .create_for_e2e(OFFERED_NAME, TOTAL_BYTES)
        .expect("a durable card is created");

    // Opening the lane IS the attachment: it discards whatever the previous
    // one never drained and restarts every known card at a new epoch.
    let token = host.open_lane();
    // An attach the runtime must refuse: it holds no projection for this card,
    // so the lane carries the typed reason rather than nothing at all.
    host.attach(RecordId::new(0xf1b));
    // The card's diagnostics were recorded before this attachment existed, so
    // waiting for them here is also what proves they are re-stated to it.
    let live = drain_until(&host, token, |frames| {
        frames.iter().any(|frame| is_snapshot(frame))
            && frames.iter().any(|frame| timeline(frame).is_some())
    });
    let opening = live
        .iter()
        .find_map(|frame| match decoded(frame).body {
            ReadBody::CardUpdate(update) => Some(update),
            _ => None,
        })
        .expect("every epoch opens with a snapshot");
    let CardUpdateKindView::Snapshot(view) = opening.kind.clone() else {
        panic!("the first card update of an epoch is its snapshot");
    };
    assert!(
        live.iter()
            .any(|frame| matches!(decoded(frame).body, ReadBody::SubscribeRejected(_))),
        "a refused attach is typed truth on the lane, not silence"
    );

    // Re-stamped from the host's OWN frame, through the generated codec: an
    // update this attachment's epoch would admit, and one from an epoch it
    // never opened.
    let restamp = |epoch: u64, kind: CardUpdateKindView| {
        encode(&ReadFrame {
            body: ReadBody::CardUpdate(CardUpdateView {
                epoch,
                card: opening.card.clone(),
                kind,
            }),
        })
    };
    let progress = restamp(opening.epoch, CardUpdateKindView::Progress(view.clone()));
    let stale = restamp(opening.epoch + 1, CardUpdateKindView::Progress(view));
    let lag = encode(&lag_frame(
        opening.epoch,
        card,
        LosslessUpdateKind::Terminal,
    ));
    let closed = encode(&envoix_bindings::closed_frame(opening.epoch, card));

    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("frontend-lane");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create the replay directory");
    fs::write(work.join("live.frames"), live.join("\n")).expect("write the live frames");
    for (name, frame) in [
        ("progress.frame", &progress),
        ("stale.frame", &stale),
        ("lag.frame", &lag),
        ("closed.frame", &closed),
    ] {
        fs::write(work.join(name), frame).expect("write a frame");
    }

    if std::env::var_os(SKIP_DART).is_some() {
        eprintln!("{SKIP_DART} is set: the Dart replay did NOT run");
        return;
    }
    let dart = dart_sdk();
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/lane_replay.dart");
    let output = Command::new(&dart)
        .arg("run")
        .arg(&driver)
        .arg(&work)
        .arg(format!("{:016x}", card.get()))
        .arg(OFFERED_NAME)
        .arg(TOTAL_BYTES.to_string())
        .output()
        .unwrap_or_else(|error| panic!("{} did not start: {error}", dart.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the Dart lane replay failed ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{stdout}");
}

/// Flutter re-subscribes by calling `onCancel(null)` and then `onListen(...)`
/// on ONE thread microseconds apart, so the pump of the attachment being
/// replaced is still awake while the next attachment opens. Sharing one stop
/// flag between them, the old thread kept calling the DESTRUCTIVE poll and ate
/// the opening snapshot the new attachment was waiting for — after which the
/// card never appeared at all.
///
/// The token is what makes that unrepresentable rather than a matter of which
/// thread wakes first: a superseded consumer is REFUSED, and refused loudly
/// enough to stop.
#[test]
fn a_superseded_attachment_cannot_consume_a_frame() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    host.create_for_e2e(OFFERED_NAME, TOTAL_BYTES)
        .expect("a durable card is created");

    let replaced = host.open_lane();
    let current = host.open_lane();
    assert_ne!(replaced, current, "each attachment is its own identity");

    // The zombie polls hard, exactly as the leaked thread did.
    for _ in 0..64 {
        assert_eq!(host.poll_frame(replaced), FramePoll::Superseded);
    }
    // A token this host never issued is refused the same way, so a frontend
    // that polls without attaching cannot consume either.
    assert_eq!(
        host.poll_frame(AttachmentToken::NONE),
        FramePoll::Superseded
    );

    // The snapshot the live attachment opens with survived all of it.
    let live = drain(&host, current);
    assert!(
        live.iter().any(|frame| matches!(
            decoded(frame).body,
            ReadBody::CardUpdate(CardUpdateView {
                kind: CardUpdateKindView::Snapshot(_),
                ..
            })
        )),
        "the superseded pump destroyed the epoch's opening snapshot"
    );
    assert_eq!(host.poll_frame(replaced), FramePoll::Superseded);
}

/// A card's diagnostics outlive the frontend that watched them.
///
/// Evidence is recorded whenever the authority commits, which is almost never
/// while an observer happens to be attached — the card here is created before
/// any attachment exists, and the second attachment sees nothing new happen at
/// all. An observer told only about changes made after it arrived would show an
/// empty log beside a card with a whole history, so a fresh attachment is
/// re-told every timeline the host still holds.
#[test]
fn a_fresh_attachment_is_told_the_diagnostics_it_missed() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    let card = host
        .create_for_e2e(OFFERED_NAME, TOTAL_BYTES)
        .expect("a durable card is created");

    let mut seen = Vec::new();
    for _ in 0..2 {
        let token = host.open_lane();
        let frames = drain_until(&host, token, |frames| {
            frames.iter().any(|frame| timeline(frame).is_some())
        });
        let evidence = frames
            .iter()
            .find_map(|frame| timeline(frame))
            .expect("the lane carries the card's timeline");
        assert_eq!(evidence.session.card, format!("{:016x}", card.get()));
        assert!(
            !evidence.entries.is_empty(),
            "a timeline with no entries states nothing"
        );
        assert_eq!(
            evidence.status,
            DiagnosticsStatusView::Complete,
            "nothing was dropped, so nothing may be claimed dropped"
        );
        seen.push(evidence);
    }
    assert_eq!(
        seen[0].entries.len(),
        seen[1].entries.len(),
        "re-attaching re-states the timeline; it does not restart or extend it"
    );
}

/// F2a's invariant test: a gesture becomes a durable effect, and the app can
/// still tell the truth about it after the isolate that made it is gone.
///
/// Everything here is real. The host is a real `Host` over real durable
/// storage; the affordances come from the authority's own `allowed_commands`,
/// published in the read contract; the submit frames are encoded by the APP's
/// encoder in a Dart VM; the acceptance, the completion, the duplicate, the
/// conflict and the stale-epoch refusal are what the running host answered; and
/// the hot restart is a fresh attachment, which is exactly what a restarted
/// isolate opens. The app's own view model then reports what it surfaced.
#[test]
fn flutter_mutating_hot_restart_preserves_cards() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    let card = host
        .create_for_e2e(OFFERED_NAME, TOTAL_BYTES)
        .expect("a durable card is created");
    let hex = format!("{:016x}", card.get());

    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("command-lane");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create the replay directory");

    // What the store holds for this card BEFORE anything commands it. The debug
    // probe is the only thing that can tell the device instrumentation a write
    // landed rather than merely happened in memory, so it is read twice here and
    // has to give two different answers — a probe that returned a constant would
    // satisfy the assertion below on its own.
    #[cfg(feature = "e2e-instrumentation")]
    let uncommanded = host.durable_state_for_e2e(card);

    let token = host.open_lane();
    let live = drain(&host, token);
    fs::write(work.join("live.frames"), live.join("\n")).expect("write the live frames");

    if std::env::var_os(SKIP_DART).is_some() {
        eprintln!("{SKIP_DART} is set: the command replay did NOT run");
        return;
    }
    let dart = dart_sdk();
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/command_replay.dart");
    let replay = |mode: &str| {
        let output = Command::new(&dart)
            .arg("run")
            .arg(&driver)
            .arg(mode)
            .arg(&work)
            .arg(&hex)
            .output()
            .unwrap_or_else(|error| panic!("{} did not start: {error}", dart.display()));
        assert!(
            output.status.success(),
            "the Dart command replay ({mode}) failed ({}):\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    };
    // The frontend reads the published legality and encodes what it may send.
    replay("issue");
    let submit = fs::read(work.join("submit.frame")).expect("the app encoded a submit");
    let conflict = fs::read(work.join("conflict.frame")).expect("the app encoded a conflict");

    // Acceptance first, and it is NOT the effect: the committed completion
    // arrives separately, on the frame lane.
    fs::write(work.join("accepted.frame"), host.submit(&submit)).expect("write the acceptance");
    let settled = drain_until(&host, token, |frames| {
        frames.iter().any(|frame| is_completion(frame))
    });
    let completion = settled
        .iter()
        .find(|frame| is_completion(frame))
        .expect("the completion arrived");
    fs::write(work.join("completed.frame"), completion).expect("write the completion");

    // "Committed truth" is a claim about the STORE, so read the store.
    #[cfg(feature = "e2e-instrumentation")]
    {
        assert_ne!(
            uncommanded, "paused",
            "the card began paused; prove nothing"
        );
        assert_eq!(
            host.durable_state_for_e2e(card),
            "paused",
            "the completion committed, so the record on disk must say so"
        );
    }

    // The same identity again, now that its effect is in committed truth.
    fs::write(work.join("duplicate.frame"), host.submit(&submit)).expect("write the duplicate");
    fs::write(work.join("conflict.frame"), host.submit(&conflict)).expect("write the conflict");

    // The hot restart: the isolate dies, the host does not. The next attachment
    // is a new epoch, and the frame the OLD one encoded is refused typed.
    let restarted = host.open_lane();
    assert_ne!(restarted, token);
    fs::write(work.join("stale.frame"), host.submit(&submit)).expect("write the refusal");
    // The card may still have been retiring its worker when this attachment
    // opened, in which case the rest of the story arrives as `state` updates
    // rather than inside the opening snapshot. Both are the same truth.
    let reseeded = drain_until(&host, restarted, |frames| {
        frames
            .iter()
            .filter_map(|frame| card_view(frame))
            .any(|view| {
                matches!(view.state, ProductStateView::Paused(_))
                    && view.quiescence == QuiescenceView::Quiescent
            })
    });
    fs::write(work.join("restart.frames"), reseeded.join("\n")).expect("write the restart frames");

    replay("render");

    // The frontend restart above proves the FRONTEND kept nothing. This proves
    // the other half, which no restart of a surviving process can: the
    // command's effect is on disk, not in the process that applied it. A fresh
    // host over the same root reconstitutes the card from bytes alone — paused,
    // and offering exactly what a paused card may be asked.
    host.shutdown();
    let rebooted = Host::boot(root.path()).expect("the host boots again");
    let reboot_token = rebooted.open_lane();
    let restored = drain(&rebooted, reboot_token)
        .iter()
        .find_map(|frame| card_view(frame))
        .expect("the rebooted host reseeds the card");
    assert!(
        matches!(restored.state, ProductStateView::Paused(_)),
        "a rebooted host restored the card as {:?}, not paused",
        restored.state
    );
    assert!(
        restored.allowed_actions.contains(&CommandKindView::Resume),
        "a restored paused card must still be offered resume, got {:?}",
        restored.allowed_actions
    );
}

/// Whether a frame is a command completion. Read frames share this lane, so a
/// command decode that fails is one of those, not a fault.
fn is_completion(frame: &str) -> bool {
    matches!(
        decode_command_frame(frame.as_bytes()).map(|frame| frame.body),
        Ok(CommandBody::Completion(_))
    )
}

/// Every frame the attachment has queued, as UTF-8 text, drained until the
/// epoch's opening snapshot has arrived.
fn drain(host: &Host, token: AttachmentToken) -> Vec<String> {
    drain_until(host, token, |frames| {
        frames.iter().any(|frame| is_snapshot(frame))
    })
}

/// Drains the lane until `ready` accepts what has arrived. The pump ticks every
/// 50 ms and evidence crosses an asynchronous worker before it is published, so
/// a test waits for the frame it asserts about rather than racing it.
fn drain_until(
    host: &Host,
    token: AttachmentToken,
    ready: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        match host.poll_frame(token) {
            FramePoll::Frame(bytes) => {
                frames.push(String::from_utf8(bytes).expect("frames are UTF-8"));
            }
            FramePoll::Drained if ready(&frames) => break,
            FramePoll::Drained => std::thread::sleep(Duration::from_millis(25)),
            FramePoll::Superseded => panic!("the attachment under test was superseded"),
        }
    }
    assert!(
        ready(&frames),
        "the lane never delivered what this attachment was waiting for; it did deliver:\n{}",
        frames.join("\n")
    );
    frames
}

fn is_snapshot(frame: &str) -> bool {
    matches!(
        decode_read_frame(frame.as_bytes()).map(|frame| frame.body),
        Ok(ReadBody::CardUpdate(CardUpdateView {
            kind: CardUpdateKindView::Snapshot(_),
            ..
        }))
    )
}

/// The card as a frame describes it, whichever update kind carried it. An
/// attachment that opens while the card is still settling is told the rest as
/// `state` updates, so a test waiting for a settled card must read those too.
/// The lane multiplexes both contracts, so a read decode that fails is a
/// command frame, not a fault.
fn card_view(frame: &str) -> Option<CardView> {
    let ReadBody::CardUpdate(update) = decode_read_frame(frame.as_bytes()).ok()?.body else {
        return None;
    };
    match update.kind {
        CardUpdateKindView::Snapshot(view)
        | CardUpdateKindView::Progress(view)
        | CardUpdateKindView::State(view)
        | CardUpdateKindView::Terminal(view) => Some(view),
        CardUpdateKindView::CapabilityDuty(_) => None,
    }
}

fn timeline(frame: &str) -> Option<EvidenceTimelineView> {
    match decode_read_frame(frame.as_bytes()).ok()?.body {
        ReadBody::Evidence(timeline) => Some(timeline),
        _ => None,
    }
}

fn decoded(frame: &str) -> ReadFrame {
    decode_read_frame(frame.as_bytes()).expect("the host emits frames its own contract accepts")
}

fn encode(frame: &ReadFrame) -> String {
    String::from_utf8(encode_read_frame(frame).expect("the frame encodes"))
        .expect("frames are UTF-8")
}

fn dart_sdk() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    std::env::var_os("ENVOIX_DART")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|directory| directory.join("dart"))
                .find(|candidate| candidate.is_file())
        })
        .or_else(|| Some(home.join("development/flutter/bin/dart")).filter(|path| path.is_file()))
        .unwrap_or_else(|| {
            panic!(
                "the Dart SDK was not found. Install it, point ENVOIX_DART at it, or set \
                 {SKIP_DART}=1 to run this crate's gates without the replay — which leaves \
                 the frontend's decoding unproven."
            )
        })
}

/// The app consumes BOTH generated bindings BY REFERENCE. A copy would compile
/// and then rot: the point of a generated contract is that there is one of it,
/// and `flutter analyze` must be analysing the artifacts this workspace emits
/// rather than snapshots of them.
#[test]
fn the_flutter_app_reads_the_generated_binding_itself() {
    let repository = repository_root();
    for artifact in ["envoix_read.dart", "envoix_command.dart"] {
        let linked = repository
            .join("apps/envoix-flutter/lib/bindings")
            .join(artifact);
        assert!(
            fs::symlink_metadata(&linked)
                .unwrap_or_else(|_| panic!("the app links {artifact}"))
                .file_type()
                .is_symlink(),
            "{} must be a link to the generated artifact, not a copy",
            linked.display()
        );
        assert_eq!(
            fs::canonicalize(&linked).expect("the link resolves"),
            fs::canonicalize(
                repository
                    .join("crates/l5/envoix-bindings/generated/dart")
                    .join(artifact)
            )
            .expect("the generated artifact exists"),
        );
    }
}

/// F2a's frontend commands, so it legitimately carries an encoder — the ONE
/// encoder the command contract emits for a native, because `submit` is the one
/// body a frontend originates (BN3b). This replaces F1b's "no encoder at all"
/// sweep with what actually has to hold now: it carries that encoder and
/// nothing else, and it keeps no durable command state.
///
/// The durable half is gated at the door: a retry ledger or a command journal
/// that outlives the process needs `dart:io` or a storage package, and the
/// app's imports are an allow-list that admits neither. A retry TIMER is the
/// one thing the door lets through — `Timer` lives in `dart:async`, which the
/// lane legitimately imports for its stream — so that one is named. R0 gives
/// the frontend no transfer truth to keep, and the host's own ledger — 256
/// completions per card — is the memory a re-issue is answered from.
#[test]
fn the_mutating_frontend_carries_only_the_submit_encoder() {
    /// Everything the app may import. Anything else is a door to state, to a
    /// second lane onto the host, or to a clock.
    const ALLOWED_IMPORTS: [&str; 3] = ["dart:async", "dart:convert", "dart:math"];

    let repository = repository_root();
    let sources = repository.join("apps/envoix-flutter/lib");
    let mut checked = 0;
    let mut encoders = Vec::new();
    for entry in fs::read_dir(&sources).expect("the app has sources") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|extension| extension != "dart") {
            continue;
        }
        let name = path
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).expect("a Dart source reads");
        if text.contains("encodeCommandFrame") {
            encoders.push(name.clone());
        }
        // The generated encoder writes the JSON. A frontend that writes its own
        // is a second, unversioned dialect of the command contract.
        for forbidden in [
            "jsonEncode(",
            "jsonDecode(",
            READ_SCHEMA_ID,
            COMMAND_SCHEMA_ID,
        ] {
            assert!(
                !text.contains(forbidden),
                "{name} spells `{forbidden}`: frames are the generated codec's, \
                 not this app's"
            );
        }
        // The import allow-list cannot exclude this one: `dart:async` is on it.
        // A frontend that re-presents a command on a clock owns a retry policy,
        // which is the authority's (BN2 R4).
        assert!(
            !text.contains("Timer("),
            "{name} builds a Timer: a re-issue is a user's tap, not a schedule"
        );
        for import in text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("import '"))
            .filter_map(|line| line.split_once('\''))
            .map(|(uri, _)| uri)
        {
            let allowed = ALLOWED_IMPORTS.contains(&import)
                || import.starts_with("package:flutter/")
                || !import.contains(':');
            assert!(allowed, "{name} imports {import}, which is not on the list");
        }
        checked += 1;
    }
    assert!(
        checked >= 8,
        "the app's sources were not swept, {checked} seen"
    );
    // Non-vacuity in both directions: the app must be able to command at all,
    // and the encoder must live in ONE place rather than wherever a widget felt
    // like building a frame.
    assert_eq!(
        encoders,
        vec!["commands.dart".to_owned()],
        "the submit encoder must be reached from exactly one file"
    );

    // The only third-party code in this app is the Flutter SDK. A dependency is
    // how a durable store gets in, so the manifest is part of the same gate.
    let pubspec = fs::read_to_string(repository.join("apps/envoix-flutter/pubspec.yaml"))
        .expect("the app has a pubspec");
    let declared: Vec<&str> = pubspec
        .lines()
        .skip_while(|line| !line.starts_with("dependencies:"))
        .skip(1)
        .take_while(|line| line.starts_with(' ') || line.trim().is_empty())
        .filter_map(|line| {
            // A dependency is a two-space-indented key, with or without a
            // version after it; anything deeper belongs to the entry above.
            let entry = line.strip_prefix("  ")?;
            (!entry.starts_with([' ', '#']))
                .then(|| entry.split(':').next().unwrap_or(entry).trim())
        })
        .collect();
    assert_eq!(declared, vec!["flutter"], "{pubspec}");
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the host crate sits two levels below the repository root")
        .to_path_buf()
}
