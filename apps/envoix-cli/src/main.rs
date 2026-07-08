use std::io;
use std::process::ExitCode;
use std::time::Duration;

mod args;
mod render;

use args::{Cli, Command, TransferPlan};
use clap::Parser;
use envoix_client::api;
use envoix_client::api::TransferError;
use envoix_client::{
    IdentityConfig, SPAKE2_EXPERIMENTAL_WARNING, TransferDirection, TransferSummary,
};
use render::EventRenderer;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Initialize the tracing subscriber. `RUST_LOG` always wins; otherwise the
/// verbosity flag picks the filter - default keeps libraries at `warn` and
/// envoix at `info`, `-v` shows envoix internals, `-vv` adds iroh internals
/// (path selection, hole-punching). Output goes to stderr so stdout stays
/// clean for `--json`.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::{EnvFilter, fmt};
    let default_filter = match verbosity {
        0 => "envoix=info,warn",
        1 => "envoix=debug,warn",
        _ => "envoix=trace,iroh=debug,warn",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_target(false)
        .init();
}

async fn run(cli: Cli) -> Result<(), TransferError> {
    let output = EventRenderer::new(cli.json);
    match cli.command {
        Command::Send(args) => {
            let plan = args.into_plan()?;
            let summary = execute(plan, TransferDirection::Send, output).await?;
            eprintln!(
                "sent {} bytes from {}",
                summary.bytes_transferred, summary.file_name
            );
        }
        Command::Receive(args) => {
            let plan = args.into_plan()?;
            let summary = execute(plan, TransferDirection::Receive, output).await?;
            eprintln!(
                "received {} bytes into {}",
                summary.bytes_transferred, summary.file_name
            );
        }
    }
    Ok(())
}

/// Starts the planned transfer and drives it to completion.
async fn execute(
    plan: TransferPlan,
    direction: TransferDirection,
    output: EventRenderer,
) -> Result<TransferSummary, TransferError> {
    if let Some(note) = &plan.note {
        eprintln!("{note}");
    }
    let client = api_client(plan.config.as_deref(), plan.identity)?;
    // One source today (no fallback); routing through `run` unifies the path
    // with the app and makes multi-source fallback a matter of the source list.
    let transfer = client.run(api::TransferRequest {
        direction,
        path: plan.path,
        sources: vec![plan.source],
        options: plan.options,
    })?;
    run_transfer(transfer, output).await
}

/// How long a first Ctrl-C waits for a clean shutdown before forcing exit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Builds the new-API client from the CLI's config/identity arguments.
fn api_client(
    config_path: Option<&std::path::Path>,
    identity: IdentityConfig,
) -> Result<api::Client, TransferError> {
    eprintln!("{SPAKE2_EXPERIMENTAL_WARNING}");
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = identity;
    Ok(client)
}

/// Drives a new-API transfer to completion: renders its event stream and
/// handles Ctrl-C (first press cancels gracefully; a second press or the
/// grace period elapsing forces exit).
async fn run_transfer(
    mut transfer: api::Transfer,
    mut renderer: EventRenderer,
) -> Result<TransferSummary, TransferError> {
    let interrupted = tokio::select! {
        _ = drain_events(&mut transfer, &mut renderer) => false,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| {
                TransferError::input(format!("failed to listen for interrupt signal: {error}"))
            })?;
            true
        }
    };
    if interrupted {
        eprintln!("interrupt received; shutting down (Ctrl-C again to force)...");
        transfer.cancel();
        tokio::select! {
            _ = drain_events(&mut transfer, &mut renderer) => {}
            _ = tokio::signal::ctrl_c() => return Err(TransferError::cancelled(transfer.phase())),
            _ = tokio::time::sleep(SHUTDOWN_GRACE) => return Err(TransferError::cancelled(transfer.phase())),
        }
    }
    transfer.wait().await
}

async fn drain_events(transfer: &mut api::Transfer, renderer: &mut EventRenderer) {
    while let Some(event) = transfer.next_event().await {
        renderer.render(event);
    }
}
