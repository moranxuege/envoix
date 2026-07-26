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

use envoix_bindings::lag_frame;
use envoix_bindings::read::{
    CardUpdateKindView, CardUpdateView, DiagnosticsStatusView, EvidenceTimelineView, ReadBody,
    ReadFrame, decode_read_frame, encode_read_frame,
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
    let deadline = Instant::now() + Duration::from_secs(5);
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
        "the lane never delivered what this attachment was waiting for"
    );
    frames
}

fn is_snapshot(frame: &str) -> bool {
    matches!(
        decoded(frame).body,
        ReadBody::CardUpdate(CardUpdateView {
            kind: CardUpdateKindView::Snapshot(_),
            ..
        })
    )
}

fn timeline(frame: &str) -> Option<EvidenceTimelineView> {
    match decoded(frame).body {
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

/// The app consumes the generated binding BY REFERENCE. A copy would compile
/// and then rot: the point of a generated contract is that there is one of it,
/// and `flutter analyze` must be analysing the artifact this workspace emits
/// rather than a snapshot of it.
#[test]
fn the_flutter_app_reads_the_generated_binding_itself() {
    let repository = repository_root();
    let linked = repository.join("apps/envoix-flutter/lib/bindings/envoix_read.dart");
    assert!(
        fs::symlink_metadata(&linked)
            .expect("the app links the generated read binding")
            .file_type()
            .is_symlink(),
        "{} must be a link to the generated artifact, not a copy",
        linked.display()
    );
    assert_eq!(
        fs::canonicalize(&linked).expect("the link resolves"),
        fs::canonicalize(
            repository.join("crates/l5/envoix-bindings/generated/dart/envoix_read.dart")
        )
        .expect("the generated artifact exists"),
    );
}

/// F1b is an observer. The command contract is generated for Dart and stays
/// out of the app: a frontend with no encoder cannot issue a command by
/// accident, which is a stronger statement than a rule saying it must not.
#[test]
fn the_read_only_frontend_carries_no_command_encoder() {
    let sources = repository_root().join("apps/envoix-flutter/lib");
    assert!(
        !sources.join("bindings/envoix_command.dart").exists(),
        "the command binding stays out of the app until F2"
    );
    let mut checked = 0;
    for entry in fs::read_dir(&sources).expect("the app has sources") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|extension| extension != "dart") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("a Dart source reads");
        assert!(
            !text.contains("encodeCommandFrame") && !text.contains("envoix_command"),
            "{} reaches for the command contract; F1b observes",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "the app's sources were not swept, {checked} seen"
    );
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the host crate sits two levels below the repository root")
        .to_path_buf()
}
