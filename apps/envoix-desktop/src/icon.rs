//! The window and taskbar icon, drawn rather than shipped as an asset.
//!
//! A filled accent tile reads on both light and dark taskbars, and the arrow is
//! the same transfer motif the activity card uses.

const SIZE: usize = 64;
const RADIUS: f32 = 14.0;
/// `accent` from the light palette, so the icon matches the app's primary.
const ACCENT: [u8; 3] = [0x16, 0x77, 0xFF];

pub fn window_icon() -> egui::IconData {
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let coverage = tile_coverage(x as f32 + 0.5, y as f32 + 0.5);
            if coverage <= 0.0 {
                continue;
            }
            let pixel = (y * SIZE + x) * 4;
            let on_arrow = arrow_coverage(x as f32 + 0.5, y as f32 + 0.5);
            let channel = |accent: u8| -> u8 {
                let value = f32::from(accent) + (255.0 - f32::from(accent)) * on_arrow;
                value.round().clamp(0.0, 255.0) as u8
            };
            rgba[pixel] = channel(ACCENT[0]);
            rgba[pixel + 1] = channel(ACCENT[1]);
            rgba[pixel + 2] = channel(ACCENT[2]);
            rgba[pixel + 3] = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

/// Signed coverage of a rounded square, antialiased over one pixel.
fn tile_coverage(x: f32, y: f32) -> f32 {
    let half = SIZE as f32 / 2.0;
    let inner = half - RADIUS;
    let dx = (x - half).abs() - inner;
    let dy = (y - half).abs() - inner;
    let distance = if dx > 0.0 && dy > 0.0 {
        dx.hypot(dy)
    } else {
        dx.max(dy)
    };
    (RADIUS - distance).clamp(0.0, 1.0)
}

/// A downward arrow: a vertical shaft above a triangular head.
fn arrow_coverage(x: f32, y: f32) -> f32 {
    let centre = SIZE as f32 / 2.0;
    let dx = (x - centre).abs();
    let shaft = dx <= 3.5 && (16.0..=36.0).contains(&y);
    // The head narrows linearly from its baseline down to the tip.
    let head = (36.0..=48.0).contains(&y) && dx <= (48.0 - y);
    if shaft || head { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the generated icon out so it can be eyeballed; the shapes come
    /// from arithmetic, not from an asset anyone has looked at.
    #[test]
    fn icon_renders() {
        let icon = window_icon();
        assert_eq!(icon.rgba.len(), SIZE * SIZE * 4);
        // The centre sits on the arrow shaft and must be opaque and pale.
        let centre = ((SIZE / 2) * SIZE + SIZE / 2) * 4;
        assert_eq!(icon.rgba[centre + 3], 255);
        assert!(icon.rgba[centre] > 200, "arrow should be near-white");
        // The extreme corner falls outside the rounded tile.
        assert_eq!(icon.rgba[3], 0, "corner should be transparent");

        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-preview");
        std::fs::create_dir_all(&directory).expect("preview directory");
        image::RgbaImage::from_raw(SIZE as u32, SIZE as u32, icon.rgba)
            .expect("icon buffer")
            .save(directory.join("icon.png"))
            .expect("write icon");
    }
}
