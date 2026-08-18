use std::process::ExitCode;

#[cfg(any(unix, windows))]
#[path = "unix_agent.rs"]
mod agent;

#[cfg(any(unix, windows))]
#[tokio::main]
async fn main() -> ExitCode {
    match agent::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn main() -> ExitCode {
    eprintln!("error: envoix-agent supports Unix sockets and Windows Named Pipes");
    ExitCode::FAILURE
}
