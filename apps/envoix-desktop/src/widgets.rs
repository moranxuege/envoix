//! The app's recurring controls: segmented toggles, status pills, section
//! labels, the primary action button, and the transfer progress bar.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme::{self, Palette, RADIUS_CONTROL, RADIUS_PILL};

/// `I WANT TO`, `SAVE TO` - 11sp muted small caps above a control.
pub fn section_label(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::bold(11.0))
            .color(palette.muted),
    );
}

pub fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    let font = theme::bold(11.0);
    let width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), fg)
        .size()
        .x;
    let (rect, _) = ui.allocate_exact_size(vec2(width + 22.0, 24.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS_PILL), bg);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
}

/// Two-up toggle matching `Show QR / Scan QR` and `Send / Receive`.
pub fn segmented(
    ui: &mut egui::Ui,
    palette: &Palette,
    labels: &[&str],
    selected: usize,
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), palette.bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(RADIUS_CONTROL),
        Stroke::new(1.0, palette.line),
        StrokeKind::Inside,
    );

    let inner = rect.shrink(3.0);
    let segment_width = inner.width() / labels.len() as f32;
    let mut clicked = None;
    for (index, label) in labels.iter().enumerate() {
        let segment = Rect::from_min_size(
            pos2(inner.min.x + index as f32 * segment_width, inner.min.y),
            vec2(segment_width, inner.height()),
        );
        let response = ui.interact(
            segment,
            ui.id().with(("segmented", labels.len(), index)),
            Sense::click(),
        );
        if response.clicked() {
            clicked = Some(index);
        }
        let active = index == selected;
        if active {
            painter.rect_filled(
                segment,
                CornerRadius::same(RADIUS_CONTROL.saturating_sub(2)),
                palette.accent,
            );
        }
        painter.text(
            segment.center(),
            Align2::CENTER_CENTER,
            label,
            theme::bold(14.0),
            if active {
                palette.on_accent
            } else {
                palette.muted
            },
        );
        if !active && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    clicked
}

/// Full-width accent button, the app's single primary affordance per surface.
pub fn primary_button(ui: &mut egui::Ui, palette: &Palette, text: &str, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), 46.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if !enabled {
        palette.line
    } else if response.is_pointer_button_down_on() {
        palette.accent_strong
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        palette.accent.gamma_multiply(0.92)
    } else {
        palette.accent
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme::bold(15.0),
        if enabled {
            palette.on_accent
        } else {
            palette.muted
        },
    );
    enabled && response.clicked()
}

/// Bordered secondary action, used for file selection and copy.
pub fn ghost_button(ui: &mut egui::Ui, palette: &Palette, text: &str) -> bool {
    let font = theme::bold(13.0);
    let width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), palette.accent)
        .size()
        .x;
    let (rect, response) = ui.allocate_exact_size(vec2(width + 28.0, 36.0), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        palette.accent_soft
    } else {
        palette.surface
    };
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, palette.line),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        palette.accent,
    );
    response.clicked()
}

/// Transfer direction indicator. Painted rather than typed: neither Roboto nor
/// egui's fallback chain carries U+2191/U+2193, so the glyph renders as tofu.
pub fn direction_arrow(ui: &mut egui::Ui, color: Color32, up: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(13.0, 16.0), Sense::hover());
    let painter = ui.painter();
    let x = rect.center().x;
    let (tail, head) = if up {
        (rect.max.y - 1.0, rect.min.y + 1.0)
    } else {
        (rect.min.y + 1.0, rect.max.y - 1.0)
    };
    painter.line_segment([pos2(x, tail), pos2(x, head)], Stroke::new(2.0, color));
    let back = if up { 5.0 } else { -5.0 };
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(x, head),
            pos2(x - 4.0, head + back),
            pos2(x + 4.0, head + back),
        ],
        color,
        Stroke::NONE,
    ));
}

/// `fill` carries the transfer's health: a stalled bar in the healthy accent
/// reads as paused rather than failed.
pub fn progress_bar(ui: &mut egui::Ui, palette: &Palette, fraction: f32, fill: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), palette.line);
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.0 {
        let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
        painter.rect_filled(filled, CornerRadius::same(4), fill);
    }
}

/// Left-rail navigation entry; the active one wears the accent-soft pill the
/// bottom bar uses on mobile.
pub fn rail_item(ui: &mut egui::Ui, palette: &Palette, label: &str, active: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, CornerRadius::same(10), palette.accent_soft);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        painter.rect_filled(rect, CornerRadius::same(10), palette.bg);
    }
    painter.text(
        pos2(rect.min.x + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::bold(14.0),
        if active {
            palette.accent
        } else {
            palette.muted
        },
    );
    response.clicked()
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Draws a rounded surface card and runs `content` inside it.
pub fn card<R>(
    ui: &mut egui::Ui,
    palette: &Palette,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    theme::card_frame(palette).show(ui, content).inner
}
