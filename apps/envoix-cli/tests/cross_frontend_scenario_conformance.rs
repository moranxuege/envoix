//! F3: two real frontend witnesses over one generated contract.
//!
//! The authority process is the same `Host` in both runs. Flutter drives its
//! generated Dart artifact and app attachment; the CLI drives its generated
//! Rust artifact and executable. The test normalizes only authority-minted
//! identities and display choices, then compares the product facts each
//! frontend observed.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use envoix_bindings::capability::{
    CapabilityBody, CapabilityStepView, DeclinedView, decode_capability_frame,
};
use envoix_bindings::command::{
    AcceptanceView, CommandBody, CompletionView, CreateOutcomeView, DispositionView, PauseCauseView,
};
use envoix_bindings::read::{
    CardUpdateKindView, CardView, CommandKindView, DirectionView, PauseOriginView,
    ProductStateView, QuiescenceView, ReadBody, decode_read_frame,
};
use envoix_cli::{Frontend, Ingested};
use envoix_host_android::{AttachmentToken, FramePoll, Host};

const NAME: &str = "quarterly report.pdf";
const TOTAL: u64 = 4096;
const TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn cross_frontend_scenario_conformance() {
    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("f3-cross-frontend");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create conformance work directory");

    let cli = drive_cli(&work.join("cli"));
    let flutter = drive_flutter(&work.join("flutter"));

    assert_eq!(
        cli, flutter,
        "the generated Rust and Dart frontends observed different product truth"
    );
}

#[test]
fn local_cli_declines_camera_scanning_as_unsupported() {
    let answer = run_cli_frame(&["capability", "scan-invite"], &[]);
    let CapabilityBody::Exchange(exchange) = decode_capability_frame(&answer)
        .expect("the CLI emits a generated capability answer")
        .body;
    assert_eq!(
        exchange.step,
        CapabilityStepView::Declined(envoix_bindings::capability::DeclinedReasonView {
            reason: DeclinedView::Unsupported,
        })
    );
}

fn drive_cli(work: &Path) -> String {
    fs::create_dir_all(work).expect("create CLI witness directory");
    let root = tempfile::tempdir().expect("CLI authority root");
    let host = Host::boot(root.path()).expect("the CLI authority boots");
    let token = host.open_lane();

    // A real CLI process emits the generated create frame. It owns the request
    // identity, not the authority-minted card identity in the answer.
    let create = run_cli_frame(
        &[
            "send",
            "10000000000000000000000000000001",
            NAME,
            &TOTAL.to_string(),
        ],
        &[],
    );
    let created = host
        .intent(&create)
        .expect("the authority answers the CLI create");
    let opening = drain_until(&host, token, |frames| {
        frames.iter().filter_map(card_view).any(|view| {
            view.offered_name == NAME && view.allowed_actions.contains(&CommandKindView::Cancel)
        })
    });
    let card = created_card_through_cli(&created);

    // A second CLI process is seeded only by generated frames, consults the
    // published offer, emits a generated command, and exits. The Host remains.
    let submit = run_cli_frame(
        &[
            "command",
            &card,
            "10000000000000000000000000000002",
            "cancel",
        ],
        frame_lines(&opening).as_bytes(),
    );
    let accepted = host
        .intent(&submit)
        .expect("the authority answers the CLI command");
    let settled = drain_until(&host, token, |frames| {
        frames.iter().any(is_completion)
            && frames
                .iter()
                .filter_map(card_view)
                .any(is_quiescent_cancelled)
    });

    // The commanding process is already gone. Opening a fresh attachment
    // supersedes only its observation lane; no transfer verb exists here.
    let reattached = host.open_lane();
    let final_frames = drain_until(&host, reattached, |frames| {
        frames
            .iter()
            .filter_map(card_view)
            .any(is_quiescent_cancelled)
    });
    let rendered = run_cli(&["observe"], frame_lines(&final_frames).as_bytes());
    assert!(
        String::from_utf8(rendered)
            .expect("CLI rendering is UTF-8")
            .contains("state=Cancelled"),
        "the reattached CLI did not render the surviving cancelled card"
    );

    let witness = rust_witness(&created, &opening, &accepted, &settled, &final_frames);
    host.shutdown();
    witness
}

