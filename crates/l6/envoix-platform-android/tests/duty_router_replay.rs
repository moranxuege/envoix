//! EH-25's evidence half: the shipped Kotlin router, compiled and RUN against
//! order frames the Rust authority encoded.
//!
//! Everything else about this lane is an argument. The schema emits both
//! codecs, so they ought to agree; the `when` is over a sealed type, so it
//! ought to be exhaustive. Those are good arguments and they are exactly the
//! kind that the gate this replaces also made — it compared `when` labels in
//! source text and stayed green while Rust wrote a notice as a string and
//! Kotlin read it as an object. Only executing the two together can tell the
//! difference between agreeing and appearing to.
//!
//! The toolchain lookup is duplicated from the bindings crate's conformance
//! test rather than shared. Two tests in two crates that each state what they
//! need is the lesser evil against a helper crate whose only purpose is to hold
//! sixty lines of `which`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use envoix_bindings::duty::{
    DutyBody, DutyFrame, DutyOrderView, DutyProvenanceView, ForegroundWorkView, LockDirectiveView,
    LockWorkView, NoticeView, NotificationWorkView, OutcomeCodeView, PublicationWorkView, WorkView,
    decode_duty_frame, encode_duty_frame,
};

/// Skips the replay. Named in the failure message on purpose: a gate that
/// silently skips itself when a toolchain is missing is how this lane's last
/// gate came to mean nothing.
const SKIP_NATIVE: &str = "ENVOIX_BINDINGS_SKIP_NATIVE";

fn from_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
}

fn jar(directory: &Path, prefix: &str) -> Option<PathBuf> {
    let mut jars: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".jar"))
        })
        .collect();
    jars.sort();
    jars.pop()
}

/// `kotlin-compiler-embeddable` ships inside the Gradle distribution, which is
/// the only Kotlin compiler this workspace has.
fn kotlin_lib(directory: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let mut nested: Vec<PathBuf> = fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    nested.sort();
    for path in &nested {
        if path.file_name().is_some_and(|name| name == "lib")
            && jar(path, "kotlin-compiler-embeddable").is_some()
        {
            return Some(path.clone());
        }
    }
    nested.iter().find_map(|path| kotlin_lib(path, depth - 1))
}

fn require(found: Option<PathBuf>, what: &str, key: &str) -> PathBuf {
    found.unwrap_or_else(|| {
        panic!(
            "{what} was not found. Install it, point {key} at it, or set {SKIP_NATIVE}=1 to run \
             this crate's gates without the replay — which leaves the router unproven."
        )
    })
}

fn run(label: &str, command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label} did not start: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{label} failed ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn provenance() -> DutyProvenanceView {
    DutyProvenanceView {
        card: "00112233445566aa".to_owned(),
        generation: 9,
        request: "efefefefefefefefefefefefefefefef".to_owned(),
    }
}

fn order(work: WorkView) -> String {
    let frame = DutyFrame {
        body: DutyBody::Order(DutyOrderView {
            provenance: provenance(),
            work,
        }),
    };
    String::from_utf8(encode_duty_frame(&frame).expect("an order encodes"))
        .expect("the encoder emits utf-8")
}

/// Every arm, and what the router must have delivered for it.
fn vectors() -> Vec<(String, &'static str, Option<OutcomeCodeView>)> {
    vec![
        (
            order(WorkView::Notification(NotificationWorkView {
                notice: NoticeView::TransferComplete,
            })),
            "effects=[notice=TRANSFER_COMPLETE card=00112233445566aa]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Notification(NotificationWorkView {
                notice: NoticeView::ActionNeeded,
            })),
            "effects=[notice=ACTION_NEEDED card=00112233445566aa]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Notification(NotificationWorkView {
                notice: NoticeView::TransferFailed,
            })),
            "effects=[notice=TRANSFER_FAILED card=00112233445566aa]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Lock(LockWorkView {
                directive: LockDirectiveView::Hold,
            })),
            "effects=[lock=HOLD]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Lock(LockWorkView {
                directive: LockDirectiveView::Release,
            })),
            "effects=[lock=RELEASE]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Foreground(ForegroundWorkView {
                active_transfers: 4_294_967_295,
            })),
            "effects=[foreground=4294967295]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Publication(PublicationWorkView {
                staged: "artifacts/one".to_owned(),
                display_name: "report.pdf".to_owned(),
                total_bytes: 9_223_372_036_854_775_807,
            })),
            "effects=[publish staged=artifacts/one name=report.pdf \
             total=9223372036854775807]",
            Some(OutcomeCodeView::Completed),
        ),
        (
            order(WorkView::Courier),
            "effects=[courier]",
            Some(OutcomeCodeView::Internal),
        ),
        (
            order(WorkView::SourceHandle),
            "effects=[source card=00112233445566aa gen=9]",
            Some(OutcomeCodeView::Completed),
        ),
        // The three the vocabulary carries and this platform does not execute.
        // Outstanding means the duty is re-delivered, so a router that reported
        // anything at all here would discharge work nobody did.
        (order(WorkView::Grant), "effects=[]", None),
        (order(WorkView::Staging), "effects=[]", None),
        (order(WorkView::OpenShare), "effects=[]", None),
    ]
}

