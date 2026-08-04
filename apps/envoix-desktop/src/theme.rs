//! The mobile app's design tokens, ported verbatim.
//!
//! Values mirror `android/app/src/main/java/dev/envoix/app/ui/Theme.kt` so the
//! desktop demo reads as the same product. Radii and type sizes follow the
//! dominant Compose choices in `android/app/src/main/java/dev/envoix/app/ui/`.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Visuals};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_raised: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub accent: Color32,
    pub accent_strong: Color32,
    pub accent_soft: Color32,
    /// Ink for text sitting on `accent`. The dark palette's accent is a light
    /// blue, so white on it would be unreadable.
    pub on_accent: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF8, 0xFA, 0xFD),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_raised: Color32::from_rgb(0xFD, 0xFE, 0xFE),
    text: Color32::from_rgb(0x0A, 0x13, 0x30),
    muted: Color32::from_rgb(0x53, 0x62, 0x7A),
    line: Color32::from_rgb(0xE6, 0xEC, 0xF5),
    accent: Color32::from_rgb(0x16, 0x77, 0xFF),
    accent_strong: Color32::from_rgb(0x0D, 0x47, 0xA1),
    accent_soft: Color32::from_rgb(0xEA, 0xF2, 0xFF),
    on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    success: Color32::from_rgb(0x14, 0x7A, 0x4B),
    success_soft: Color32::from_rgb(0xDD, 0xF3, 0xE7),
    warning: Color32::from_rgb(0xA0, 0x5A, 0x00),
    danger: Color32::from_rgb(0xE7, 0x4C, 0x3C),
};

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x06, 0x11, 0x26),
    surface: Color32::from_rgb(0x0A, 0x18, 0x30),
    surface_raised: Color32::from_rgb(0x10, 0x21, 0x3D),
    text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    muted: Color32::from_rgb(0xB8, 0xC5, 0xD9),
    line: Color32::from_rgb(0x26, 0x3B, 0x5D),
    accent: Color32::from_rgb(0x66, 0xA9, 0xFF),
    accent_strong: Color32::from_rgb(0xA8, 0xCE, 0xFF),
    accent_soft: Color32::from_rgb(0x14, 0x2F, 0x55),
    on_accent: Color32::from_rgb(0x06, 0x11, 0x26),
    success: Color32::from_rgb(0x61, 0xD6, 0x9A),
    success_soft: Color32::from_rgb(0x16, 0x36, 0x2A),
    warning: Color32::from_rgb(0xFF, 0xC1, 0x66),
    danger: Color32::from_rgb(0xF0, 0x71, 0x67),
};

/// Card corner radius: `RoundedCornerShape(16.dp)`, the dominant Compose choice.
pub const RADIUS_CARD: u8 = 16;
/// Buttons and inputs: `RoundedCornerShape(12.dp)`.
pub const RADIUS_CONTROL: u8 = 12;
/// Status pills read as fully rounded at the app's pill height.
pub const RADIUS_PILL: u8 = 20;

pub const PAD_SCREEN: i8 = 20;
pub const PAD_CARD: i8 = 16;

/// Family name for the bold face. Roboto is vendored so Windows and Linux
/// render the same weights the Android app uses.
const BOLD: &str = "roboto-bold";

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "roboto".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Roboto-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        BOLD.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Roboto-Bold.ttf"
        ))),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "roboto".to_owned());

    // The bold family falls back through the proportional chain, so glyphs
    // Roboto Bold lacks (arrows, symbols) still resolve instead of drawing tofu.
    let mut bold_chain = vec![BOLD.to_owned()];
    bold_chain.extend(proportional.iter().cloned());
    fonts
        .families
        .insert(FontFamily::Name(BOLD.into()), bold_chain);
    ctx.set_fonts(fonts);
}

pub fn card_frame(palette: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.line))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::same(PAD_CARD))
}

/// Applies the palette to egui's own widget visuals so stock widgets inherit
/// the app's surfaces instead of egui's defaults.
pub fn apply(ctx: &egui::Context, palette: &Palette, dark: bool) {
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.selection.bg_fill = palette.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);

    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(RADIUS_CONTROL);
        widget.bg_stroke = Stroke::new(1.0, palette.line);
    }
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.line);
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.hovered.weak_bg_fill = palette.accent_soft;
    visuals.widgets.active.weak_bg_fill = palette.accent_soft;

    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 10.0);
    });
}
