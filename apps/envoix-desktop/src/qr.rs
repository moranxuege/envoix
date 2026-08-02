//! Presentation-only QR rendering for egui.
//!
//! `envoix-qr` renders to a terminal string; a painted widget needs the module
//! matrix, so this encodes the same opaque payload straight from `qrcode`.

use egui::{Color32, Rect, Sense, Vec2, pos2};
use qrcode::QrCode;
use qrcode::types::Color;

pub struct QrMatrix {
    width: usize,
    modules: Vec<bool>,
}

impl QrMatrix {
    pub fn encode(data: &str) -> Option<Self> {
        let code = QrCode::new(data.as_bytes()).ok()?;
        let width = code.width();
        let modules = code
            .into_colors()
            .into_iter()
            .map(|color| color == Color::Dark)
            .collect();
        Some(Self { width, modules })
    }

    fn is_dark(&self, row: usize, column: usize) -> bool {
        self.modules[row * self.width + column]
    }
}

/// Paints the matrix into a square of `size` points, including a quiet zone.
pub fn draw(ui: &mut egui::Ui, matrix: &QrMatrix, size: f32, fg: Color32, bg: Color32) {
    const QUIET: usize = 2;

    let (response, painter) = ui.allocate_painter(Vec2::splat(size), Sense::hover());
    let origin = response.rect.min;
    painter.rect_filled(response.rect, 8.0, bg);

    let padded = matrix.width + QUIET * 2;
    let module = size / padded as f32;
    for row in 0..matrix.width {
        for column in 0..matrix.width {
            if !matrix.is_dark(row, column) {
                continue;
            }
            let x = origin.x + (column + QUIET) as f32 * module;
            let y = origin.y + (row + QUIET) as f32 * module;
            // Overdraw by a hairline so neighbouring modules do not seam.
            painter.rect_filled(
                Rect::from_min_max(pos2(x, y), pos2(x + module + 0.5, y + module + 0.5)),
                0.0,
                fg,
            );
        }
    }
}
