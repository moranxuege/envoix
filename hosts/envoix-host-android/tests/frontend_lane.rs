//! F1b: the Flutter attachment, against a host that is actually running.
//!
//! The frames the Dart side is fed here are produced by booting a real `Host`
//! over real durable storage, creating a real card and draining the real frame
//! pump — not hand-written fixtures. What the Dart side then does with them is
//! executed, not asserted about: the app's own `Attachment` runs in a Dart VM
//! and reports what it surfaced.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use envoix_bindings::command::{
    COMMAND_SCHEMA_ID, CommandBody, CommandFrame, CreateIntentView, CreateOutcomeView, CreateView,
    FrontendIntentView, JoinInviteView, SendSourceView, decode_command_frame, encode_command_frame,
};
use envoix_bindings::lag_frame;
use envoix_bindings::read::{
    CardUpdateKindView, CardUpdateView, CardView, CommandKindView, DiagnosticsStatusView,
    EvidenceTimelineView, ProductStateView, QuiescenceView, READ_SCHEMA_ID, ReadBody, ReadFrame,
    decode_read_frame, encode_read_frame,
};
use envoix_host_android::{AttachmentToken, FramePoll, Host};
use envoix_outcomes::OutcomeCode;
use envoix_platform_android::{Work, WorkOrder, WorkReport};
use envoix_runtime::LosslessUpdateKind;
use envoix_types::{RecordId, Secret};

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
    fs::write(
        work.join("accepted.frame"),
        host.intent(&submit).expect("the host accepts the intent"),
    )
    .expect("write the acceptance");
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
    fs::write(
        work.join("duplicate.frame"),
        host.intent(&submit).expect("the host accepts the retry"),
    )
    .expect("write the duplicate");
    fs::write(
        work.join("conflict.frame"),
        host.intent(&conflict)
            .expect("the host accepts the conflict"),
    )
    .expect("write the conflict");

    // The hot restart: the isolate dies, the host does not. The next attachment
    // is a new epoch, and the frame the OLD one encoded is refused typed.
    let restarted = host.open_lane();
    assert_ne!(restarted, token);
    fs::write(
        work.join("stale.frame"),
        host.intent(&submit)
            .expect("the host accepts the stale command"),
    )
    .expect("write the refusal");
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

