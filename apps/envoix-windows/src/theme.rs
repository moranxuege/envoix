use egui::Color32;

// Exact v0.3 semantic roles from docs/v0.3/presentation.md. Keep these values
// aligned with Apple Theme.swift and Android Theme.kt.
pub(crate) const BACKGROUND: Color32 = Color32::from_rgb(0xF8, 0xFA, 0xFD);
pub(crate) const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub(crate) const SURFACE_RAISED: Color32 = Color32::from_rgb(0xFD, 0xFE, 0xFF);
pub(crate) const TEXT: Color32 = Color32::from_rgb(0x0A, 0x13, 0x30);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0x53, 0x62, 0x7A);
pub(crate) const BORDER: Color32 = Color32::from_rgb(0xE6, 0xEC, 0xF5);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x16, 0x77, 0xFF);
pub(crate) const ACCENT_DARK: Color32 = Color32::from_rgb(0x0D, 0x47, 0xA1);
pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(0xEA, 0xF2, 0xFF);
pub(crate) const SUCCESS: Color32 = Color32::from_rgb(0x14, 0x7A, 0x4B);
pub(crate) const SUCCESS_SOFT: Color32 = Color32::from_rgb(0xDD, 0xF3, 0xE7);
pub(crate) const WARNING: Color32 = Color32::from_rgb(0xA0, 0x5A, 0x00);
pub(crate) const WARNING_SOFT: Color32 = Color32::from_rgb(0xFF, 0xF2, 0xDC);
pub(crate) const DANGER: Color32 = Color32::from_rgb(0xE7, 0x4C, 0x3C);
pub(crate) const DANGER_SOFT: Color32 = Color32::from_rgb(0xFF, 0xF4, 0xF2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_the_normative_v03_light_roles() {
        assert_eq!(BACKGROUND, Color32::from_rgb(0xF8, 0xFA, 0xFD));
        assert_eq!(SURFACE, Color32::WHITE);
        assert_eq!(SURFACE_RAISED, Color32::from_rgb(0xFD, 0xFE, 0xFF));
        assert_eq!(TEXT, Color32::from_rgb(0x0A, 0x13, 0x30));
        assert_eq!(MUTED, Color32::from_rgb(0x53, 0x62, 0x7A));
        assert_eq!(BORDER, Color32::from_rgb(0xE6, 0xEC, 0xF5));
        assert_eq!(ACCENT, Color32::from_rgb(0x16, 0x77, 0xFF));
        assert_eq!(ACCENT_DARK, Color32::from_rgb(0x0D, 0x47, 0xA1));
        assert_eq!(ACCENT_SOFT, Color32::from_rgb(0xEA, 0xF2, 0xFF));
        assert_eq!(SUCCESS, Color32::from_rgb(0x14, 0x7A, 0x4B));
        assert_eq!(SUCCESS_SOFT, Color32::from_rgb(0xDD, 0xF3, 0xE7));
        assert_eq!(WARNING, Color32::from_rgb(0xA0, 0x5A, 0x00));
        assert_eq!(WARNING_SOFT, Color32::from_rgb(0xFF, 0xF2, 0xDC));
        assert_eq!(DANGER, Color32::from_rgb(0xE7, 0x4C, 0x3C));
        assert_eq!(DANGER_SOFT, Color32::from_rgb(0xFF, 0xF4, 0xF2));
    }
}
