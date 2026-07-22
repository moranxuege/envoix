use std::process::ExitCode;

use xtask::{arch_check, identifier_check, workspace_root};

fn main() -> ExitCode {
    let Some(command) = std::env::args().nth(1) else {
        eprintln!("usage: cargo run -p xtask -- <identifier-check|arch-check>");
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
