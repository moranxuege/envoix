//! Presentation-only QR rendering.
//!
//! Invitation encoding, parsing, and authentication belong to
//! `envoix-invite`. This crate deliberately treats its input as an opaque
//! string so every carrier passes identical bytes to the Rust core.

use qrcode::QrCode;
use qrcode::types::Color;

/// Render an opaque payload as a terminal QR code.
pub fn render_terminal_qr(data: &str) -> Option<String> {
    const QUIET: usize = 4;

    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.into_colors();
    let padded = width + QUIET * 2;
    let is_dark = |row: usize, col: usize| -> bool {
        if row < QUIET || col < QUIET || row >= width + QUIET || col >= width + QUIET {
            return false;
        }
        colors[(row - QUIET) * width + (col - QUIET)] == Color::Dark
    };

    let mut output = String::new();
    for row in (0..padded).step_by(2) {
        for col in 0..padded {
            let top = is_dark(row, col);
            let bottom = row + 1 < padded && is_dark(row + 1, col);
            output.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        output.push('\n');
    }
    Some(output)
}

#[cfg(test)]
mod tests;