/// F2b's invariant test: a card comes into existence from the real frontend
/// path, in a configuration where the debug creation symbol does not exist.
///
/// `create_for_e2e` is never called here and `E2eBridge` is not compiled by
/// `cargo test --workspace` at all — the feature that carries it is off by
/// default, which is what makes this configuration release-shaped. The only
/// thing that makes a card is `Host::intent`, fed bytes the APP's own encoder
/// produced, which is exactly what the `intent` JNI verb passes through.
///
/// Both directions are here, and the second is joined with the FIRST one's own
/// published invite: the text the sender's card offers is the text the joiner
/// pastes, so the invite crosses the whole system without any part of the
/// frontend ever parsing one.
#[test]
fn flutter_creates_a_transfer_without_the_debug_bridge() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    let token = host.open_lane();
    assert!(
        host.live_cards().is_empty(),
        "a fresh host has no cards; anything below is this test's doing"
    );

    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("create-lane");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create the replay directory");

    if std::env::var_os(SKIP_DART).is_some() {
        eprintln!("{SKIP_DART} is set: the create replay did NOT run");
        return;
    }
    let dart = dart_sdk();
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/create_replay.dart");
    let replay = |mode: &str| {
        let output = Command::new(&dart)
            .arg("run")
            .arg(&driver)
            .arg(mode)
            .arg(&work)
            .output()
            .unwrap_or_else(|error| panic!("{} did not start: {error}", dart.display()));
        assert!(
            output.status.success(),
            "the Dart create replay ({mode}) failed ({}):\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    };

    // The send: the frontend hands over the sanitized metadata the platform
    // reported for the document it picked, and nothing else.
    replay("ask-send");
    let send = fs::read(work.join("send.frame")).expect("the app encoded a create");
    let result = host
        .intent(&send)
        .expect("the host accepts the send intent");
    fs::write(work.join("send.result"), &result).expect("write the send result");
    let card = created_card(&result).expect("the authority created the send");
    assert_eq!(host.live_cards(), vec![card], "exactly one card exists");

    // The card is durable BEFORE anything else happens to it: a fresh host over
    // the same root finds it, which no in-memory create could satisfy.
    let published = drain_until(&host, token, |frames| {
        frames
            .iter()
            .filter_map(|frame| card_view(frame))
            .any(|view| view.invite.is_some())
    });
    let invite = published
        .iter()
        .filter_map(|frame| card_view(frame))
        .find_map(|view| view.invite)
        .expect("the send publishes an invite");
    let link = invite
        .link
        .clone()
        .expect("the invite has a shareable link");
    // Written out for `f2b-e2e.sh`, which types this exact text into the real
    // app: the invite a device harness pastes is one the core published, never
    // one a script invented. The code rides beside it because it lives inside
    // the link's base64, where a harness cannot read it.
    fs::write(work.join("invite.txt"), link.expose()).expect("write the invite");
    fs::write(work.join("code.txt"), invite.code.expose()).expect("write the room code");
    // The digest a device harness can actually compare against. A release build
    // must never log the room code — it IS the SPAKE2 password — so what reaches
    // the screen is this fingerprint, and a harness holding only `code.txt`
    // has nothing to match it with.
    fs::write(work.join("fingerprint.txt"), &invite.code_fingerprint)
        .expect("write the code fingerprint");

    // Every card this build mints is frozen to the deployment it was compiled
    // for, and the invite a harness pastes therefore names the live rendezvous.
    // Asserted on the PUBLISHED invite rather than on the plan, so this is the
    // endpoint that actually crossed the contract.
    let decoded = envoix_invite::route_invite(link.expose()).expect("the published invite parses");
    assert_eq!(
        decoded.broker(),
        envoix_deployment::BUILD_TARGET.rendezvous_endpoint.as_ref(),
        "a published invite must name the deployment this build is for"
    );

    // The join: the sender's own published invite, pasted back. The authority
    // parses it, chooses the opposite role, and creates the receiving card.
    // Beside it, text that is only a room code — refused typed.
    replay("ask-join");
    for (name, result) in [("join", "join.result"), ("bare", "bare.result")] {
        let frame = fs::read(work.join(format!("{name}.frame"))).expect("the app encoded it");
        fs::write(
            work.join(result),
            host.intent(&frame)
                .expect("the host accepts the create intent"),
        )
        .expect("write the result");
    }
    let joined = created_card(&fs::read(work.join("join.result")).expect("join result"))
        .expect("the authority created the join");
    assert_ne!(joined, card, "a join is its own card");
    assert!(
        created_card(&fs::read(work.join("bare.result")).expect("bare result")).is_none(),
        "a bare room code creates nothing"
    );

    // Everything the lane said about both cards. The first drain already
    // consumed the send card's frames, so the predicate reads what has arrived
    // ACROSS both.
    let so_far = published.clone();
    let rest = drain_until(&host, token, |frames| {
        let both: BTreeSet<String> = so_far
            .iter()
            .chain(frames.iter())
            .filter_map(|frame| card_view(frame))
            .map(|view| view.identity.card)
            .collect();
        both.len() == 2
    });
    let cards: Vec<String> = published.into_iter().chain(rest).collect();
    fs::write(work.join("cards.frames"), cards.join("\n")).expect("write the card frames");
    replay("render");

    // The `SourceHandle` duty round-trips: asked (the host dispatched a work
    // order for it, for THIS card), granted (the service executed it), admitted
    // exactly once. The order is the duty's whole surface to the platform —
    // BN4's pump routes duties to the service rather than also publishing them
    // to observers, so this is where the ask is visible.
    let order = poll_source_order(&host).expect("the host dispatched the source duty");
    let provenance = order.provenance;
    assert_eq!(
        RecordId::new(provenance.card.value()),
        card,
        "the source duty belongs to the card that needs a source"
    );
    let report = WorkReport::new(provenance.to_provenance(), OutcomeCode::Completed);
    let encoded = report.encode().expect("the report encodes");
    assert!(host.report_duty(&encoded), "the first report is admitted");
    assert!(
        !host.report_duty(&encoded),
        "a replayed report is admitted once, never twice"
    );

    // And the cards are on disk, not in this process: a fresh host over the
    // same root brings both back, invites included.
    host.shutdown();
    let rebooted = Host::boot(root.path()).expect("the host boots again");
    let reboot_token = rebooted.open_lane();
    let restored = drain_until(&rebooted, reboot_token, |frames| {
        frames
            .iter()
            .filter_map(|frame| card_view(frame))
            .filter(|view| view.invite.is_some())
            .map(|view| view.identity.card)
            .collect::<BTreeSet<String>>()
            .len()
            == 2
    });
    let codes: BTreeSet<String> = restored
        .iter()
        .filter_map(|frame| card_view(frame))
        .filter_map(|view| view.invite.map(|invite| invite.code.expose().clone()))
        .collect();
    assert_eq!(
        codes,
        BTreeSet::from([invite.code.expose().clone()]),
        "both restored cards carry the one room code the send minted"
    );
}

/// A create id is authority identity, not just frontend correlation. The first
/// durable record stores it in the same commit that creates the card, so a
/// repeated delivery after the process has forgotten all memory answers with
/// that card instead of allocating another one.
#[test]
fn repeated_create_identity_survives_restart_without_creating_another_card() {
    let root = tempfile::tempdir().expect("tempdir");
    let request_id = "0123456789abcdeffedcba9876543210";
    let request = encode_command_frame(&CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent: CreateIntentView::Send(SendSourceView {
                display_name: "one-card.bin".to_owned(),
                total: 4096,
            }),
            request_id: request_id.to_owned(),
        })),
    })
    .expect("a create intent encodes");

    let first = {
        let host = Host::boot(root.path()).expect("first process boots");
        let answer = host
            .intent(&request)
            .expect("the authority accepts the intent");
        assert_eq!(
            host.live_cards().len(),
            1,
            "the first delivery creates one card"
        );
        host.shutdown();
        answer
    };

    let rebooted = Host::boot(root.path()).expect("second process boots");
    let repeated = rebooted
        .intent(&request)
        .expect("the authority accepts the repeated intent");
    assert_eq!(
        repeated, first,
        "the repeated identity gets its original durable answer"
    );
    assert_eq!(
        rebooted.live_cards().len(),
        1,
        "one create identity is one card across process generations"
    );
    rebooted.shutdown();
}

