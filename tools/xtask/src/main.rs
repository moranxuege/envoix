use std::process::ExitCode;

use xtask::{
    arch_check, deploy_check, identifier_check, record_bundled, record_payload, release_gate,
    workspace_root,
};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        eprintln!(
            "usage: cargo run -p xtask -- \
             <identifier-check|arch-check|deploy-check [environment]|release-gate\
             |record-payload|record-bundled>"
        );
        return ExitCode::FAILURE;
    };
    let subject = arguments.next();
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
        "deploy-check" => deploy_check(&root).map(|report| {
            println!(
                "deploy-check: environments={} deployable={} blocked={} violations={}",
                report.environments,
                report.deployable.len(),
                report.blocked.len(),
                report.violations.len()
            );
            for name in &report.deployable {
                println!("deployable: {name}");
            }
            for blocked in &report.blocked {
                println!("blocked: {blocked}");
            }
            match subject.as_deref() {
                Some(environment) => report.ensure_deployable(environment),
                None => report.ensure_success(),
            }
        }),
        "release-gate" => release_gate(&root).map(|report| {
            println!(
                "release-gate: artifacts={} identities={} disagreements={}",
                report.artifacts,
                report.identities,
                report.verdict.disagreements.len()
            );
            for line in report.invariant_summary() {
                println!("evaluated: {line}");
            }
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
        "record-bundled" => record_bundled(&root).map(|record| {
            println!("record-bundled: bundled={}", record.bundled.len());
            for library in &record.bundled {
                println!(
                    "accepted: {} {} {}",
                    library.soname, library.abi, library.sha256
                );
            }
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