fn drive_flutter(work: &Path) -> String {
    fs::create_dir_all(work).expect("create Flutter witness directory");
    let root = tempfile::tempdir().expect("Flutter authority root");
    let host = Host::boot(root.path()).expect("the Flutter authority boots");
    let token = host.open_lane();

    run_dart("create", work);
    let create = fs::read(work.join("create.frame")).expect("Flutter emitted a create frame");
    let created = host
        .intent(&create)
        .expect("the authority answers the Flutter create");
    fs::write(work.join("create.result"), &created).expect("write the create answer");
    let opening = drain_until(&host, token, |frames| {
        frames.iter().filter_map(card_view).any(|view| {
            view.offered_name == NAME && view.allowed_actions.contains(&CommandKindView::Cancel)
        })
    });
    fs::write(work.join("opening.frames"), frame_lines(&opening))
        .expect("write Flutter opening frames");

    // The Dart process uses the app's Attachment and generated command encoder,
    // then exits. The authority remains alive in this Rust process.
    run_dart("command", work);
    let submit = fs::read(work.join("submit.frame")).expect("Flutter emitted a command frame");
    let accepted = host
        .intent(&submit)
        .expect("the authority answers the Flutter command");
    fs::write(work.join("accepted.frame"), &accepted).expect("write the command answer");
    let settled = drain_until(&host, token, |frames| {
        frames.iter().any(is_completion)
            && frames
                .iter()
                .filter_map(card_view)
                .any(is_quiescent_cancelled)
    });
    fs::write(work.join("settled.frames"), frame_lines(&settled))
        .expect("write Flutter settled frames");

    let reattached = host.open_lane();
    let final_frames = drain_until(&host, reattached, |frames| {
        frames
            .iter()
            .filter_map(card_view)
            .any(is_quiescent_cancelled)
    });
    fs::write(work.join("reattached.frames"), frame_lines(&final_frames))
        .expect("write Flutter reattachment frames");
    run_dart("witness", work);
    let witness =
        fs::read_to_string(work.join("witness.txt")).expect("Flutter wrote its product witness");
    host.shutdown();
    witness
}

fn rust_witness(
    created: &[u8],
    opening: &[String],
    accepted: &[u8],
    settled: &[String],
    reattached: &[String],
) -> String {
    let mut initial = Frontend::default();
    let created = match initial
        .ingest(created)
        .expect("CLI decodes the create answer")
    {
        Ingested::Command(frame) => match frame.body {
            CommandBody::CreateResult(result) => result.outcome,
            body => panic!("the create answer was {body:?}"),
        },
        Ingested::Read(frame) => panic!("the create answer was a read frame: {frame:?}"),
    };
    for frame in opening {
        initial
            .ingest(frame.as_bytes())
            .expect("CLI admits an opening frame");
    }
    let (card, before) = initial.cards().next().expect("CLI observed one card");
    assert_eq!(initial.cards().count(), 1, "CLI observed exactly one card");
    let card = card.to_owned();
    let before = before.clone();

    let acceptance = match initial
        .ingest(accepted)
        .expect("CLI decodes the acceptance")
    {
        Ingested::Command(frame) => match frame.body {
            CommandBody::Acceptance(answer) => answer.acceptance,
            body => panic!("the acceptance was {body:?}"),
        },
        Ingested::Read(frame) => panic!("the acceptance was a read frame: {frame:?}"),
    };
    let mut completion = None;
    for frame in settled {
        if let Ingested::Command(command) = initial
            .ingest(frame.as_bytes())
            .expect("CLI admits a settled frame")
            && let CommandBody::Completion(answer) = command.body
        {
            completion = Some(answer.completion);
        }
    }

    // A new value has no projection until the authority's new attachment
    // seeds it. No card or command journal crosses this boundary.
    drop(initial);
    let mut fresh = Frontend::default();
    assert_eq!(fresh.cards().count(), 0);
    for frame in reattached {
        fresh
            .ingest(frame.as_bytes())
            .expect("CLI admits a reattachment frame");
    }
    let (fresh_card, after) = fresh.cards().next().expect("the card was re-seeded");
    assert_eq!(fresh.cards().count(), 1, "one card survived CLI exit");
    assert_eq!(fresh_card, card);

    witness_text(
        matches!(created, CreateOutcomeView::Created(_)),
        &before.view,
        acceptance_token(&acceptance),
        completion_token(&completion.expect("the completion arrived")),
        &after.view,
        after.epoch > before.epoch,
        0,
    )
}

