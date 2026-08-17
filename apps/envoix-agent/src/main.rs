use std::process::ExitCode;

#[cfg(unix)]
mod unix_agent;

#[cfg(unix)]
#[tokio::main]
async fn main() -> ExitCode {
    match unix_agent::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!("error: envoix-agent currently targets Linux/WSL and requires Unix sockets");
    ExitCode::FAILURE
}