/// Malformed bytes reached the authority and were refused before an intent
/// handler ran. This must remain distinct from a missing host or a lost answer.
#[test]
fn authority_refuses_non_contract_intent_as_a_third_origin() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = Host::boot(root.path()).expect("the host boots");
    assert_eq!(
        host.intent(br#"{"schema":"not-envoix"}"#),
        Err(envoix_host_android::IntentRejection::Contract)
    );
    assert!(
        host.live_cards().is_empty(),
        "a refused frame creates nothing"
    );
    host.shutdown();
}

/// The card a create result names, or `None` for a refusal.
fn created_card(reply: &[u8]) -> Option<RecordId> {
    let CommandBody::CreateResult(result) = decode_command_frame(reply).ok()?.body else {
        return None;
    };
    let CreateOutcomeView::Created(created) = result.outcome else {
        return None;
    };
    u64::from_str_radix(&created.card, 16)
        .ok()
        .map(RecordId::new)
}

/// The first `source_handle` work order the host dispatched to the service.
fn poll_source_order(host: &Host) -> Option<WorkOrder> {
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    while Instant::now() < deadline {
        match host.poll_work() {
            Some(bytes) => {
                let order = WorkOrder::decode(&bytes).expect("the host emits decodable orders");
                if order.work == Work::SourceHandle {
                    return Some(order);
                }
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    None
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

/// F2a's frontend commands and F2b's asks for cards, so it legitimately carries
/// an encoder — the ONE encoder the command contract emits for a native,
/// because `intent` is the one body a frontend originates (BN3b). This replaces
/// F1b's "no encoder at all" sweep with what actually has to hold now: it
/// carries that encoder and nothing else, keeps no durable command state, and
/// cannot recognise an invite.
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
    /// The invite grammar's URI scheme, and the ONE spelling still worth
    /// denying. `XI03`'s heuristic is not on this list any more: a deny-list of
    /// spellings catches `contains('-')` and misses `indexOf`, a regex and a
    /// character loop, and that particular idiom is ordinary Dart that will one
    /// day false-positive on innocent code and be deleted in frustration. What
    /// holds instead is structural (below) and behavioural (`flutter test`:
    /// Join is never disabled, the padded text crosses the whole widget path
    /// unchanged, and every refusal rendered is the authority's own words), and
    /// neither of those cares how a heuristic is spelled.
    ///
    /// The scheme stays because it catches something the structure does not:
    /// nothing forbids Dart from BUILDING a string, so a widget that spells the
    /// outer form can fabricate invite text — a link a user would share and
    /// nobody could join — without ever inferring anything. `envoix://` and
    /// `invite/v` are dropped as redundant: any text containing either
    /// contains this.
    const NO_INVITE_GRAMMAR: [&str; 1] = ["envoix:"];
    /// Everything the app may import. Anything else is a door to state, to a
    /// second lane onto the host, or to a clock.
    const ALLOWED_IMPORTS: [&str; 3] = ["dart:async", "dart:convert", "dart:math"];

    let repository = repository_root();
    let sources = repository.join("apps/envoix-flutter/lib");
    let mut checked = 0;
    let mut encoders = Vec::new();
    let mut builders = Vec::new();
    let mut verdicts = Vec::new();
    let mut carriers = Vec::new();
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
        // The generated encoder is wrapped, so watching only for it would miss
        // a widget that reached for a WRAPPER instead. Both frames a frontend
        // may originate are built by the same two functions, and calling one is
        // building a frame wherever it happens.
        if name != "commands.dart"
            && ["submitFrame(", "createFrame("]
                .into_iter()
                .any(|builder| text.contains(builder))
        {
            builders.push(name.clone());
        }
        // Naming a refusal VALUE is deciding one. The type may be matched on
        // anywhere; a specific verdict is a sentence about the user's text, and
        // the only file entitled to say one is the total map from the
        // authority's answer to words.
        if text.contains("CreateRefusalView.") {
            verdicts.push(name.clone());
        }
        // The generated payload is the only way invite text leaves this app,
        // so wherever it is spelled is where the text can go.
        if text.contains("JoinInviteView(") {
            carriers.push(name.clone());
        }
        // The generated encoder writes the JSON. A frontend that writes its own
        // is a second, unversioned dialect of the command contract. And the
        // invite grammar lives in Rust (`XI02`), so nothing here may know what
        // an invite looks like — including the shape the old app guessed with.
        for forbidden in NO_INVITE_GRAMMAR.into_iter().chain([
            "jsonEncode(",
            "jsonDecode(",
            READ_SCHEMA_ID,
            COMMAND_SCHEMA_ID,
        ]) {
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
        checked >= 9,
        "the app's sources were not swept, {checked} seen"
    );
    // Why parsing an invite in Dart is USELESS rather than merely banned, as
    // two structural facts about this app rather than a list of spellings.
    //
    // There is nowhere to put a verdict: the only vocabulary that says
    // anything about invite text is the generated `CreateRefusalView`, which
    // arrives on the lane, and the one file allowed to name a value of it maps
    // the authority's answer to words. A widget that worked out for itself
    // that a paste was bad could not say so.
    assert_eq!(
        verdicts,
        vec!["labels.dart".to_owned()],
        "an invite verdict is decoded from the authority, never minted here"
    );
    // And there is one way out: the generated payload, built in the lane, from
    // where the whole conversation is visible. Non-vacuity comes with it — the
    // app must really CARRY invite text, or "it cannot parse one" is a claim
    // about an app that never sees one.
    assert_eq!(
        carriers,
        vec!["lane.dart".to_owned()],
        "invite text leaves through the generated payload and nowhere else"
    );
    let creator = fs::read_to_string(sources.join("lane.dart")).expect("the lane reads");
    assert!(
        creator.contains("JoinInviteView(invite:"),
        "the app never carries invite text at all"
    );
    // Non-vacuity in both directions: the app must be able to command at all,
    // and the encoder must live in ONE place rather than wherever a widget felt
    // like building a frame.
    assert_eq!(
        encoders,
        vec!["commands.dart".to_owned()],
        "the intent encoder must be reached from exactly one file"
    );
    // And the frames it wraps are built from exactly one other file: the lane,
    // where `Commander` and `Creator` live. A widget that builds a frame owns a
    // conversation with the authority that nothing else can see.
    assert_eq!(
        builders,
        vec!["lane.dart".to_owned()],
        "a frontend frame is built in the lane, not wherever a widget felt like it"
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

/// The retry the request id CANNOT answer.
///
/// The frontend forms a create id with the intent, which is what makes a resend
/// within one process recognisably the same ask. But that id lives in the sheet
/// the user is looking at, and the sheet dies with the process. So the reported
/// sequence — the answer is lost, the app restarts, the user pastes the same
/// invite again — arrives as a create the authority has never seen, carrying a
/// FRESH id and asking for the room it already joined.
///
/// Only the rendezvous says those two asks are the same thing. Keyed on the id
/// alone this makes a second card frozen to the first one's room; keyed on the
/// endpoint too, the second ask is answered with the first card.
#[test]
fn a_fresh_id_for_a_room_already_joined_does_not_make_a_second_card() {
    let root = tempfile::tempdir().expect("tempdir");
    let sender = tempfile::tempdir().expect("sender tempdir");

    // An invite the CORE published, never one this test invented.
    let link = {
        let host = Host::boot(sender.path()).expect("the sending host boots");
        let token = host.open_lane();
        host.intent(&create_send_frame("shared.bin", 4096))
            .expect("the send is created");
        let published = drain_until(&host, token, |frames| {
            frames
                .iter()
                .filter_map(|frame| card_view(frame))
                .any(|view| view.invite.is_some())
        });
        let link: envoix_types::Secret<String> = published
            .iter()
            .filter_map(|frame| card_view(frame))
            .find_map(|view| view.invite)
            .expect("the send publishes an invite")
            .link
            .clone()
            .expect("the invite has a shareable link");
        host.shutdown();
        link
    };

    // First join, first process, first id.
    {
        let host = Host::boot(root.path()).expect("the joining host boots");
        host.intent(&create_join_frame(
            link.expose(),
            "0123456789abcdeffedcba9876543210",
        ))
        .expect("the first join is accepted");
        assert_eq!(host.live_cards().len(), 1, "the first join makes one card");
        host.shutdown();
    }

    // The answer never arrived, the app died, the user pastes it again. A new
    // sheet mints a new id, because nothing durable on the frontend remembers.
    let rebooted = Host::boot(root.path()).expect("the joining host boots again");
    rebooted
        .intent(&create_join_frame(
            link.expose(),
            "fedcba98765432100123456789abcdef",
        ))
        .expect("the repeated join is accepted");
    assert_eq!(
        rebooted.live_cards().len(),
        1,
        "a second id for a room already joined must answer with the first card, \
         not freeze a second card to the same rendezvous"
    );
}

fn create_send_frame(name: &str, total: u64) -> Vec<u8> {
    encode_command_frame(&CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent: CreateIntentView::Send(SendSourceView {
                display_name: name.to_owned(),
                total,
            }),
            request_id: "00000000000000000000000000000001".to_owned(),
        })),
    })
    .expect("a send intent encodes")
}

fn create_join_frame(link: &str, request_id: &str) -> Vec<u8> {
    encode_command_frame(&CommandFrame {
        body: CommandBody::Intent(FrontendIntentView::Create(CreateView {
            intent: CreateIntentView::Join(JoinInviteView {
                invite: Secret::new(link.to_owned()),
            }),
            request_id: request_id.to_owned(),
        })),
    })
    .expect("a join intent encodes")
}