fn witness_text(
    created: bool,
    before: &CardView,
    acceptance: String,
    completion: String,
    after: &CardView,
    epoch_advanced: bool,
    fresh_commands: usize,
) -> String {
    [
        format!("created={created}"),
        format!("direction={}", direction_token(before.direction)),
        format!("name={}", before.offered_name),
        format!("total={}", before.total),
        format!("before_state={}", state_token(&before.state)),
        format!(
            "before_allowed={}",
            before
                .allowed_actions
                .iter()
                .copied()
                .map(command_token)
                .collect::<Vec<_>>()
                .join(",")
        ),
        format!("invite={}", before.invite.is_some()),
        format!("acceptance={acceptance}"),
        format!("completion={completion}"),
        format!("after_state={}", state_token(&after.state)),
        format!("after_quiescence={}", quiescence_token(&after.quiescence)),
        format!(
            "after_allowed={}",
            after
                .allowed_actions
                .iter()
                .copied()
                .map(command_token)
                .collect::<Vec<_>>()
                .join(",")
        ),
        "card_count=1".to_owned(),
        format!("epoch_advanced={epoch_advanced}"),
        format!("fresh_commands={fresh_commands}"),
    ]
    .join("\n")
        + "\n"
}

fn run_cli_frame(arguments: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut output = run_cli(arguments, stdin);
    while matches!(output.last(), Some(b'\n' | b'\r')) {
        output.pop();
    }
    output
}

fn run_cli(arguments: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_envoix"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the real CLI");
    child
        .stdin
        .take()
        .expect("the CLI stdin is piped")
        .write_all(stdin)
        .expect("write generated frames to the CLI");
    let output = child.wait_with_output().expect("wait for the CLI");
    assert!(
        output.status.success(),
        "CLI {:?} failed ({}):\n{}",
        arguments,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn run_dart(mode: &str, work: &Path) {
    let driver =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/cross_frontend_replay.dart");
    let output = Command::new(dart_sdk())
        .arg("run")
        .arg(driver)
        .arg(mode)
        .arg(work)
        .output()
        .expect("start the Dart frontend witness");
    assert!(
        output.status.success(),
        "Dart {mode} failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn dart_sdk() -> PathBuf {
    std::env::var_os("ENVOIX_DART")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|directory| directory.join("dart"))
                .find(|candidate| candidate.is_file())
        })
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME")?);
            let candidate = home.join("development/flutter/bin/dart");
            candidate.is_file().then_some(candidate)
        })
        .expect("Dart is required for the cross-frontend conformance witness")
}

fn created_card_through_cli(reply: &[u8]) -> String {
    let mut frontend = Frontend::default();
    let Ingested::Command(frame) = frontend
        .ingest(reply)
        .expect("CLI decodes the authority's create answer")
    else {
        panic!("the create answer was not a command frame");
    };
    let CommandBody::CreateResult(result) = frame.body else {
        panic!("the create answer was not a create result");
    };
    let CreateOutcomeView::Created(created) = result.outcome else {
        panic!("the authority refused the conformance create");
    };
    created.card
}

fn drain_until(
    host: &Host,
    token: AttachmentToken,
    ready: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        match host.poll_frame(token) {
            FramePoll::Frame(bytes) => {
                frames.push(String::from_utf8(bytes).expect("contract frames are UTF-8"));
            }
            FramePoll::Drained if ready(&frames) => return frames,
            FramePoll::Drained => std::thread::sleep(Duration::from_millis(25)),
            FramePoll::Superseded => panic!("the conformance attachment was superseded early"),
        }
    }
    panic!(
        "the authority did not publish the scenario state:\n{}",
        frames.join("\n")
    );
}

