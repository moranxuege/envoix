//! Envoix desktop demo for Windows and Linux.
//!
//! A thin egui front end over `envoix-client`, covering one route: a
//! directional invitation through the deployed rendezvous.

// Release builds must not pop a console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod icon;
mod qr;
mod theme;
mod widgets;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 720.0])
            .with_min_inner_size([960.0, 600.0])
            .with_title("Envoix")
            .with_icon(icon::window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Envoix",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new(&cc.egui_ctx)))
        }),
    )
}
