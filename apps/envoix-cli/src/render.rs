//! Terminal rendering of the transfer event stream: human progress lines or
//! JSON lines for tooling.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use envoix_client::TransferDirection;
use envoix_client::api;
use envoix_qr::render_terminal_qr;

const PROGRESS_RENDER_INTERVAL: Duration = Duration::from_millis(250);

/// How transfer events reach the user: human terminal rendering, or one JSON
/// object per line on stdout for tooling (the observation-campaign driver).
#[derive(Debug)]
pub(crate) enum EventOutput {
    Console(Renderer),
    Json,
}

impl EventOutput {
    pub(crate) fn new(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Console(Renderer::default())
        }
    }

    pub(crate) fn render(&mut self, event: api::StampedEvent) {
        match self {
            Self::Console(renderer) => renderer.render(event.event),
            Self::Json => match serde_json::to_string(&event) {
                Ok(line) => println!("{line}"),
                Err(error) => eprintln!("failed to encode event as JSON: {error}"),
            },
        }
    }
}

impl Clone for EventOutput {
    fn clone(&self) -> Self {
        match self {
            // A fresh renderer per transfer: progress state is per-transfer.
            Self::Console(_) => Self::Console(Renderer::default()),
            Self::Json => Self::Json,
        }
    }
}

/// Renders the unified event stream to the terminal. Single-task use, so no
/// locking - unlike the legacy sink, which was called from library threads.
#[derive(Debug, Default)]
pub(crate) struct Renderer {
    progress: Option<ProgressState>,
}

impl Renderer {
    fn render(&mut self, event: api::TransferEvent) {
        use api::TransferEvent as E;
        match event {
            // Contextual lines (which mode, which broker) are printed by the
            // dispatch site that knows the arguments.
            E::Binding { .. } | E::Pairing { .. } => {}
            E::Connecting => eprintln!("connecting..."),
            E::Connected { path } | E::PathChanged { path } => {
                eprintln!("data path: {path}");
            }
            E::Advertised {
                peer,
                token,
                invite,
            } => {
                eprintln!("peer: {peer}");
                if let Some(invite) = invite {
                    eprintln!("\ninvite: {invite}");
                    if let Some(qr) = render_terminal_qr(&invite) {
                        eprintln!("{qr}");
                    }
                } else if let Some(token) = token {
                    eprintln!("token: {token}");
                }
            }
            E::Started {
                direction,
                file_name,
                total_bytes,
                bytes_resumed,
                ..
            } => {
                let state = ProgressState {
                    file_name,
                    direction,
                    total_bytes,
                    bytes_resumed,
                    started_at: Instant::now(),
                    last_rendered_at: Instant::now(),
                };
                render_progress_line(&state, bytes_resumed, false);
                self.progress = Some(state);
            }
            E::Progress {
                bytes_transferred, ..
            } => {
                if let Some(state) = self.progress.as_mut()
                    && state.last_rendered_at.elapsed() >= PROGRESS_RENDER_INTERVAL
                {
                    render_progress_line(state, bytes_transferred, false);
                    state.last_rendered_at = Instant::now();
                }
            }
            E::Verifying {
                direction,
                file_name,
                bytes_to_hash,
                ..
            } => render_hash_line(direction, &file_name, bytes_to_hash, false),
            E::Verified {
                direction,
                file_name,
                bytes_hashed,
                ..
            } => render_hash_line(direction, &file_name, bytes_hashed, true),
            E::Completed {
                bytes_transferred, ..
            } => match self.progress.take() {
                Some(state) => render_progress_line(&state, bytes_transferred, true),
                None => eprintln!("completed {bytes_transferred} bytes"),
            },
            E::Failed { direction, reason } => match self.progress.take() {
                Some(state) => render_transfer_failure_line(&state, &reason),
                None => render_attempt_failure_line(direction, &reason),
            },
            // The event enum is non_exhaustive; render nothing for variants
            // this build does not know.
            _ => {}
        }
    }
}

#[derive(Debug)]
struct ProgressState {
    file_name: String,
    direction: TransferDirection,
    total_bytes: u64,
    bytes_resumed: u64,
    started_at: Instant,
    last_rendered_at: Instant,
}

fn render_hash_line(direction: TransferDirection, file_name: &str, bytes_hashed: u64, done: bool) {
    let verb = match direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let status = if done { "verified" } else { "verifying" };
    let line = format!(
        "{:<24} {:>9} {}",
        format!("{verb} {}", display_file_name(file_name)),
        format_bytes(bytes_hashed),
        status,
    );

    let mut stderr = io::stderr().lock();
    if done {
        let _ = writeln!(stderr, "\r{line:<80}");
    } else {
        let _ = write!(stderr, "\r{line:<80}");
        let _ = stderr.flush();
    }
}

fn render_transfer_failure_line(state: &ProgressState, reason: &str) {
    let verb = match state.direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let line = format!(
        "{:<24} failed: {}",
        format!("{verb} {}", display_file_name(&state.file_name)),
        reason
    );
    eprintln!("\r{line:<80}");
}

fn render_attempt_failure_line(direction: TransferDirection, reason: &str) {
    let verb = match direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    eprintln!("{verb} attempt failed: {reason}");
}

fn render_progress_line(state: &ProgressState, bytes_transferred: u64, done: bool) {
    let percent = bytes_transferred
        .saturating_mul(100)
        .checked_div(state.total_bytes)
        .unwrap_or(100);
    let elapsed = state.started_at.elapsed();
    let bytes_this_attempt = bytes_transferred.saturating_sub(state.bytes_resumed);
    let bytes_per_second = if elapsed.is_zero() {
        0.0
    } else {
        bytes_this_attempt as f64 / elapsed.as_secs_f64()
    };
    let eta = eta(
        bytes_transferred,
        state.total_bytes,
        bytes_this_attempt,
        bytes_per_second,
    );
    let verb = match state.direction {
        TransferDirection::Send => "send",
        TransferDirection::Receive => "recv",
    };
    let line = format!(
        "{:<24} {:>4}% {:>9}/{:<9} {:>10}/s {:>5}",
        format!("{verb} {}", display_file_name(&state.file_name)),
        percent.min(100),
        format_bytes(bytes_transferred),
        format_bytes(state.total_bytes),
        format_bytes(bytes_per_second as u64),
        eta,
    );

    let mut stderr = io::stderr().lock();
    if done {
        let _ = writeln!(stderr, "\r{line:<80}");
    } else {
        let _ = write!(stderr, "\r{line:<80}");
        let _ = stderr.flush();
    }
}

fn display_file_name(file_name: &str) -> String {
    const MAX_LEN: usize = 19;

    if file_name.chars().count() <= MAX_LEN {
        return file_name.to_owned();
    }

    let suffix: String = file_name
        .chars()
        .rev()
        .take(MAX_LEN - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("~{suffix}")
}

fn eta(
    bytes_transferred: u64,
    total_bytes: u64,
    bytes_this_attempt: u64,
    bytes_per_second: f64,
) -> String {
    if bytes_transferred >= total_bytes {
        return "00:00".into();
    }
    if bytes_this_attempt == 0 || bytes_per_second <= 0.0 {
        return "--:--".into();
    }

    let remaining = total_bytes - bytes_transferred;
    format_duration(Duration::from_secs_f64(remaining as f64 / bytes_per_second))
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes}B")
    } else if value < 10.0 {
        format!("{value:.1}{unit}")
    } else {
        format!("{value:.0}{unit}")
    }
}
