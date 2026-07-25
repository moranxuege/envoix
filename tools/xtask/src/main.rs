use std::process::ExitCode;

use xtask::{arch_check, identifier_check, record_payload, release_gate, workspace_root};

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        eprintln!(
            "usage: cargo run -p xtask -- \
             <identifier-check|arch-check|release-gate|record-payload>"
        );
        return ExitCode::FAILURE;
    };
    let root = workspace_root();
    let result = match command.as_str() {
        "identifier-check" => identifier_check(&root).map(|report| {
            println!(
                "identifier-check: checked={} pending={} violations={}",
                report.checked,
                report.pending.len(),
                report.violations.len()
            );
            for pending in &report.pending {
                println!("pending: {pending}");
            }
            report.ensure_success()
        }),
        "arch-check" => arch_check(&root).map(|report| {
            println!(
                "arch-check: packages={} manifests={} violations={}",
                report.packages_checked,
                report.manifests_checked,
                report.violations.len()
            );
            report.ensure_success()
        }),
        "release-gate" => release_gate(&root).map(|report| {
            println!(
                "release-gate: artifacts={} identities={} distribution={} disagreements={}",
                report.artifacts,
                report.identities,
                report.distribution.as_str(),
                report.disagreements.len()
            );
            report.ensure_success()
        }),
        "record-payload" => record_payload(&root).map(|record| {
            println!(
                "record-payload: libraries={} manifest={}",
                record.library.len(),
                record.build_manifest_sha256
            );
            Ok(())
        }),
        _ => {
            eprintln!("unknown xtask subcommand: {command}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) | Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