#[test]
fn the_shipped_router_replays_authority_encoded_orders() {
    if std::env::var_os(SKIP_NATIVE).is_some() {
        eprintln!("{SKIP_NATIVE} is set: the duty router replay did NOT run");
        return;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let app = manifest
        .join("../../../apps/envoix-flutter/android/app/src/main/kotlin")
        .canonicalize()
        .expect("the android app sources");
    let generated = manifest
        .join("../../l5/envoix-bindings/generated/kotlin")
        .canonicalize()
        .expect("the generated kotlin");
    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("duty-router");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create the harness directory");

    let cases = vectors();
    fs::write(
        work.join("orders.txt"),
        cases
            .iter()
            .map(|(frame, _, _)| frame.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("write the order vectors");

    let java = require(
        from_env("ENVOIX_JAVA").or_else(|| on_path("java")),
        "a JVM",
        "ENVOIX_JAVA",
    );
    let lib = require(
        from_env("ENVOIX_KOTLIN_LIB")
            .or_else(|| kotlin_lib(&home().join(".gradle/wrapper/dists"), 5)),
        "the Kotlin compiler (kotlin-compiler-embeddable, shipped with Gradle)",
        "ENVOIX_KOTLIN_LIB",
    );
    let stdlib = jar(&lib, "kotlin-stdlib").expect("kotlin-stdlib beside the compiler");
    let org_json = require(
        from_env("ENVOIX_ORG_JSON_JAR").or_else(|| {
            Some(home().join(".cache/envoix-bindings/org-json.jar")).filter(|jar| jar.is_file())
        }),
        "the org.json reference jar (Android bundles it; the JVM does not)",
        "ENVOIX_ORG_JSON_JAR",
    );
    let classes = work.join("classes");
    let classpath = format!("{}:{}", stdlib.display(), org_json.display());
    run(
        "the Kotlin compiler",
        Command::new(&java)
            .arg("-cp")
            .arg(lib.join("*"))
            .arg("org.jetbrains.kotlin.cli.jvm.K2JVMCompiler")
            .arg("-no-stdlib")
            .arg("-classpath")
            .arg(&classpath)
            .arg("-d")
            .arg(&classes)
            .arg(generated.join("EnvoixDuty.kt"))
            // The file the app ships, compiled unchanged.
            .arg(app.join("app/envoix/host/DutyRouter.kt"))
            .arg(manifest.join("tests/native/DutyRouterReplay.kt")),
    );
    let stdout = run(
        "the duty router replay",
        Command::new(&java)
            .arg("-cp")
            .arg(format!("{}:{classpath}", classes.display()))
            .arg("app.envoix.host.DutyRouterReplayKt")
            .arg(work.join("orders.txt")),
    );

    let lines: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), cases.len(), "one line per vector:\n{stdout}");
    for (index, (_, effects, outcome)) in cases.iter().enumerate() {
        let line = lines[index];
        let (head, report) = line
            .split_once(" report=")
            .unwrap_or_else(|| panic!("vector {index} printed no report: {line}"));
        assert_eq!(
            head,
            format!("{index} {effects}"),
            "vector {index} routed differently than the authority meant"
        );
        let Some(expected) = outcome else {
            assert_eq!(
                report, "OUTSTANDING",
                "vector {index} reported work this platform does not perform"
            );
            continue;
        };
        // RUST decodes what Kotlin emitted. Decoding it on the Kotlin side would
        // only have proven Kotlin agrees with itself; the real ledger admits
        // from this direction, and it matches provenance EXACTLY — so a report
        // whose provenance drifted is admitted by nobody and the duty stays in
        // flight until the process dies.
        let frame = decode_duty_frame(report.as_bytes())
            .unwrap_or_else(|error| panic!("vector {index} report is not a duty frame: {error:?}"));
        let DutyBody::Report(returned) = frame.body else {
            panic!("vector {index} returned something that is not a report");
        };
        assert_eq!(
            returned.provenance,
            provenance(),
            "vector {index} reported a provenance the ledger would refuse"
        );
        assert_eq!(returned.outcome, *expected, "vector {index} outcome");
    }
}
