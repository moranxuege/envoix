//! Presentation-only QR rendering for egui.
//!
//! `envoix-qr` renders to a terminal string; a painted widget needs the module
//! matrix, so this encodes the same opaque payload straight from `qrcode`.
//!
//! Painting is left to the caller. The invitation payload is around a kilobyte,
//! which forces a symbol of ~113 modules a side; at card width that is one pixel
//! per module, which egui's edge feathering erases outright. Only a full-window
//! view has room for a module size a camera can resolve.

use qrcode::types::Color;
use qrcode::{EcLevel, QrCode};

/// Quiet zone, in modules. The spec requires 4, and shrinking it to save space
/// stopped decoders locating the symbol at all - measured, not assumed.
const QUIET: usize = 4;

/// Below this, a camera cannot resolve individual modules on a screen.
pub const MIN_SCANNABLE_MODULE_PX: f32 = 3.0;

pub struct QrMatrix {
    width: usize,
    modules: Vec<bool>,
}

impl QrMatrix {
    pub fn encode(data: &str) -> Option<Self> {
        // Error-correction L, not the crate's default M. A screen is not a
        // printed label: nothing is torn or smudged, so the redundancy buys
        // nothing and costs a denser symbol. For a ~1 kB invitation that is the
        // difference between 121 and 109 modules a side, and module size is
        // what decides whether a camera can resolve it.
        let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L).ok()?;
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

    /// One pixel per module, for upload as a texture. Painting the modules as
    /// rectangles puts egui's edge feathering on every one of them, and that
    /// grey fringe is enough to stop a decoder at the module sizes a kilobyte
    /// payload forces. A nearest-filtered texture has hard edges at any scale.
    pub fn to_image(&self) -> egui::ColorImage {
        let side = self.padded_width();
        let mut pixels = vec![egui::Color32::WHITE; side * side];
        for row in 0..side {
            for column in 0..side {
                if self.is_dark_padded(row, column) {
                    pixels[row * side + column] = egui::Color32::BLACK;
                }
            }
        }
        egui::ColorImage {
            size: [side, side],
            pixels,
            source_size: egui::vec2(side as f32, side as f32),
        }
    }

    /// Module state in padded coordinates, so an overlay can paint the quiet
    /// zone and the symbol from one loop.
    pub fn is_dark_padded(&self, row: usize, column: usize) -> bool {
        let (Some(r), Some(c)) = (row.checked_sub(QUIET), column.checked_sub(QUIET)) else {
            return false;
        };
        r < self.width && c < self.width && self.is_dark(r, c)
    }

    /// Symbol width in modules, including the quiet zone.
    pub fn padded_width(&self) -> usize {
        self.width + QUIET * 2
    }

    /// Largest whole-pixel module size that fits `available`, and the resulting
    /// symbol size. Whole pixels matter: a fractional module lands across pixel
    /// boundaries, and the grey edges that produces are what stops a decoder
    /// resolving the finder patterns.
    pub fn fit(&self, available: f32) -> (f32, f32) {
        let module = (available / self.padded_width() as f32).floor().max(1.0);
        (module, module * self.padded_width() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invitation payload is long enough to force a large symbol, so the
    /// module size a card can afford is the thing that decides scannability.
    #[test]
    fn module_size_is_whole_pixels() {
        let matrix = QrMatrix::encode(&"a".repeat(1050)).expect("encode");
        assert!(
            matrix.padded_width() >= 100,
            "a 1050-byte payload should need a large symbol, got {}",
            matrix.padded_width()
        );
        for available in [150.0_f32, 226.0, 400.0, 565.0] {
            let (module, size) = matrix.fit(available);
            assert_eq!(module, module.floor(), "module must be whole pixels");
            assert!(size <= available, "symbol must fit the space offered");
            // Largest that fits means one pixel more per module would overflow.
            assert!(
                (module + 1.0) * matrix.padded_width() as f32 > available,
                "a larger module would still have fit {available}"
            );
        }
    }

    /// Records why the card cannot simply show a small QR: at card width the
    /// modules fall below what a camera resolves.
    #[test]
    fn a_card_sized_qr_is_not_scannable() {
        let matrix = QrMatrix::encode(&"a".repeat(1050)).expect("encode");
        let (card_module, _) = matrix.fit(226.0);
        assert!(
            card_module < MIN_SCANNABLE_MODULE_PX,
            "card-sized QR unexpectedly scannable; the enlarge path may be unnecessary"
        );
        let (large_module, _) = matrix.fit(600.0);
        assert!(
            large_module >= MIN_SCANNABLE_MODULE_PX,
            "enlarged QR still below the scannable threshold"
        );
    }

    /// A real invitation must still reach a scannable module size in the
    /// space an overlay can offer. Error-correction L is what keeps it there;
    /// the crate's default M pushed the symbol to 121 modules, and at that
    /// density decoders failed on the rendered result.
    #[test]
    fn an_invitation_sized_payload_stays_scannable() {
        let matrix = QrMatrix::encode(&"x".repeat(1050)).expect("encode");
        assert!(
            matrix.padded_width() <= 120,
            "symbol grew to {} modules; check the error-correction level",
            matrix.padded_width()
        );
        // The overlay gets roughly the window height less room for captions.
        let (module, _) = matrix.fit(640.0);
        assert!(
            module >= MIN_SCANNABLE_MODULE_PX,
            "{module} px per module in an overlay is below the scannable floor"
        );
    }
}