fn card_view(frame: &String) -> Option<CardView> {
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

fn is_completion(frame: &String) -> bool {
    matches!(
        envoix_bindings::command::decode_command_frame(frame.as_bytes()).map(|frame| frame.body),
        Ok(CommandBody::Completion(_))
    )
}

fn is_quiescent_cancelled(view: CardView) -> bool {
    view.state == ProductStateView::Cancelled && view.quiescence == QuiescenceView::Quiescent
}

fn frame_lines(frames: &[String]) -> String {
    frames.join("\n") + "\n"
}

fn direction_token(direction: DirectionView) -> &'static str {
    match direction {
        DirectionView::Send => "send",
        DirectionView::Receive => "receive",
    }
}

fn command_token(command: CommandKindView) -> &'static str {
    match command {
        CommandKindView::Pause => "pause",
        CommandKindView::Cancel => "cancel",
        CommandKindView::Resume => "resume",
        CommandKindView::Remove => "remove",
        CommandKindView::RePickSource => "re_pick_source",
    }
}

fn state_token(state: &ProductStateView) -> String {
    match state {
        ProductStateView::Preparing => "preparing".to_owned(),
        ProductStateView::Waiting => "waiting".to_owned(),
        ProductStateView::Connecting => "connecting".to_owned(),
        ProductStateView::Verifying => "verifying".to_owned(),
        ProductStateView::Transferring => "transferring".to_owned(),
        ProductStateView::Confirming => "confirming".to_owned(),
        ProductStateView::Paused(paused) => format!(
            "paused:{}",
            match paused.origin {
                PauseOriginView::Local => "local",
                PauseOriginView::Peer => "peer",
                PauseOriginView::Lost => "lost",
            }
        ),
        ProductStateView::Unconfirmed => "unconfirmed".to_owned(),
        ProductStateView::Completed => "completed".to_owned(),
        ProductStateView::Failed => "failed".to_owned(),
        ProductStateView::Cancelled => "cancelled".to_owned(),
    }
}

fn quiescence_token(quiescence: &QuiescenceView) -> &'static str {
    match quiescence {
        QuiescenceView::Running(_) => "running",
        QuiescenceView::Retiring(_) => "retiring",
        QuiescenceView::Quiescent => "quiescent",
    }
}

fn acceptance_token(acceptance: &AcceptanceView) -> String {
    match acceptance {
        AcceptanceView::Accepted => "accepted".to_owned(),
        AcceptanceView::Duplicate(disposition) => {
            format!("duplicate:{}", disposition_token(disposition))
        }
        AcceptanceView::Conflict(command) => format!("conflict:{command:?}"),
        AcceptanceView::Rejected(reason) => format!("rejected:{reason:?}"),
    }
}

fn completion_token(completion: &CompletionView) -> String {
    match completion {
        CompletionView::Committed(disposition) => {
            format!("committed:{}", disposition_token(disposition))
        }
        CompletionView::CommitFailed(disposition) => {
            format!("commit_failed:{}", disposition_token(disposition))
        }
        CompletionView::Interrupted => "interrupted".to_owned(),
        CompletionView::Internal => "internal".to_owned(),
    }
}

fn disposition_token(disposition: &DispositionView) -> String {
    match disposition {
        DispositionView::Preparing => "preparing".to_owned(),
        DispositionView::Waiting => "waiting".to_owned(),
        DispositionView::Connecting => "connecting".to_owned(),
        DispositionView::Verifying => "verifying".to_owned(),
        DispositionView::Transferring => "transferring".to_owned(),
        DispositionView::Confirming => "confirming".to_owned(),
        DispositionView::Paused(paused) => format!(
            "paused:{}",
            match paused.origin {
                PauseCauseView::Local => "local",
                PauseCauseView::Peer => "peer",
                PauseCauseView::Lost => "lost",
            }
        ),
        DispositionView::Unconfirmed => "unconfirmed".to_owned(),
        DispositionView::Completed => "completed".to_owned(),
        DispositionView::Failed => "failed".to_owned(),
        DispositionView::Cancelled => "cancelled".to_owned(),
    }
}
