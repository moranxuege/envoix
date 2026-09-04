//! Native Windows shell for the persistent per-user Envoix Agent.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod presentation;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod controller;

#[cfg(windows)]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([900.0, 580.0])
            .with_title("Envoix"),
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
