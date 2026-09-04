//! Native Windows shell for the persistent per-user Envoix Agent.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod presentation;
#[cfg(any(windows, test))]
mod theme;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod controller;

#[cfg(windows)]
fn main() -> eframe::Result {
    let screenshot_mode = std::env::var_os("ENVOIX_UI_SCREENSHOT").is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1120.0, 720.0])
        .with_min_inner_size([900.0, 580.0])
        .with_title("Envoix");
    if screenshot_mode {
        viewport = viewport
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_position([24.0, 24.0]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Envoix",
        options,
        Box::new(|creation| Ok(Box::new(app::EnvoixWindowsApp::new(&creation.egui_ctx)))),
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("envoix-windows is supported only on Windows");
}
